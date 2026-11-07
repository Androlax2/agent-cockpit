# A coordinator grant covers the coordinator's own project unless the owner names every project

Issue #1131. Decided by the worker, within the issue's terms.

- **`Scope::Project(name)` in `cockpit-authorization`, checked by the same `evaluate`.** The
  daemon asks over the project of the session acted on (its checkout's project,
  `cockpit_sessions::coordinator_scope`), or of the directory a start is in. A project grant
  covers exactly that project; an owner grant covers every project. A directory in no project
  is asked over `Scope::Owner`, so only a grant over everything covers it.
- **The project's name, not its id, is the scope.** It is what the owner and the brief read
  (`over project alpha`) and what workflows already key on. Removing a project and registering
  another under the same name carries the grant over: an owner action, accepted.
- **`torlek session grant` defaults to the granted session's own project; `--all-projects` is
  the one opt-out.** An old client that sends no flag gets the narrower grant, never a wider
  one.
- **A holder keeps one grant of each coordinator permission.** Granting it over another scope
  (or `start-sessions` with other terms) revokes and replaces it
  (`grant_coordinator`, `grant_start_sessions`), so `session revoke SESSION PERMISSION` takes
  back whatever scope it had (`revoke_permission`).
- **Grants stored before stay over everything.** Migration 7 rebuilds `authorization_grants`
  to widen `scope_kind` and copies every row unchanged
  (`crates/cockpit-authorization/tests/project_scope.rs`), so the owner's current
  coordinators keep their reach until regranted.
- **An approval re-checks the very `start-sessions` grant the start was asked under**, active,
  instead of evaluating it over everything: the start's project was checked when it was asked.
- **`agent sessions` lists only covered sessions; a message, start or end outside the scope is
  refused naming the grant's own scope only** (`... grant covers project alpha only`). Naming
  the scope asked would tell the coordinator another project's sessions and their projects; a
  message to an unknown session is asked over everything, so it reads the same as one in
  another project. `agent end` checks that the coordinator started the session before the
  scope, since no wider grant would let it end another's.
- **Rejected:** a `--project NAME` flag naming any project. The issue asks for the session's
  own project or every project; another project is a later, separate grant if needed.
