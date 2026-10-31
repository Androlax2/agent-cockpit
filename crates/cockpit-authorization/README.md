# cockpit-authorization

Effective grants: the only place that answers whether an action may happen.

The owner gives a session grants: one permission (`read-state`, `change-state`,
`run-commands`, `read-pane-contents`, `write-clipboard`) over one scope (the owner's whole
state, one session or one task). `evaluate` answers a request from the authenticated actor
with the decision and the authorization revision it read; a refusal names the grant that
would unblock it, or says the action is the owner's alone (`host-actions`). Every grant and
revocation increments the revision, and nothing is cached, so a revocation applies from the
next evaluation that reads it.

Prose, payload fields, skills, team messages and plugin manifests request; they are not
inputs to `evaluate` and cannot widen a grant (reports 11, 14, 17). Ids match exactly, and
permission names parse only when spelled exactly.

## Dependency edges

- Depends on: `cockpit-turso`, for the transaction its functions take and its migrations.
- Depends on no product capability (report 02); `tests/crate_graph.rs` enforces it.
- Used by: the daemon, which evaluates every mediated request before acting on it.

## Tests

- `tests/evaluation.rs`: grant, evaluate, revoke and restart on a real state database.
- `tests/no_widening.rs`: issue #25's acceptance, checked against a reference model over every
  grantable grant, adversarial ids and permission spellings.
