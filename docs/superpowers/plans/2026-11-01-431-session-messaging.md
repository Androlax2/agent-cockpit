# Session messaging implementation plan

> **For agentic workers:** Use `executing-plans` for inline implementation; the issue authorizes execution and one fresh independent branch review.

**Goal:** Deliver owner send/cancel through the provider, falling back only on unsupported delivery.

**Architecture:** Sessions own action validation, launch context and serialization. The daemon supplies verified socket connections and owner routes. CLI and protocol carry the two verbs.

**Tech Stack:** Existing Rust workspace, Turso, Tokio and tmux; no new dependencies.

**Spec:** [#431](https://github.com/Androlax2/agent-cockpit/issues/431), including the remaining daemon proof in [#450](https://github.com/Androlax2/agent-cockpit/issues/450).

## Constraints and review focus

- Preserve provider adapter ownership, authenticated owner routing and listener verification.
- Never retry uncertain delivery as terminal input or as a fresh scheduled message.
- Use the recorded launch mapping after config changes and daemon restart.
- Refuse unknown conversations, missing capabilities, stopped sessions and open dialogs.
- Keep the session lock until blocking delivery actually finishes, including after timeout.

## Tasks

1. Delivery contract and launch context
   - [x] Test explicit unsupported delivery and persisted socket mappings.
   - [x] Add `Delivered::Unsupported`; keep provider failures distinct.
   - [x] Persist the launch plan's reach in sessions; record the same verified process root in that launch context and the channel in one transaction.
   - [x] Retain the existing sessions/provider dependency edges and guard against importing daemon socket code into sessions.
2. Sessions, daemon and CLI
   - [x] Add daemon entry tests for both transports, send/cancel, refusal and uncertain outcomes.
   - [x] Extend `Sessions::submit`, add `Sessions::cancel`, and inject the daemon socket connector.
   - [x] Add owner-only protocol verbs and CLI parsing/output; preserve coordinator and scheduler semantics.
3. Mapped Codex proof and completion
   - [x] Drive the real Codex adapter through the daemon with a fake sandbox and app-server, including missing mount support and daemon restart.
   - [x] Update coverage and record the launch-context decision.
   - [x] Run focused nextest checks with two test threads, crate graph, formatting and affected lint checks.
   - [ ] Commit, rebase onto main, obtain one fresh full-delta review, fix actionable findings and verify.
   - [ ] Push this branch, open the PR, arm auto-merge, resolve CI/conflicts, and report merge on #431.
