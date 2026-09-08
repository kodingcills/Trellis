# .agent/reviews/ — adversarial review records

One file per review, named:

    YYYY-MM-DD-<NODE>-<reviewer-slug>.md

Reviewer is a **fresh context** with no stake in the implementation. Review
against: frozen spec (cited sections), node contract (`ROADMAP.yaml`), tests,
correctness, maintainability, benchmark validity.

Required structure:

    # Review <date> <node>
    scope: what was reviewed (commits, files, node contract)
    spec_sections: frozen spec sections checked against
    findings:
      - [BLOCKER|IMPORTANT|OPTIONAL|OUT_OF_SCOPE] description
    dispositions:
      - finding → corrected | dispositioned (rationale) | BACKLOG
    verdict: GREEN | REPAIR_REQUIRED | ARCHITECTURE_CONFLICT
    final_gate: COMMIT_ALLOWED | COMMIT_BLOCKED
    review_closure: fresh re-review | REVIEW_INCOMPLETE

`review_closure` is mandatory and states how post-repair closure was
achieved: either a fresh reviewer's COMMIT_ALLOWED against the full
candidate diff, or REVIEW_INCOMPLETE (reviewer failed/cancelled — commits
are then forbidden, see `.agent/REVIEWER.md`). Verdict vocabulary follows
`.agent/REVIEWER.md`.

Every finding carries exactly one severity per `QUALITY_GATES.yaml`.
Review records are append-only; corrections happen in code, not by editing records.
