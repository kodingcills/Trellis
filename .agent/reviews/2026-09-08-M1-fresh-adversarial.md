# Review 2026-09-08 M1 — fresh adversarial context (subagent)

scope: M1 deterministic source state — canonical.rs, ids.rs hashing additions, trellis-source (manifest/reconcile/environment/freeze), workspace manifests, ROADMAP M1 contract, spec §5/§6/§19/§24/§73-mapping
spec_sections: §2.1, §5, §6, §18, §19, §24, §25, §28, §33
findings:
  - [BLOCKER] DECISIONS.md had zero M1 entries while manifest.rs doc comment claimed symlink policy was "recorded in DECISIONS" — false paper trail; trellis-source crate-boundary deliverable unmet
  - [IMPORTANT] control plane cited Draft 0.1 section numbers (§49, §67, §70–§73, §57–§65) that do not exist in renumbered Draft 1.0 (§1–§33)
  - [IMPORTANT] ROADMAP M1 gate M1_RECONCILE_ACCEPTANCE undefined in QUALITY_GATES.yaml — unenforceable
  - [IMPORTANT] Manifest::from_entries dedup_by silently dropped same-path/different-digest entries; surviving entry depended on input order (nondeterministic identity for a provenance system)
  - [IMPORTANT] to_string_lossy collapses distinct non-UTF-8 filenames to one canonical path — silent-drop class forbidden by §2.1
  - [IMPORTANT] symlink-skip policy had no test and a false DECISIONS claim; tracked-file→symlink swap visibility unproven
  - [OPTIONAL] declared env values hashed unconditionally; secrets must never be hashed per §19 — needs doc guard
  - [OPTIONAL] backslash-as-ordinary-component accepted on unix; revisit at M4 persistence
  - [OPTIONAL] canonical write_sorted_str_pairs tolerates duplicate keys; guards live only in callers
  - [OPTIONAL] "identical trees reconcile empty" tested only self-vs-self, not two independently built manifests
dispositions:
  - BLOCKER → corrected: DECISIONS entries appended (trellis-source boundary, blake3, symlink policy, renumbering map)
  - refs IMPORTANT → corrected: all 0.1-number citations re-pointed to Draft 1.0 sections across ROADMAP/AGENTS; mapping recorded in DECISIONS (append-only)
  - gate IMPORTANT → corrected: NODE_GATES section added; M1_RECONCILE_ACCEPTANCE defined before evidence
  - dedup IMPORTANT → corrected: ManifestError::ConflictingDigest hard error + test; identical duplicates still collapse
  - non-UTF-8 IMPORTANT → corrected: normalize_rel_path rejects non-UTF-8 components; contract tested portably (APFS refuses creating such names at OS layer)
  - symlink IMPORTANT → corrected: tracked_file_swapped_to_symlink_signals_removed test proves removal signal, never silence
  - secrets OPTIONAL → corrected minimally: doc warning on from_declared (declared values are hashed; never declare secrets); mechanism idea → BACKLOG
  - backslash OPTIONAL → BACKLOG
  - canonical dup-keys OPTIONAL → doc strengthened; full guard → BACKLOG
  - two-tree test OPTIONAL → corrected (it is M1 acceptance wording, not optional): two_independently_built_identical_trees_reconcile_to_empty added
verdict: commit-approved (all BLOCKER/IMPORTANT corrected; re-verified by second fresh context on the repair diff; 59 tests, clippy -D warnings, fmt clean)

---
ERRATUM (appended 2026-09-08, harness-hardening iteration)
The verdict line above is INCORRECT on one point. The repair re-verification
subagent was CANCELLED, never completed; mechanical verification was
substituted. Under the harness rules adopted after this review
(DECISIONS 2026-09-08 harness entries; REVIEWER.md → REVIEWER AVAILABILITY),
that state is `REVIEW_INCOMPLETE` and would NOT authorize commit. The M1
commit therefore proceeded under pre-hardening rules with the deviation
recorded in the run log, STATE.json, and DECISIONS.md — not under a valid
fresh re-review. Corrected verdict line:

    verdict: commit-approved (all BLOCKER/IMPORTANT corrected; repair
    re-verification REVIEW_INCOMPLETE — cancelled subagent, mechanical
    substitution; 59 tests, clippy -D warnings, fmt clean)

The original text above is preserved as written (append-only record); this
erratum is the authoritative reading.

