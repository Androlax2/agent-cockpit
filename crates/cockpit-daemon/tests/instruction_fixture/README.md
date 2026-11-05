# Native instruction delivery (#419)

`native_instructions.rs` launches an owner `SessionNew` through the real daemon.
The native harness receives the real hook/plugin subscription, instruction skill
directory, MCP server and authenticated agent channel. The first prompt is only
`Reply with only OK.` A local model fixture records the request and returns canned
text. It never emits a tool call or approval response.

On Linux, the ordinary test runs thirteen Python standard-library regressions for external
configuration refusal, credential isolation, descendant cleanup and false
instruction evidence and model shutdown; three Rust regressions cover failed capture,
cleanup I/O and final model exit. The three native tests are ignored by default and currently
support Linux only. They require explicit paths to already approved binaries;
they never install or download anything.

## Recorded evidence

The receipts in `captured/` were produced on 2026-11-02. They contain native version
output, the selected program's SHA-256, fixture source SHA-256, request SHA-256 and
the assertions that passed. Session IDs, temporary paths, request bodies and
credentials are not exported. Codex's program digest identifies its npm JavaScript
launcher, not the native executable it selects.

These receipts retain the pre-drain-fix fixture hash and are historical evidence.
They have not been regenerated for the shutdown follow-through; local fake-process
regressions do not substitute for a new exact-version native run.

| Harness | Target pin | Binary used | Native evidence |
| --- | --- | --- | --- |
| Claude Code | 2.1.316 | 2.1.316 | Print mode: brief and native `cockpit` skill in a `messages` system entry; Cockpit MCP tools advertised for `ToolSearch` |
| Codex | 0.161.0 | 0.166.0 | Actual TUI/app-server: brief in developer input and MCP tool schemas; no native Cockpit skill |
| OpenCode | 1.19.5 | 1.19.5 approved extraction | Actual TUI: brief and native `cockpit` skill in system input, plus MCP tool schemas |

The installed `/usr/bin/opencode` is 2.1.4; it was not substituted for the target.
The approved 1.19.5 archive and executable provenance are recorded in
[the #417 capture documentation](../../../cockpit-opencode/tests/fixtures/opencode-1.19.5/README.md).
Production version pins are unchanged. Codex 0.166.0 evidence does not validate
0.161.0.

## Running

Build the real sibling programs first:

```sh
CARGO_BUILD_JOBS=2 cargo build --locked --offline -j 2 \
  -p cockpit-cli -p cockpit-daemon -p cockpit-claude -p cockpit-codex -p cockpit-opencode
```

Set `COCKPIT_CLAUDE_PROGRAM`, `COCKPIT_CODEX_PROGRAM` and
`COCKPIT_OPENCODE_PROGRAM` to the explicitly approved absolute paths. An optional
`COCKPIT_INSTRUCTION_CAPTURE_DIR` receives the three small receipts. Then run:

```sh
CARGO_BUILD_JOBS=2 cargo nextest run --locked --offline \
  -p cockpit-daemon --test native_instructions -j 2 --run-ignored only --no-capture
```

`--no-capture` serializes these native probes. Ordinary checks use:

```sh
CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 cargo nextest run --locked --offline \
  -p cockpit-daemon --test native_instructions -j 2
```

## Isolation and limits

Each probe has fresh HOME/XDG/config/cache/state directories, a clean fixture Git
project and a private tmux server. Git setup clears the caller’s environment and
disables user/system configuration, hooks, templates and signing; the shared
fixture’s older setup remains tracked separately in #526. Native wrappers preserve only the daemon's
launch identity and explicit terminal/provider launch settings. They replace model
transport with loopback fixtures and suppress updates, telemetry and external
provider discovery. Managed/system policy and instruction-bearing parent paths
are refused using metadata only. Real authentication discovery is intercepted as
signed out. No auth file is copied or read by the fixture.

OpenCode's bundled compatible provider avoids dependency downloads. A temporary
read-only global config directory prevents 1.19.5's automatic plugin dependency
installation; Rust restores permissions even on test failure. Child groups retain
their waitable leader until killed. Native commands have 3-second version (inside discovery’s 10-second deadline) and
50-second execution limits; requests are limited to eight, and bodies/captures to
2 MiB. Cooperative teardown works even after daemon failure. Before publishing a
receipt, the Rust owner stops native wrappers and asks the model server to drain.
The server joins active HTTP handlers before exiting; Rust requires successful
exit within ten seconds and checks the final error record. Timeout or failed
shutdown fails the probe instead of publishing a receipt.

The daemon fixture runs its agents unsandboxed, the default `agents.sandbox = "off"`. Application configuration controls are not proof of an OS network boundary or real sandbox
mount behavior. These tests prove delivery into native model input, not model
understanding, approval routing or persistent completion (#516).

**#419 stays open:** no signed-in model has listed the skill or identified its
brief in this run; Claude TUI parity is also unmeasured. A real run needs a separate
concrete `ask-to-run` approval naming the account, model usage and host effects.
It must preserve the owner's authentication in place, audit effective config and
avoid inheriting unreviewed hooks/plugins/MCP servers. This fixture has no live
mode and cannot be switched to an account by changing one environment variable.
The harness's total context remains #479, not #419.
