# Architecture review ledger

Raw findings, one line each, appended during the 2026-10-29 review. Triaged
into `arch-review`-labeled issues at the end of each pass; the status column
says where each one went. A finding that rests on an **[inferred]** or
**[assumed]** claim in `as-intended.md` says so in the Rests-on column.

Severity: **high** (a rule or invariant is broken in a way that can lose
work or spend tokens wrongly) · **med** (a boundary or ownership drift that
will keep costing fixes) · **low** (clean-up, naming, dead code).

Status: `raw` · `issue #N` · `dropped: <why>` · `known: #N` (already an
open issue).

| # | Pass | Module | Step | Sev | Rests on | Finding | Status |
|---|---|---|---|---|---|---|---|
