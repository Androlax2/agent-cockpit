# OpenCode provider operations use the host-retained launch endpoint

Issue #416. Decided by the worker to implement #407's adapter-performed delivery contract.

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
