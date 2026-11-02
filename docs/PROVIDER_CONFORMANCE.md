# Provider conformance

Part of [#51](https://github.com/Androlax2/agent-cockpit/issues/51): one fixture
suite for Claude Code, Codex and OpenCode, with explicit unsupported items and
remaining acceptance gaps. Passing this suite proves adapter behavior against
controlled inputs, not signed-in model access or full native conformance.

## Run the suite

From the repository root, with the locked dependencies already cached:

```sh
make provider-conformance
```

The [Make target](../Makefile) runs the provider API and three adapter packages,
then the daemon package's `provider_conformance` test binary, serially through
the existing build runner. Each invocation uses two build jobs and two test
threads, locked/offline dependencies, and nextest's default exclusion of ignored
tests. Missing prerequisites fail visibly; the target installs nothing.

Only the test processes' `OPENCODE_CONFIG_CONTENT` is removed: OpenCode launch
tests must not merge the caller's real inline configuration. No installed harness
or account is required. Executed sign-in tests use fake programs; native sign-in
and model probes remain ignored. The daemon target instantiates adapters directly;
it does not use shared Git/project fixtures or launch a daemon.

## Six-area contract

Pins: **Claude Code 2.1.316**, **Codex 0.161.0**, **OpenCode 1.19.5**. These are
the adapters' capability pins, not a claim that every row was measured natively.

| Area | Claude Code | Codex | OpenCode |
| --- | --- | --- | --- |
| Delivery receipts | Direct delivery returns `Unsupported`; terminal input is a fallback instruction, not an execution receipt | Idle submit returns `StartedTurn`; busy submit returns `JoinedTurn`; steer/end race and server refusals covered | Authenticated inbox submit returns `Accepted`, not proof a turn started or completed |
| Resume | Requested conversation retained with `--resume` | Requested thread retained by launcher and TUI resume arguments | Requested session retained with `--session`; invalid native session IDs refused |
| Cancellation | Direct delivery unsupported; terminal Escape has no affirmative interruption receipt | Active turn returns `Interrupted` after server acknowledgement; no active turn is refused | `Interrupted` only after authenticated affirmative abort; false/refused responses fail |
| Command discovery/invocation | Unsupported: exact `Commands` capability refusal before socket access | Unsupported: exact `Commands` capability refusal before socket access | Authenticated launch endpoint; command name, description and source retained; invocation message ID is not completion evidence |
| Evidence retrieval | Unavailable; no provider trait operation exists | Unavailable; no provider trait operation exists | Unavailable; no provider trait operation exists |
| Capability provenance | Pin reports `Tested`; different/unknown version reports `Discovered` | Same policy; native 0.166.0 evidence does not establish the 0.161.0 pin | Same policy; unknown version is unverified for every reported capability |

### Executable anchors

- **Receipts/cancellation:** [Codex delivery tests](../crates/cockpit-codex/tests/delivery.rs)
  (`a_message_to_an_idle_conversation_starts_a_turn_on_its_thread`,
  `a_message_while_a_turn_is_under_way_joins_that_turn`,
  `a_cancel_without_a_turn_under_way_is_refused`) and
  [OpenCode server tests](../crates/cockpit-opencode/tests/server.rs)
  (`the_provider_delivers_literal_inbox_text_and_cancels_through_the_launch_server`,
  `the_provider_preserves_server_refusals_and_does_not_claim_a_refused_cancel`).
  [Claude input tests](../crates/cockpit-claude/tests/input.rs) cover terminal
  fallback and refuse submission into dialogs or unknown activity.
- **Resume:** each adapter's launch tests retain the requested identity:
  [Claude](../crates/cockpit-claude/tests/launch.rs),
  [Codex](../crates/cockpit-codex/tests/launch.rs),
  [OpenCode](../crates/cockpit-opencode/tests/launch.rs).
- **Commands:** OpenCode's
  `the_provider_lists_commands_with_their_source_and_invokes_one` exercises the
  real `Provider` methods with an authenticated loopback fixture. The new
  [shared contract tests](../crates/cockpit-daemon/tests/provider_conformance.rs)
  exercise actual Claude/Codex refusal methods, Claude's direct-delivery fallback,
  and all three adapters' unknown-version provenance without opening sockets.
- **Evidence/provenance:** each adapter's capabilities tests explicitly check
  unavailable evidence and pinned/changed versions:
  [Claude](../crates/cockpit-claude/tests/capabilities.rs),
  [Codex](../crates/cockpit-codex/tests/capabilities.rs),
  [OpenCode](../crates/cockpit-opencode/tests/capabilities.rs).
  The [provider contract](../crates/cockpit-provider-api/src/lib.rs) has no evidence
  retrieval method; this suite does not invent a successful substitute.

## Native evidence and remaining acceptance

`Tested` is the adapter's existing metadata policy for its pin. Supplying a pin
to a constructor or replaying a recorded event is not a new native measurement.
Keep these boundaries separate from fixture pass/fail:

| Evidence | What it establishes | What remains open |
| --- | --- | --- |
| [#419 instruction captures](../crates/cockpit-daemon/tests/instruction_fixture/README.md) | Real daemon launches deliver brief/MCP input to a local canned model: Claude 2.1.316 print mode, Codex 0.166.0 TUI/app-server, OpenCode 1.19.5 TUI; Claude/OpenCode advertise the native skill | Signed-in recognition and Claude TUI parity unmeasured. Claude TUI failed `SELF_SIGNED_CERT_IN_CHAIN` before model input. Stored receipts predate the model-drain fix and retain their original hashes |
| [Codex approval routing](../crates/cockpit-codex/tests/approval_routing.rs), #436 | Account-free native 0.166.0 start/steer approval reaches the persistent TUI after delivery disconnects; owner cancels | Production pin 0.161.0 and signed-in routing unmeasured. Approved signed-in attempt stopped at custom-keymap preflight before a model turn |
| [OpenCode native events](../crates/cockpit-opencode/tests/fixtures/opencode-1.19.5/README.md), #417 | Recorded 1.19.5/Bun 1.4.4 tool, permission-rejection and aborted-turn events plus bridge reports; no account used | Persisted completion is a **known failure**, #516: completed → trailing busy → idle clears the stored outcome. Event replay is not proof of persisted completion |

Other fixture payloads retain their recorded native or schema-derived provenance.
Skipped native tests are not passes. The suite does not establish actual sandbox
enforcement, model understanding, a native review gate result, or owner authority.
Total-context measurement remains #479.

Shared daemon Git/project fixture isolation remains #526. The conformance target
does not invoke those project-building tests. Its ordinary Rust checks also run
under the existing workspace test gate; that broader gate has its own fixture
boundaries and is not replaced by this focused command.
