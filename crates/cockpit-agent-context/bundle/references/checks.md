# Checks and evidence

A result counts as checked only through a receipt: one run of a check, bound to the exact
source (head and uncommitted changes), its definition, environment and the files it covered.

- Run the project's checks yourself while you work; that tells you, not Torlek.
- `torlek check gate [--project P] [--checkout PATH]` shows, for the exact source in front
  of you, each required check: `checked`, `stale` (and why), `unverified`, or `missing`, with
  its next action.
- Receipts come from `torlek check run CHECK --project P`, which the owner runs. From a
  session, ask for it: `torlek ask run --reason "receipt for the gate" "torlek check run
  CHECK --project P"`.
- A check runs where the owner's sandbox setting says, and its receipt says where it ran.
  Sandboxed, it has no agent socket and no GitHub token; a check that is
  `unavailable` there names why, and running it again does not change that. A `docker`
  check names its `image`; Torlek does not run docker checks yet, so they are
  `unavailable`.
- A host a sandboxed check was refused (`403` from its proxy) goes to the owner as one
  network request of that check; the run's output ends with a `torlek:` line naming it.
  Once the owner allows it always, run the check again; after a denial its runs keep
  failing, and the output names the denial.
- `torlek check claim CHECK STATEMENT` records what you say passed. It is shown to the
  next agent and the human and never counts as evidence.
- An edited file makes its receipt stale at once: check again after your last change.

Report outcomes exactly: `passed`, `failed`, `unavailable`, `blocked`, `cancelled` or
`unknown`. Never round `unknown` up to `passed`.
