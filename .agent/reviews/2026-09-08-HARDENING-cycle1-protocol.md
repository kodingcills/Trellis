# Review 2026-09-08 HARDENING cycle 1 — harness control-plane protocols (fresh context)

scope: harness-hardening candidate diff from 1ad8aa2 — .gitignore, docs/architecture/TRELLIS_V0_1_FROZEN.md relocation, AGENTS.md, .agent/{REVIEWER,CONTROLLER,QUALITY_GATES,ROADMAP,STATE,DECISIONS,BACKLOG}
spec_sections: §1–§33 numbering integrity; §2 principles; §32 DoD references
findings:
  - [IMPORTANT] M1 review-record verdict line falsely asserted "re-verified by second fresh context" while run log/STATE/DECISIONS record the repair re-verification subagent as CANCELLED with mechanical substitution — false paper trail contradicting the iteration's own purpose
  - [OPTIONAL] REVIEW_INCOMPLETE retry budget referenced but never numerically defined
  - [OPTIONAL] review-record verdict vocabulary drift (reviews/README vs REVIEWER.md)
  - [OUT_OF_SCOPE] crates/trellis-core/src/lib.rs:8 crate doc cites spec by title only, not canonical path
dispositions:
  - IMPORTANT verdict-line → corrected: append-only ERRATUM added to .agent/reviews/2026-09-08-M1-fresh-adversarial.md stating REVIEW_INCOMPLETE semantics; reviews/README.md now requires mandatory review_closure field
  - OPTIONAL retry budget → BACKLOG
  - OPTIONAL vocab drift → corrected as part of the reviews/README.md verdict-line convention (same edit as the IMPORTANT's process guard)
  - OUT_OF_SCOPE lib.rs doc → BACKLOG
verdict: REPAIR_REQUIRED
final_gate: COMMIT_BLOCKED
review_closure: fresh re-review — full candidate diff re-reviewed by an independent fresh context (cycle 2, see 2026-09-08-HARDENING-cycle2-provenance.md)
