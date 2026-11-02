# Measure delivered-turn approvals through the owner's real TUI

Issue #436. Decided by the worker within the authorized isolated-conformance scope.

- **Keep production delivery unchanged.** On installed Codex 0.166.0, both isolated
  delivered-start and delivered-steer probes observed the approval on the real TUI after
  the delivery connection closed. The TUI cancelled it, the app-server resolved its request
  ID, and the matching turn ended interrupted. The evidence does not support introducing
  a persistent approval owner or refusing delivery on this version.
- **Keep provenance separate.** These measurements use a local scripted Responses model,
  not an account. The older capability pin stays 0.161.0. Signed-in acceptance remains
  open in #436. One host-approved run on 2026-11-02 exited 101 at the custom-keymap guard
  before any thread/model turn; it is a refusal, not signed-in routing evidence.
- **Record, never answer.** The test proxy forwards bytes unchanged. Only the actual TUI
  supplies a cancellation, and the test checks its connection and request ID. A fixture
  created from a protocol schema alone would not prove this routing.
- **Keep helper threads distinct.** The TUI generates titles in a tool-less helper
  conversation. Counting model calls without distinguishing it initially misidentified
  that helper as a delivered turn; the fixture now distinguishes the tool-bearing calls.
- **Respect the TUI's input quarantine.** Codex 0.166.0 drains pending input at a protected
  screen boundary for up to one second. The probe waits for the prompt, lets that interval
  elapse, then presses Escape; an early key can be discarded without answering the request.
- **Reject an unsafe live fixture setup.** Empty TOML tables merge with the owner's config,
  so `mcp_servers={}` cannot disable existing servers. The live probe asks Codex for its
  effective config and refuses enabled MCP servers and inherited execution rules before
  starting any thread. Rules could otherwise allow the fixture command without a prompt.
  Unsupported layers or managed requirements need a separate conformance plan. Legacy
  `notify` commands are independent of `features.hooks`; `notify=[]` disables them for both
  processes, with a native sentinel regression.
- **Check project settings and key bindings.** `config/read` without `cwd` omits project
  layers. Both probe modes now pass the work directory and reject nonempty project layers,
  even when marked disabled: CLI trust can differ from the trust used by the config reader.
  Custom TUI keymaps are refused before launch because Escape can be rebound to approval.
  Native regressions use only temporary configuration and never exercise an acceptance key.
- **Require signed-in provenance.** A directory or custom local model is not signed-in
  evidence. The live preflight requires the built-in OpenAI provider, official endpoints
  and a non-null `account/read` result requiring OpenAI authentication. It never prints
  account details or asks for a token refresh.
- **Bound observations precisely.** A steer has a preceding start connection; only closure
  of the connection carrying that steer counts. The proxy closes the upstream socket before
  recording disconnect. Version and tmux commands use deadlines and bounded output so a
  stalled wrapper cannot defeat a test deadline.

Evidence: [approval routing tests](../../crates/cockpit-codex/tests/approval_routing.rs)
and [run instructions](../../crates/cockpit-codex/README.md#delivered-turn-approval-conformance-436).
Source checks used the exact `rust-v0.166.0` tag:
[request fan-out](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server/src/outgoing_message.rs),
[input quarantine](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/tui/src/tui/input_boundary.rs),
[config merging](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/config/src/merge.rs),
and [managed execution rules](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/config/src/requirements_exec_policy.rs).
Those sources explain the measurement; they do not replace the executable proof.
