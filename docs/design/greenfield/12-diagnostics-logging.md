# 12. Diagnostics and logging

Status: decided

## Decision

`cockpit-diagnostics` provides leveled logging for **Cockpit's own operation**, runtime level
changes, bounded log files and a shareable support bundle.

## Scope rule (decided 2026-10-31)

- Logged: Cockpit decisions, state transitions, IPC, tmux/provider integration results,
  Cockpit's own paths (config, database, sockets, log files) and the error text of Cockpit's
  own operations.
- Never logged: prompts, agent output, repository contents, commands the agent runs,
  credentials, environment values. Hosted work is not Cockpit's to log.
- This relaxes the 2026-10-29 diagnostics spec only for Cockpit-internal paths and error text;
  its storage budgets, host-only control and separation from the security audit stay.

## Levels

| Level | Records |
| --- | --- |
| `error` | A failure needing attention |
| `warn` | Degraded or recovered behavior: retry, fallback, dropped log lines |
| `info` | Lifecycle: task assigned, session started, approval decided, recovery after restart |
| `debug` | Decisions and reasons: scheduling choice, admission math, capability negotiation |
| `trace` | Wire-level and per-event detail; time-boxed only |

`COCKPIT_LOG_LEVEL` accepts a level or per-component directives (`scheduling=debug,tmux=trace`)
in `EnvFilter` syntax. A dedicated variable avoids inheriting unrelated `RUST_LOG`.

## Runtime control

`cockpit logs level <level> [--for 15m]` reloads the running daemon's filter and reverts
automatically (default 5 min, hard maximum 15 min, resets on restart). Control goes through
the host-only channel; agents cannot raise levels or read logs. What agents need ("why
blocked") is a product API answer, not log mining.

## Files

- One `cockpit.log` per long-lived process under `$XDG_STATE_HOME/cockpit/logs/`, 0600,
  size-rotated with a total cap (reuse the 2026-10-29 budgets: 4 × 4 MiB, seven days).
- Non-blocking writer thread with a **visible drop counter**; a stalled disk never stalls
  input handling. Shutdown drain is bounded.
- Every record carries task / attempt / session / request ids so one problem can be followed
  across daemon, TUI and tmux integration. Structured (JSON lines) on disk, pretty in `--foreground`.

## Support bundle

`cockpit support-bundle [--since …]` writes one size-bounded archive: logs for the window,
version manifest (installed, running daemon, session helper bundles), readiness and
capability report, redacted config, schema version, platform facts (OS, WSL, tmux, provider
versions). It lists contents before writing.

## Rejected

- Routing high-frequency events through the security audit log.
- Content-free `debug` (spec 2026-10-29 as written): too little to debug a distributed product.

## Sources

- `EnvFilter` directives: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html>
- Runtime reload: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/reload/index.html>
- Non-blocking appender and drop counter: <https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/index.html>
- Prior spec: `docs/superpowers/specs/2026-10-29-bounded-diagnostic-logging-design.md`
  (approved 2026-10-29 in another session; not yet on `main`)
