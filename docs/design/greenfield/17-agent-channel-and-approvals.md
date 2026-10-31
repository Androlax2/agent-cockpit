# 17. Agent channel, approvals and host actions

Status: proposed

## Decision

All agent-to-Cockpit traffic goes through **one authenticated channel**: a Unix socket the
sandbox can reach, speaking `cockpit-protocol`, with the sender identified by kernel peer
credentials plus a one-time launch token. It replaces today's report socket, team socket,
todo-update file queue and per-command socat calls.

## Identity

- At launch, `cockpit-sessions` mints a launch token and passes it to the agent's bridge
  (hooks, plugin, Codex bridge). The first report binds token → session → verified process
  tree; later reports are matched by peer credentials against that tree.
- Payload claims (ids, paths, "I am session X") never identify the sender.
- Each session gets a **task-scoped grant set**; `cockpit-authorization` evaluates every request
  against it. Nested agents (subagents) inherit the parent's session identity and grants.

## Agent verbs (CLI over the channel)

`cockpit agent report` (state, label, doing, needs-input), `cockpit todo …`,
`cockpit task …` (packet, checkpoint, handoff), `cockpit ask run`, `cockpit ask guide`,
`cockpit evidence …` (bounded source/CI/runtime reads), `cockpit message …` (team/Chief).
Each verb returns a typed result or a typed refusal with a reason the agent can act on.
Bridges for Claude hooks, the OpenCode plugin and the Codex bridge translate native events
into the same `report` verb; no shell+socat plumbing in the product.

## Approvals (`ask run`)

1. The agent submits command, working directory and reason. The daemon stores its own copy
   (never re-reads the sandbox's) and shows it in the TUI approval pane under the asking
   session, and on the phone client if paired.
2. Display rules stay: exact text, hidden characters escaped, non-ASCII flagged, working
   directory outside the project in red, Decline default, first keystrokes ignored.
3. Approval binds the command digest, working directory and request revision; it is consumed
   once with the effect intent (report 04). Editing creates a new revision.
4. Execution runs on the host in a PTY with hooks disabled and safe Git settings; output
   returns with secrets redacted, bounded, through the per-project **read-only answers
   directory** (host-written, mounted read-only into that project's sandbox only).
5. Exit codes keep their meaning: 125 declined, 124 pending (`--wait <id>`), 126 unreachable
   (with the fix), 3 stuck (guide). `--check` at session start stays.
6. Queue: one approval asks at a time per owner; approved commands keep running while the next
   asks; a command needing input comes back to the front.

## Guides (`ask guide`)

Steps on stdin (`#`, `##`, `open:`, `copy:`, `ask:`, `run:` via `ask run`), shown one at a
time in the TUI (and phone), answers and stuck-step notes returned the same way. Floating
desktop windows are an optional desktop-client rendering (19), not the only path.

## Host-reviewed installs

`.handoff/` with `SHA256SUMS` and `cockpit handoff-install` (diff per file, y/N, refuses setuid
and symlinked destinations) and `cockpit system-install` for `/etc` stay as CLI host actions.
They are owner-only commands; agents only produce the handoff files.

## Rules

- A refusal is a first-class answer: reason code, what would unblock it, who owns that.
- Nothing outside the sandbox runs Git in a session's folder except through the reviewed
  push/apply paths with hooks off and program-running settings reset.
- The channel is also the plugin boundary later (report 14): same auth, same grants.

## Sources

- Current boundaries: `README.md` "Out of the sandbox, with my approval" and the 2026-10-19
  security audit items H3/M3/M4/M5 referenced in `apps/crates/cockpit-core/src/approval.rs`.
