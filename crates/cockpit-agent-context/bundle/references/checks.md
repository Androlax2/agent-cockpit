# Checks and evidence

A result counts as checked only through a receipt: one run of a check, bound to the exact
source (head and uncommitted changes), its definition, environment and the files it covered.

- Run the project's checks yourself while you work; that tells you, not Cockpit.
- `cockpit check gate [--project P] [--checkout PATH]` shows, for the exact source in front
  of you, each required check: `checked`, `stale` (and why), `unverified`, or `missing`, with
  its next action.
- Receipts come from `cockpit check run CHECK --project P`, which the owner runs. From a
  session, ask for it: `cockpit ask run --reason "receipt for the gate" "cockpit check run
  CHECK --project P"`.
- `cockpit check claim CHECK STATEMENT` records what you say passed. It is shown to the
  next agent and the human and never counts as evidence.
- An edited file makes its receipt stale at once: check again after your last change.

Report outcomes exactly: `passed`, `failed`, `unavailable`, `blocked`, `cancelled` or
`unknown`. Never round `unknown` up to `passed`.
