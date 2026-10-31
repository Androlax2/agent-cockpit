# cockpit-diagnostics

Owns logging of Cockpit's own operation (levels, time-boxed runtime overrides, bounded log
files, the support bundle); never hosted work: prompts, agent output, commands the agent
runs, credentials and environment values are not Cockpit's to log
([report 12](../../docs/design/greenfield/12-diagnostics-logging.md)).

Dependency edges: no other Cockpit crate. Each long-lived process (`cockpit-daemon`, the TUI)
calls `start` once and installs its dispatch; the daemon routes the host-only
`cockpit logs level` and `cockpit support-bundle` requests to `LevelControl` and
`plan_bundle`. Agents never reach either: authorization stays with the daemon's host-only
channel.

- `COCKPIT_LOG_LEVEL`: a level or `component=level` list; `tmux=trace` filters `cockpit_tmux`.
  An unreadable value is an error, never a silent default.
- Overrides: 5 minutes by default, 15 at most, reverted by a timer; nothing persists across
  a restart.
- `$XDG_STATE_HOME/cockpit/logs/<process>/cockpit.log`, JSON lines, 0600 in a 0700
  directory, 4 files of 4 MiB, rotated files removed after seven days (checked hourly). A
  second process with the same name fails to start: `cockpit.log.lock` is held locked.
- Level changes are written straight to the file, so an override cannot hide its own record.
- A bounded queue and one writer thread: a stalled disk drops records, counts them
  (`dropped_records`) and writes a `dropped log records` warning once writing resumes.
- Span fields (`task_id`, `attempt_id`, `session_id`, `request_id`) appear on every record
  inside the span.
- Records or spans with a hosted-content field name (`prompt`, `shell_command`, `env`, …)
  are refused and reported by callsite only.
