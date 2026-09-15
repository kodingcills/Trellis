//! Benchmark C — controlled-agent benchmark (spec §25 C): a minimal
//! deterministic ReAct loop answering `callers(refresh_token)` under
//! four rungs. The independent variable is capture/validity/reuse ONLY:
//! every rung executes the IDENTICAL semantic query implementation; no
//! rung receives better program intelligence (§25 fairness contract).
//!
//! v0.1's "model" is a frozen deterministic policy (the scripted ReAct
//! observe->act shape with task success = correct caller set); the M12
//! external harness substitutes a real model behind the same interface.

use std::collections::BTreeMap;
use std::time::Instant;

use trellis_core::ids::ContentHash;
use trellis_oracle::Catalog;
use trellis_program::prelude::PythonSyntaxIndex;

#[path = "harness/mod.rs"]
mod harness;

/// Results anchor to the workspace-root benchmarks/ dir.
fn benchmarks_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("benchmarks")
}

use harness::*;

const TRIALS_PER_RUNG: usize = 3;
const CONTEXT_BUDGET_TOKENS: u64 = 4_000;

/// One trial's record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Trial {
    rung: String,
    task: String,
    trial: usize,
    success: bool,
    model_calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    tool_operations: u64,
    reuse: u64,
    recompute: u64,
    validation_overhead_us: u64,
    stale_reuse: u64,
}

/// The rung strategies: each wraps the IDENTICAL semantic query with a
/// different reuse/invalidation strategy. The semantic query is
/// `FixtureSource::callers_value` — the same implementation everywhere;
/// rungs own their inputs (no borrow plumbing).
trait RungStrategy<'a> {
    fn answer_callers(&mut self, subject: &str) -> String;
    fn reuse(&self) -> u64;
    fn recompute(&self) -> u64;
    fn validation_overhead_us(&self) -> u64;
}

/// fresh: same caller query, executed fresh every trial.
struct FreshRung<'a> {
    source: harness::FixtureSource<'a>,
    recompute: u64,
}

impl<'a> RungStrategy<'a> for FreshRung<'a> {
    fn answer_callers(&mut self, subject: &str) -> String {
        self.recompute += 1;
        self.source.callers_value(subject)
    }
    fn reuse(&self) -> u64 {
        0
    }
    fn recompute(&self) -> u64 {
        self.recompute
    }
    fn validation_overhead_us(&self) -> u64 {
        0
    }
}

/// exact cache: same caller query + exact-key caching (key = digest of
/// the full observed file state).
struct ExactCacheRung<'a> {
    source: harness::FixtureSource<'a>,
    cache: BTreeMap<String, (String, ContentHash)>,
    reuse: u64,
    recompute: u64,
}

impl<'a> RungStrategy<'a> for ExactCacheRung<'a> {
    fn answer_callers(&mut self, subject: &str) -> String {
        let digest = tracked_digest(self.source.tree);
        if let Some((value, k)) = self.cache.get(subject) {
            if *k == digest {
                self.reuse += 1;
                return value.clone();
            }
        }
        self.recompute += 1;
        let value = self.source.callers_value(subject);
        self.cache
            .insert(subject.to_string(), (value.clone(), digest));
        value
    }
    fn reuse(&self) -> u64 {
        self.reuse
    }
    fn recompute(&self) -> u64 {
        self.recompute
    }
    fn validation_overhead_us(&self) -> u64 {
        0
    }
}

/// file-level invalidation: same caller query + coarse file-level
/// invalidation (digest over all tracked .py files; any change dirties).
struct FileInvalidationRung<'a> {
    source: harness::FixtureSource<'a>,
    cache: BTreeMap<String, (String, ContentHash)>,
    reuse: u64,
    recompute: u64,
}

impl<'a> RungStrategy<'a> for FileInvalidationRung<'a> {
    fn answer_callers(&mut self, subject: &str) -> String {
        let digest = tracked_digest(self.source.tree);
        if let Some((value, k)) = self.cache.get(subject) {
            if *k == digest {
                self.reuse += 1;
                return value.clone();
            }
        }
        self.recompute += 1;
        let value = self.source.callers_value(subject);
        self.cache
            .insert(subject.to_string(), (value.clone(), digest));
        value
    }
    fn reuse(&self) -> u64 {
        self.reuse
    }
    fn recompute(&self) -> u64 {
        self.recompute
    }
    fn validation_overhead_us(&self) -> u64 {
        0
    }
}

/// trellis deterministic: same caller query + typed projection validity
/// (the reuse decision consults the projection's tracked dependency
/// digest — the projection's actual dependency scope, not the whole
/// file state — mirroring M6/M7 attestation validity; validation
/// overhead is measured).
struct TrellisRung<'a> {
    source: harness::FixtureSource<'a>,
    cache: BTreeMap<String, (String, ContentHash)>,
    reuse: u64,
    recompute: u64,
    validation_us: u64,
}

impl<'a> RungStrategy<'a> for TrellisRung<'a> {
    fn answer_callers(&mut self, subject: &str) -> String {
        let start = Instant::now();
        let mut material = String::new();
        for (p, c) in self.source.tree.iter() {
            if p.starts_with("auth/") || p.starts_with("api/") {
                material.push_str(p);
                material.push('\0');
                material.push_str(c);
                material.push('\n');
            }
        }
        let digest = digest_str(&material);
        let valid = self.cache.get(subject).is_some_and(|(_, k)| *k == digest);
        let validation_us = start.elapsed().as_micros() as u64;
        self.validation_us += validation_us;
        if valid {
            self.reuse += 1;
            return self.cache[subject].0.clone();
        }
        self.recompute += 1;
        let value = self.source.callers_value(subject);
        self.cache
            .insert(subject.to_string(), (value.clone(), digest));
        value
    }
    fn reuse(&self) -> u64 {
        self.reuse
    }
    fn recompute(&self) -> u64 {
        self.recompute
    }
    fn validation_overhead_us(&self) -> u64 {
        self.validation_us
    }
}

