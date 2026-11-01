# The first-run flow is a checklist recomputed from the owners' facts, and sandbox consent is the owner's own `agents.sandbox` line

Issue #100. Decided by the implementing session, within report 09 (every non-available item
names the next action) and report 11 (owner settings grant, the repository cannot).

- **`cockpit init` and `cockpit` alone print a checklist; they never prompt or write.** Six
  steps: host readiness, sandbox and approvals, pick a provider, sign in, add a project, start
  a session. Each is `done`, `todo`, `waits` (an earlier step comes first) or `unknown`, and
  the last line is the one action to take now. Every run recomputes the steps from the
  readiness report, the owner's configuration, the registered projects and the sessions, so an
  interrupted setup resumes where it stands and the tests drive it as a process with no
  terminal. The actions are the existing verbs (`project add`, `session new`, …); a second
  path that performs them would drift from them.
- **It lives in `cockpit-cli`, from facts the daemon already serves.** No new crate, request or
  dependency edge: the CLI asks `readiness`, `project-list` and `session-list` over one
  connection. Without a daemon it reads the configuration and names starting the daemon as the
  first action, like `cockpit doctor`.
- **Sandbox consent is `agents.sandbox` written in the owner's file.** The step explains what
  the sandbox restricts, what needs the owner (`ask run`, `ask guide`), how approvals work and
  whether agents share the desktop session; it is `done` once the owner's file sets
  `agents.sandbox` (`"required"`, or `"off"`, which the step names as UNSANDBOXED). An explicit
  owner setting is durable, greppable, owner-only and already read by every launch; the product
  default alone is not a choice the owner made. No new state or marker file.
- **Sign-in `unknown` does not hold the setup.** Until the adapters report it (#443) the step
  shows the readiness report's own next action and the setup completes without it;
  `needs-authorization` from an adapter makes it `todo`.
- **`cockpit` alone prints the checklist while a step is `todo` or `waits`, the usage once the
  setup is done** (exit 2, as before). `cockpit --help` prints the usage at any time.
- **Rejected:** an interactive wizard (prompts, a provider picker). It cannot be driven in a
  test or a script without a terminal, and it would repeat what the verbs already do.
- **Rejected:** a "setup done" marker file. It would be state outside the daemon's database
  that goes stale when a project is removed; the facts themselves say whether a step is done.
