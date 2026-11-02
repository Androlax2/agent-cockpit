# Codex delivered-turn approval routing

Issue [#436](https://github.com/Androlax2/agent-cockpit/issues/436): accepted on
Linux with **Codex 0.166.0**, configured model **gpt-6.1-sol**, on 2026-11-02.
Source [PR #576](https://github.com/Androlax2/agent-cockpit/pull/576) merged as
`8d916180cce0a664a94553e7ac735442be51c7ff`; the run used its reviewed source head
`5d2df5edc0ddc8ebed861dc94a70db64c7864eed` without code changes.

## Approval and result

Theo selected a separate validation home after the existing home's execution
rules caused a preflight refusal. Host request `fa4200cef0fd` created a private
`/home/theo/.codex-validation-436` with the selected model and file credential
storage, then completed official `codex login` (exit 0). Credentials were issued
by sign-in, not copied; the existing home and its rules were preserved.

Separate host request **`edeea0394e55`** authorized one live attempt: initial READY
prompt, one delivered command-approval request, possible title usage, account
usage and conversation writes, cancellation only. Exact command:

```sh
env CI=1 CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=1 \
  COCKPIT_CODEX_PROGRAM=/home/theo/.local/npm/bin/codex \
  COCKPIT_CODEX_SIGNED_IN_HOME=/home/theo/.codex-validation-436 \
  COCKPIT_CODEX_ALLOW_SIGNED_IN=1 \
  agent-build cargo nextest run --locked --offline -p cockpit-codex \
  --test approval_routing --run-ignored only \
  -E 'test(=a_signed_in_delivered_turn_reaches_the_real_tui)' \
  --retries 0 --no-capture
```

Nextest runs serially with `--no-capture` and warned that it ignores the supplied
test-thread setting. Run ID: `5a293c5d-e394-4c34-bff6-33cb4a5be638`.

```text
Codex 0.166.0; action=StartedTurn; owner TUI received approval and returned "cancel"; signed-in model; account usage and home writes authorized separately
Summary [   9.872s] 1 test run: 1 passed, 19 skipped
[exit 0]
```

The [runner](../../crates/cockpit-codex/tests/common/approval_probe.rs) asserts
distinct delivery/owner connections, delivery disconnection, approval on the
persistent TUI, matching request/thread/turn identities, the TUI's cancellation
response, an interrupted matching turn and server request resolution. Cockpit
does not answer the approval. This satisfies the delivered-start criterion;
signed-in steer was not exercised. The conditional starting-connection-only
remediation is unnecessary for this observed version and setup.

## Preserved failures and boundaries

- Historical `d139c002664e`: custom-keymap preflight refusal, exit 101, no turn.
- `9ffb83dbb87e`: inherited execution-rule refusal, exit 100; 0 passed, 1 failed,
  19 skipped, no turn. Inventory `230631053269` found the existing home's
  `rules/default.rules`. Its contents were not read or changed.
- Neither refusal is reclassified by the later separately approved success.
  All guards remain intact; the clean home does not prove arbitrary inherited
  configurations compatible.
- The explicit npm path is the official Node launcher, not the Greywall wrapper
  first on the host PATH. The earlier launcher hash in the
  [capability evidence](../../crates/cockpit-codex/tests/fixtures/codex-0.166.0/README.md)
  is not a native payload digest.
- This is Linux provider acceptance, not WSL2/macOS, archive onboarding, sandbox
  enforcement or daemon-launched instruction recognition (#419). The runner
  disables hooks/skills and cannot establish #419. No release or legacy apply.
- The validation home retains its newly issued credentials and conversation
  records; this receipt authorizes neither reuse nor deletion. Future live runs
  need new exact host/account approval.
