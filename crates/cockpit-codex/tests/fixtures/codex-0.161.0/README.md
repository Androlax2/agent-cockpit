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
