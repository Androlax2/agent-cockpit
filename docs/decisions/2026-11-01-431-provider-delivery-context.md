# Deliver using the recorded launch context and an explicit unsupported result

Issue #431 (and #450). Decided by the worker, to preserve launch identity and avoid duplicate actions.

- **Only `Delivered::Unsupported` allows terminal fallback.** The provider's capability is
  checked first. Errors and timeouts may follow a successful write; the daemon returns an
  uncertain outcome and queued messaging keeps its existing unacknowledged-delivery path.
  Inspecting an error's prose cannot distinguish these cases safely.
- **Persist socket reach per execution.** The launch transaction records the sandbox plan's
  mapping or refusal. Launch confirmation records the same verified process identity in
  this context and the authenticated agent channel in one transaction. The channel replaces
  its current launch; sessions retain the context of each execution. Recomputing a mapping
  from current configuration could change how a running sandbox is reached after restart.
- **The daemon verifies listeners.** Sessions receive its socket connector, preserving the
  dependency direction. Old launches lacking recorded context refuse socket delivery and
  name the required session restart; terminal-only adapters still work.
- **Hold the operation lock until blocking work returns.** Timing out the caller does not
  release a still-running delivery onto a replacement launch.

Evidence: [daemon messaging tests](../../crates/cockpit-daemon/tests/session_messaging.rs),
[mapped Codex tests](../../crates/cockpit-daemon/tests/codex_messaging.rs), and
[schema tests](../../crates/cockpit-sessions/tests/schema.rs).