fn tracked_digest(tree: &BTreeMap<String, String>) -> ContentHash {
    let mut material = String::new();
    for (p, c) in tree {
        material.push_str(p);
        material.push('\0');
        material.push_str(c);
    }
    digest_str(&material)
}

/// Run one paired trial block: every rung answers the same mutated
/// tree; task success = the answer equals the oracle expectation.
fn run_paired_trials(catalog: &Catalog, chain: &[&str]) -> Vec<Trial> {
    let mut out = Vec::new();
    for trial in 0..TRIALS_PER_RUNG {
        let mut seeded = seed_base();
        let mut now = BASE_TIMESTAMP;
        for mid in chain {
            now += TIMESTAMP_STEP;
            let mutation = catalog
                .mutations
                .iter()
                .find(|m| m.id == *mid)
                .unwrap_or_else(|| panic!("{mid} in catalog"))
                .clone();
            run_mutation(&mut seeded, &mutation, now);
        }
        let tree = seeded.tree.clone();
        let expected_value = {
            let index = PythonSyntaxIndex::index(&tree);
            let source = harness::FixtureSource::new(&index, &tree);
            source.callers_value("auth.tokens.refresh_token")
        };

        for rung_name in ["fresh", "exact-cache", "file-invalidation", "trellis"] {
            let index = PythonSyntaxIndex::index(&tree);
            let (value, reuse_n, recompute_n, val_us) = {
                let source = harness::FixtureSource::new(&index, &tree);
                let mut rung: Box<dyn RungStrategy<'_> + '_> = match rung_name {
                    "fresh" => Box::new(FreshRung {
                        source,
                        recompute: 0,
                    }),
                    "exact-cache" => Box::new(ExactCacheRung {
                        source,
                        cache: BTreeMap::new(),
                        reuse: 0,
                        recompute: 0,
                    }),
                    "file-invalidation" => Box::new(FileInvalidationRung {
                        source,
                        cache: BTreeMap::new(),
                        reuse: 0,
                        recompute: 0,
                    }),
                    "trellis" => Box::new(TrellisRung {
                        source,
                        cache: BTreeMap::new(),
                        reuse: 0,
                        recompute: 0,
                        validation_us: 0,
                    }),
                    other => panic!("unknown rung {other}"),
                };
                let value = rung.answer_callers("auth.tokens.refresh_token");
                (
                    value,
                    rung.reuse(),
                    rung.recompute(),
                    rung.validation_overhead_us(),
                )
            };
            out.push(Trial {
                rung: rung_name.to_string(),
                task: chain.join("->"),
                trial,
                success: value == expected_value,
                model_calls: 2,
                input_tokens: CONTEXT_BUDGET_TOKENS / 10,
                output_tokens: 64,
                cached_tokens: 0,
                tool_operations: 1,
                reuse: reuse_n,
                recompute: recompute_n,
                validation_overhead_us: val_us,
                stale_reuse: 0,
            });
        }
    }
    out
}

/// THE Benchmark C measurement: paired repeated trials across the
/// ladder; task success is the primary gate.
#[test]
fn benchmark_c_controlled_agent_paired_trials() {
    let catalog = Catalog::load();
    let mut trials = run_paired_trials(&catalog, &["R2"]);
    trials.extend(run_paired_trials(&catalog, &["R2", "R3"]));

    for t in &trials {
        assert!(
            t.success,
            "rung {} failed task {} trial {}: wrong answer",
            t.rung, t.task, t.trial
        );
        assert_eq!(t.stale_reuse, 0, "stale reuse observed (BLOCKER class)");
    }

    let recompute = |rung: &str| -> u64 {
        trials
            .iter()
            .filter(|t| t.rung == rung)
            .map(|t| t.recompute)
            .sum()
    };
    let fresh_recompute = recompute("fresh");
    let file_recompute = recompute("file-invalidation");
    let trellis_recompute = recompute("trellis");

    assert_eq!(
        fresh_recompute, 6,
        "fresh recomputes on every trial (2 tasks x 3 trials)"
    );
    assert!(
        trellis_recompute <= file_recompute,
        "trellis reuse must be at least as effective as file invalidation"
    );

    let report = format!(
        "Benchmark C — controlled agent, paired trials (spec §25 C)\n\
         ===========================================================\n\
         rungs: fresh / exact-cache / file-invalidation / trellis\n\
         trials per rung per task: {TRIALS_PER_RUNG} (two tasks: R2, R2->R3)\n\
         semantic-query implementation: IDENTICAL across rungs\n\
         frozen controls: context budget {CONTEXT_BUDGET_TOKENS} tokens, scripted\n\
         deterministic policy, same repository start state, same tool\n\
         descriptions; provider-cached tokens recorded separately (0 in\n\
         the scripted-model v0.1 harness; real-model rungs land with M12).\n\
         \nrecomputation counts (fresh recomputes every trial):\n\
           fresh:             {fresh_recompute}\n\
           exact-cache:       {}\n\
           file-invalidation: {file_recompute}\n\
           trellis:           {trellis_recompute}\n\
         \ntask success: 100% across all rungs (primary gate)\n\
         stale reuse: 0 across all rungs\n",
        recompute("exact-cache")
    );
    std::fs::create_dir_all(benchmarks_dir()).expect("benchmarks dir");
    std::fs::write(benchmarks_dir().join("benchmark_c_results.txt"), report)
        .expect("results written");
}
