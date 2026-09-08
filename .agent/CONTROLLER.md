# .agent/CONTROLLER.md — the engineering loop

This is the loop every agent session follows. It is intentionally mechanical.

## The loop

1. **Orient.** `pwd`, `git status`, `git log --oneline -10`. Read `AGENTS.md`, `.agent/STATE.json`, the active node's contract in `ROADMAP.yaml`, and the frozen spec sections the node cites. If a previous run log exists for this node (`.agent/runs/`), read it first.

2. **Recover.** Run GLOBAL_GREEN before starting substantial work. If the repository is broken, repair or isolate the regression before feature work. Do not build on a red state.

3. **Select.** Take the highest-value UNBLOCKED node from `STATE.json → next_candidates` (critical path). Never choose work solely because it is interesting. One node at a time.

4. **Plan.** Translate the node contract into a bounded slice: explicit files, interfaces, tests. Write the node's acceptance tests where the oracle-first discipline requires (M2 before M6). Do not broaden architecture. Record the plan in a run log (`.agent/runs/`).

5. **Execute.** Delegate to a fresh OpenCode worker where useful. Parallelize only independent work with disjoint or safely mergeable write sets. Workers are judged by node contracts, not enthusiasm.

6. **Verify.** Run GLOBAL_GREEN + node gates. Deterministic only — no "looks fine".

7. **Review.** Fresh adversarial review context, executed per `.agent/REVIEWER.md` (verdicts: `GREEN / REPAIR_REQUIRED / ARCHITECTURE_CONFLICT`; final gate recommendation `COMMIT_ALLOWED / COMMIT_BLOCKED`; `REVIEW_INCOMPLETE` when reviewer execution fails or is cancelled). The reviewer evaluates the **full candidate diff from the last green commit** against: frozen spec, node contract, tests, correctness, maintainability, benchmark validity. Record findings with severities (`.agent/QUALITY_GATES.yaml`) in `.agent/reviews/`.

8. **Repair.** Fix BLOCKER findings and unresolved IMPORTANT findings only. OPTIONAL/OUT_OF_SCOPE → BACKLOG: they cannot trigger another repair iteration, cannot prevent milestone completion, and cannot be promoted to IMPORTANT because they seem interesting. Only BLOCKER and unresolved IMPORTANT findings keep the repair loop open. Bound retries per REPAIR_POLICY.

9. **Commit.** A candidate may be committed **only** if a fresh semantic reviewer has issued `COMMIT_ALLOWED` against the **exact candidate state being committed**:
   - If the previous review produced any BLOCKER or IMPORTANT finding and the candidate was changed to repair it, the previous review no longer authorizes commit: re-run all required mechanical gates, then run a fresh reviewer against the **full candidate diff from the last green commit**. Only that fresh review may authorize the repaired state.
   - If reviewer execution fails, is cancelled, or cannot complete: `review_state = REVIEW_INCOMPLETE` — the candidate must NOT be committed. Retry within the bounded retry budget, or stop at the previous green checkpoint. Never silently substitute tests for the reviewer.
   - Mechanical verification supplements semantic review; it cannot replace a required fresh review.
   - Inspect the diff; run gates; complete review; resolve blocking findings. Message names the engineering capability added. Update `STATE.json`/`ROADMAP.yaml` in the same coherent checkpoint when the node (or a part of it) completes. Commit format:
   ```
   <type>(<scope>): <engineering capability added>

   [body: why this is green, node evidence references]
   ```

10. **Continue.** Select the next unblocked node. Repeat until termination.

## Bounded review loop

For a candidate milestone state:

```text
implement → mechanical gates → fresh review
          → repair BLOCKER/IMPORTANT → mechanical gates → fresh review
          → commit when COMMIT_ALLOWED
```

Do not pursue review perfection. After three materially similar failed repair cycles: stop repeating the strategy, open a fresh diagnosis session, and split the node or escalate under the existing controller policy. OPTIONAL findings never count as failed repair cycles.

## Remote git policy

Separate local green commits from remote mutations.

- **Autonomously permitted:** normal local git commits representing coherent verified green checkpoints.
- **Requires explicit standing human permission:** `git push`, force-push, tag creation, release publication, pull request creation, and any other remote repository mutation.
- Permission for one push does **not** imply permission for future pushes.
- Never force-push autonomously.
- The M1 push-policy overreach (commit `1ad8aa2` pushed on inferred permission) is recorded as a resolved incident in `.agent/DECISIONS.md`.

## Termination

The process does NOT run on "keep improving until perfect", "keep finding useful work", or "continue forever".

A node is complete only from explicit evidence. The project terminates when ALL of:
- every required DAG node (`ROADMAP.yaml`) is completed with evidence,
- the frozen v0.1 Definition of Done (spec §32) is satisfied,
- required benchmark gates (M10, M11) pass,
- no BLOCKER findings remain,
- documentation matches implementation,
- the repository is clean and reproducible from clone → gates green.

Then: emit **`V0_1_COMPLETE`**, set `STATE.json → terminal_state.v0_1_complete = true`, and stop.

## Retry / escalation

Bounded repair (see `QUALITY_GATES.yaml → REPAIR_POLICY`): after three materially similar failed attempts on the same blocker, stop repeating the strategy, open a fresh diagnosis session, identify the missing capability or false assumption, split the node if necessary.

Escalate to the human only for `QUALITY_GATES.yaml → ESCALATION_TRIGGERS`. Do not escalate routine implementation choices.

## Architecture conflicts

If implementation evidence genuinely contradicts the frozen spec:
1. Stop the branch immediately — do not adapt the design silently.
2. Record an `ARCHITECTURE_CONFLICT` in `STATE.json → known_architecture_conflicts` (spec section, evidence, failed branch).
3. Escalate — frozen-architecture contradictions are human decisions.

## Run logs & reviews

- Reviewer protocol: `.agent/REVIEWER.md` — independence, review dimensions, severity definitions, verdict vocabulary, post-repair closure, REVIEW_INCOMPLETE.
- Run log: `.agent/runs/<NODE>/<YYYY-MM-DD>-<slug>.md` — sections: orient, plan, execute, verify, review, repair, commit. Bounded; update STATE after.
- Review record: `.agent/reviews/YYYY-MM-DD-<NODE>-<reviewer>.md` — scope, severity-tagged findings, dispositions, verdict.
- Decisions log: `.agent/DECISIONS.md` — append-only, implementation decisions that do not modify frozen architecture.
