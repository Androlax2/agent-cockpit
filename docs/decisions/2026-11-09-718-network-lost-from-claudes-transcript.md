# A turn the network ended is `network-lost`, read from Claude Code's transcript

Status: accepted

Issue #718. Decided by the worker from the harnesses' own reports; the owner chose how the
daemon notices the network is back (A, 2026-11-09).

- **A new outcome, `network-lost`, always with the harness's own word** (`reason`, required on
  the wire, at `--outcome-reason` and in `session_reports`). It is never inferred from a
  generic failure: an adapter reports it only where the harness tags the cause itself.
- **Claude Code: the transcript's `apiError`, not the hook.** The `StopFailure` hook of 2.1.316
  and 2.1.319 gives a lost connection `error: "server_error"`, as it does a 5xx or a timed-out
  request, with no `error_details`. The API-error message Claude Code writes to the transcript
  carries `apiError: "connection_failed"` for an unresolvable, unreachable or refused host and
  a dropped connection, after its own retries (ten by default). The bridge reads it at
  `StopFailure` only when it is the turn's last main-chain message; a later prompt, another
  `apiError` or an unreadable transcript keeps `failed`. Evidence: a 2.1.319 capture against a
  closed port (`crates/cockpit-claude/tests/fixtures/claude-code-2.1.319/`), and the owner's
  own 2.1.316 and 2.1.317 transcripts, whose eight `connection_failed` records carry the same
  fields. `apiError` is not a documented hook field: a release that drops it leaves such turns
  `failed`, never wrongly resumed.
- **Codex: nothing to classify.** 0.166.0 does not end a turn when the network is gone: after
  its WebSocket retries it falls back to HTTP and waits without a limit (81 minutes seen in
  `~/.codex/logs_2.sqlite`), then goes on by itself. `httpConnectionFailed` without a status is
  the only plausible value and was never seen on a failed turn, so it is not mapped.
- **OpenCode: not classified.** 2.2.5 ends such a turn with `provider.transport`, which also
  covers dropped streams, WebSocket closes and timeouts; the socket error code survives only
  in the message text. The pinned 1.19.5 could not be checked here.
- **No protocol version.** `ProtocolVersion` is for changes an older peer would misread; an
  older peer refuses the unknown outcome kind instead, and helpers install as one bundle
  with the daemon.
- **Until the resume (#718 part 2), a `network-lost` turn rings and notifies as a failure**,
  with "the network was lost (connection_failed)": nothing continues it yet.
- **Rejected:** matching `last_assistant_message` ("API Error: Can't reach the API server…",
  display text that changes between releases and modes); counting repeated `server_error`
  failures, which would resume a server's real failure.
