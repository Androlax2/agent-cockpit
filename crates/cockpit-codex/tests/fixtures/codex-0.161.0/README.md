# Codex 0.161.0 hook payloads

One payload per case the adapter normalizes, as Codex 0.161.0 writes it on a hook's standard
input, and `schemas/`: Codex's own hook input schemas for the subscribed events, copied from
`codex-rs/hooks/schema/generated/` at tag `rust-v0.161.0`.

Captured live on 2026-11-01 (#48) with a fresh `CODEX_HOME` and a placeholder API key, so
every turn failed at the model call after its hooks ran: `session-start.json`,
`user-prompt-submit.json` (app-server turn), `session-start-resume.json` (`codex resume` in
tmux), `interrupt.json` and `session-start-clear.json` (Escape, then `/clear`, in tmux). Only
the home and working directories were renamed (`/home/owner`, `/work`).

`stop.json`, `post-tool-use.json`, `post-tool-use-subagent.json`, `permission-request.json`
and `permission-request-subagent.json` need a model reply, which that home cannot get: they
are written from the pinned schemas, and `hooks::every_pinned_payload_matches_its_pinned_input_schema`
checks every payload against them.

`pre-compact.json`, `post-compact.json` and `session-start-compact.json` (#782) are written from
the pinned schemas, not captured: a compaction needs a model reply. The two compaction
schemas in `schemas/` follow Codex's `pre-compact.command.input` and
`post-compact.command.input` at tag `rust-v0.166.0` (required `cwd`, `hook_event_name`,
`model`, `session_id`, `transcript_path`, `trigger`, `turn_id`; optional `agent_id`,
`agent_type`; no other property), read through a summary, so their blob ids were not compared
with the 0.166.0 table. `rollout-compaction.jsonl` is a made-up rollout tail with the records
the adapter reads (`token_count` events and a `compacted` record; field names from report
686's measurement appendix).
