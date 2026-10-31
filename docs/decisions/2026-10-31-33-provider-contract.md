# The provider contract keys everything on capabilities with provenance; adapters stay behind it

Issue #33. Decided by the worker, implementing reports 02, 05 and 08 as written.

- **Provenance is `tested` only on the pinned harness version.** `cockpit-claude` pins
  `CONFORMANCE_VERSION` (2.1.316); on any other or unknown version its capabilities are
  `discovered` (reported as unverified) but still usable. Rejected: refusing an untested
  version, which would stop every session at each Claude Code update.
- **The fake provider ships in `cockpit-provider-api` behind the `fake` feature**, not as a
  new crate: the contract stays on the standard library, report 02's crate table is
  unchanged, and only test builds enable it (`cockpit-daemon`'s dev-dependency).
- **Terminal actions are keystrokes the provider chooses** (`Provider::input`): submit,
  cancel and compact return text and keys for the caller to send to the agent's pane.
  Rejected: adapters calling `cockpit-tmux`, which would give each adapter a second
  Cockpit dependency and the terminal's ownership.
- **The hook bridge is a binary of the adapter crate** (`cockpit-claude-hook`) that reports
  through the `cockpit agent report` program. Report 02 ships bridges as assets, not crates;
  running the CLI keeps adapters on `cockpit-provider-api` alone (crate-graph rule
  `provider adapters -> provider-api only`). Hooks are passed with `--settings` on the launch,
  so a nested `claude` a tool starts never reports for the session.
- **Outcome is normalized but not reported.** `TurnEnded` carries completed, failed or
  quota-stopped; the agent channel's report has no outcome field, so Claude Code does not
  report the `outcome` capability until session reports move into `cockpit-sessions` (#146).
- **No bridge, no `events`.** The daemon wires the adapter without its bridge when
  `cockpit-claude-hook` or `cockpit` is missing beside it; the capability is then not
  reported and launches subscribe no hooks, rather than hooks that fail on every event.
- **Commands, evidence and billing are named capabilities without methods.** No caller
  exists yet; each method arrives with its first caller (commands with #49, evidence with
  #40, billing with #58/#59). Claude Code does not report them meanwhile.
- **A question the agent channel would refuse is replaced, not dropped.** The bridge sends
  a plain question instead, and cuts long ones to 300 characters, so the waiting state
  always arrives. Session ids are version-4 UUIDs from `/dev/urandom`, without a new crate.
