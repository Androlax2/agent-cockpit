# Claude Code 2.1.319 captures

Captured on 2026-11-09 for #718, not the pinned `CONFORMANCE_VERSION`: a turn whose API
connection was refused. Claude Code 2.1.319 ran `-p "Reply ok"` in an empty home with a
made-up API key and `ANTHROPIC_BASE_URL=http://127.0.0.1:9`, a closed port, with
`CLAUDE_CODE_MAX_RETRIES=0`; a `StopFailure` hook saved its payload and copied the transcript
as it ran, so the transcript is what the bridge reads at that moment.

- `stop-failure-connection-failed.json` is the payload as written; only the working
  directory (`/work`) and the transcript path were renamed.
- `transcript-connection-failed.jsonl` is that transcript's tail, with the user's prompt, Claude
  Code's own API error (`isApiErrorMessage`, `apiError: "connection_failed"`, model
  `<synthetic>`) and the bookkeeping records written after it (`last-prompt`, `atis-latch`,
  `cost-state`), which are no message. The queue, attachment and API-request records before the
  error were left out; the working directory was renamed.

The payload's `error` is `server_error`, the value 2.1.319 also gives a server's 5xx and a
timed-out request; only the transcript's `apiError` names the lost connection.
