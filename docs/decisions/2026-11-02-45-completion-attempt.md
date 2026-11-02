# Bind workflow completion to its original step and attempt

Issue #45. Decided by the worker to satisfy report 04's replacement-attempt fencing.

- **Reuse the persisted step position and attempt count.** `WorkflowComplete` adds optional
  `expected_step` and `expected_attempt` fields. No schema or launch-token changes are needed.
  The daemon checks the original identity and retains its existing write-transaction check
  that the workflow and authenticated sender have not changed during evidence evaluation.
- **Agent requests must carry both fields.** Older agent requests still deserialize but
  receive `StaleRevision` with an actionable explanation. Existing owner commands without
  either field retain the intentional current-step override; supplied owner identities are
  checked too. Requirements and owner-only waivers keep their existing checks.
- **Clients bind before preparing results.** CLI packets print `--step` and `--attempt`;
  completion accepts those values explicitly. The TUI captures them when its handoff opens.
  Neither client silently fetches a new identity for an old result.
- **Rejected:** accepting missing agent identity, or refreshing it during submission, because
  either makes an obsolete result indistinguishable from a replacement attempt's result.

Evidence: the daemon's [replacement attempt fixture](../../crates/cockpit-daemon/tests/workflows/durability.rs),
[CLI entry point](../../crates/cockpit-cli/tests/workflow.rs) and
[TUI handoff fixture](../../crates/cockpit-tui/tests/workflows.rs).
The dispatch fixtures model the persisted pre-transport and pre-receipt boundaries using
real scheduling transactions, daemon startup, isolated processes and socket dispatch.
They do not claim a new process-launch recovery algorithm or live-host acceptance.
