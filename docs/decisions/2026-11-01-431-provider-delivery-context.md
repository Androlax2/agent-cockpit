# Deliver using the recorded launch context and an explicit unsupported result

Issue #431 (and #450, #416). Decided by the worker, to preserve launch identity and avoid duplicate actions.

- **Only `Delivered::Unsupported` allows terminal fallback.** The provider's capability is
  checked first. Errors and timeouts may follow a successful write; the daemon returns an
  uncertain outcome and queued messaging keeps its existing unacknowledged-delivery path.
  Inspecting an error's prose cannot distinguish these cases safely.
- **Persist socket reach per execution.** The launch transaction records the sandbox plan's
  mapping or refusal. Launch confirmation records the same verified process identity in
  this context and the authenticated agent channel in one transaction. The channel replaces
  its current launch; sessions retain the context of each execution. Recomputing a mapping
  from current configuration could change how a running sandbox is reached after restart.
- **Retain the original provider command in the same transaction.** `DeliveryTarget::launch`
  needs the endpoint and credentials the provider chose at launch (#416). The private host
  database retains the program, ordered arguments and ordered environment as byte strings,
  plus its original conversation id. Delivery reads that record after a daemon restart;
  it never regenerates a launch or reads endpoint credentials from agent-writable files.
  This record excludes Cockpit launch tokens and the inherited owner environment, which
  are added after the provider returns its command. No credential fields enter client views.
- **The daemon verifies listeners.** Sessions receive its socket connector, preserving the
  dependency direction. Old launches lacking recorded context refuse socket delivery and
  name the required session restart; terminal-only adapters still work.
- **Hold the operation lock until blocking work returns.** Timing out the caller does not
  release a still-running delivery onto a replacement launch.

Evidence: [daemon messaging tests](../../crates/cockpit-daemon/tests/session_messaging.rs),
[mapped Codex tests](../../crates/cockpit-daemon/tests/codex_messaging.rs),
[OpenCode endpoint test](../../crates/cockpit-daemon/tests/opencode_messaging.rs), and
[schema tests](../../crates/cockpit-sessions/tests/schema.rs).

## Revisited 2026-11-01 (#416)

Decided by the worker to implement #407's adapter-performed delivery contract. OpenCode
provider operations use the host-retained launch endpoint; the third bullet above is the
retention this relies on.

- **DeliveryTarget carries optional host-retained launch metadata.** OpenCode recovers its
  loopback port and password from the command the host launched. Nothing reads credentials
  or endpoint addresses out of the agent-writable session directory. Debug output of the
  target omits the command. Codex continues using the checked socket interface unchanged.
- **The provider contract exposes native command discovery and invocation.** Neutral command
  types preserve source labels; the `commands` capability carries tested/discovered
  provenance. Other providers default to unsupported. OpenCode's existing server methods
  implement the operations, without a new process or dependency.
- **An accepted inbox message is `Delivered::Accepted`.** `prompt_async` queues behind work,
  so acceptance cannot honestly claim that a turn started or joined the active turn.
  Cancel requires the server's affirmative response before reporting interruption.
- **Rejected:** writing the password to a session file (agent-controlled endpoint selection),
  or an adapter cache keyed only on the session directory (lost on daemon restart). The #431
  lane owns retaining launch metadata and passing it from its daemon delivery calls.
- **Evidence:** [provider/server tests](../../crates/cockpit-opencode/tests/server.rs) exercise
  trait-object calls, authentication, literal text, command sources and refusal paths.
