# Codex 0.166.0 conformance evidence

The capability pin was revalidated on 2026-11-02. Historical payloads remain in
[codex-0.161.0](../codex-0.161.0/README.md), with their original native/authored
provenance. No payload in that folder is claimed as a 0.166.0 capture.

## Upstream schema identity

Tag `rust-v0.166.0` resolves to commit
`6ff1e1ca3b30fc8b914720681fcf5fe62e95f856` in `openai/codex`.
All six subscribed input schemas under `codex-rs/hooks/schema/generated/`
have the same Git blob IDs as the checked-in 0.161.0 schemas:

| Schema | Git blob ID |
| --- | --- |
| `session-start.json` | `70eb76ec923c321b95d225812e16d4e7f52a9cc2` |
| `user-prompt-submit.json` | `6a10a9f75c19720b0863d11d7d7d80f190e8eacd` |
| `post-tool-use.json` | `f92af1dd3249ddf3511ef707c4310329e5b85c80` |
| `permission-request.json` | `9ee8996db169d55b0da44d7efbb57af76676c2fe` |
| `stop.json` | `dbd4a3f648071bee1af24758e9596e11ab78c782` |
| `interrupt.json` | `e0ac2a96bb719511d157dd787796e2466524a1fd` |

`tests/hooks.rs` reuses those exact schemas and historical payloads. Schema identity
does not prove runtime delivery; `codex_trusts_every_hook_a_launch_subscribes`
separately checks native acceptance of every launch-scoped trust hash.

## Native observations

Approved executable `/home/theo/.local/npm/bin/codex` reported `codex-cli 0.166.0`.
Its launcher SHA-256 was
`e14fc83739c13c1074b12bd1acce4010c893c7f6fbfc1593b4a31d74cabb57af`;
this is not the underlying native payload digest. Each native server startup
checks its version again. Native evidence was collected on Linux, not WSL2.

[conformance.rs](../../conformance.rs) exercises Resume, Events, Submit, Cancel
and Outcome with scratch homes, cleared child environments, no credentials,
private tmux and local canned Responses traffic. It observes prompt reports,
steer/cancel, failed and completed outcomes, and a new prompt/completed outcome
on the same conversation after launcher resume.

Host receipt `2618146fdd94`: 16 passed, resume failed at the default-keymap guard,
and one terminal-launch test was not run. Diagnostic `e573b4016fd2` confirmed
object/null-only default bindings. After ordinary RED/GREEN coverage and the
guard repair, receipt `2727d19ab396` passed all three affected native checks:
real remapping/unbinding refusal, resume and terminal launch. This is cumulative
evidence, not one all-green native run. Failed receipts remain part of the record.

Final receipt `e2dc023b35a9`: **35 passed, 1 skipped**, exit 0, covering all
affected ordinary/native checks; the one exclusion is signed-in routing.
Earlier final-run attempt `c2e57f1cba60` failed on interleaved shell report writes.
Direct native servers now use the launcher's report ledger, and incomplete
records are hidden until their delimiter arrives. Independent review also found
that tmux hangup alone could leave descendants alive; a retained pane group now
receives bounded TERM/KILL cleanup with Linux `/proc` verification. Both review
regressions failed in `d13bcb90ec65`, then passed in `7e8ffee9e9a6` and the final
run. Full ordinary Codex tests: 137 passed / 21 skipped; daemon consumers:
10 passed / 0 skipped. Final focused Clippy and Rustdoc passed.

[approval_routing.rs](../../approval_routing.rs) additionally observes delivered
start/steer approvals on the persistent TUI after delivery disconnects, followed
by TUI cancellation and the matching interrupted turn. Its signed-in test needs
separate approval; these account-free observations do not complete #436 or #419.
