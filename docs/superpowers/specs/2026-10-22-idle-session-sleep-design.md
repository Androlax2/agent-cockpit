# Idle session sleep — design

Todo: c17a056c

## Why

Sessions left at their prompt for hours keep their whole process tree alive. On 2026-10-22, ten
opencode sessions idle for ~9 h held 270–290 MB each, plus ~15 sandbox helpers per session
(socat, bwrap, greywall, xdg-dbus-proxy): ~3 GB, and the machine sat in 5 GB of swap. Freezing
(cgroup freeze) would not help: a frozen process keeps its memory. Only ending the process gives
it back.

Theo does come back to these sessions and picks up the same conversation, so ending them must
not lose the conversation or the card.

## What it does

A session that has sat **ready at its prompt for 1 hour** is put to sleep: the cockpit exits its
process (and with it the sandbox), keeps its card, marked asleep, and resumes the same
conversation when Theo or the cockpit needs it again.

### Goals

- Give the RAM of long-idle sessions back without Theo doing anything.
- Waking is invisible apart from a few seconds' wait: same conversation, same card, same place.
- Works for Claude and opencode sessions alike.

### Non-goals

- ~~No manual "sleep now" key~~: added later at Theo's request: `s` sleeps the selected session at once,
  skipping the idle delay but keeping the typed-text, background-work, conversation and team checks.
- No freezing / thawing.
- Waking does not need to be fast; a few seconds of resume is accepted.
- Waking with the current CLAUDE.md, settings and sandbox (not the ones the session started
  with) is accepted; nothing special is shown for it.

### Anti-goals (a failure even if it technically works)

1. **Lost typing**: a session with text in its input box is put to sleep and the draft is gone.
2. **Lost background work**: a dev server, watcher or background shell started from the session
   is killed by the sleep.
3. **Asleep at the wrong moment**: a team message, queued message or queued todo meant for the
   session is dropped or stuck because it was asleep.
4. **A sleep that looks like an exit**: the session shows up in `ended-sessions`, disappears from
   the list, or is not offered/kept across a reboot.

## When a session falls asleep

All of these must hold; any doubt means "stay awake" (fail safe toward keeping the session):

- The setting `idle-sleep` is on (default on).
- The card has been "Ready" (`activity_label == "Ready"`, i.e. state `waiting`, no explicit
  question, no approval or input request, no running command) for at least
  **`idle-sleep-after` minutes, default 60**, measured from `Session.since`.
- It is a managed session: not `terminal` (started with `t`), not external.
- Pinned sessions sleep too (Theo's choice, after the first version kept them awake).
- It is not the session on screen in workspace 6.
- It is not a member of a team (team bindings are tied to the running process: `osd/teams.rs`
  `open_and_restore` and `teams/store.rs` refuse a changed runtime, so a resumed teammate would
  lose its team).
- No message is queued for it (`prompts::count` is zero) and no todo is assigned to it.
- Its input box is empty, checked on the pane right before the exit, the same way
  `cockpit-left` checks for a draft (cursor line against `agent_<name>_prompt_start`). If the
  check cannot decide, the session stays awake.
- It has no background work: no processes in its pane's process tree beyond the agent itself and
  its sandbox helpers, and Claude's `background` / `agents` counts are zero. Builder's choice how
  to tell "the agent and its sandbox" from "something it started" (an allowlist of the known
  helper names is fine); when unsure, stay awake.
- Its conversation can be resumed (`has_conversation` for Claude; a session id for opencode).
  A session that would come back empty never sleeps.

The check runs in the cockpit's existing periodic loop (alongside the todo queue dispatch); once a
minute is enough.

## Putting it to sleep

- Reuse the agent's own exit path (`agent_*_exit` / `exit_session`), **not** `cockpit exit`:
  a sleep must not append the id to `ended-sessions`.
- Before exiting, write a durable sleep note in `~/.local/state/cockpit/asleep/<session-id>`
  holding what a resume needs: agent, conversation/session id, project, cwd, effort, billing
  (`credits`), role, pinned flag, the time it fell asleep. State dir, not `$XDG_RUNTIME_DIR`:
  a sleeping session must survive a reboot and a cockpit reload.
- If the exit fails or a check fails at the last moment, delete the note and leave the session
  alone; log why.
- Log each sleep and wake with `log_event` (session, project, idle time).

## While asleep

- The card stays in its place in the list, with the project and title it had, its activity badge
  reading `Asleep 9h` (time since it fell asleep), dimmed like the `Idle` badge.
- It counts as neither needing Theo nor working. It does not count toward the "started before a
  change" warning.
- After a reboot, asleep sessions stay asleep cards; they are not mixed into the `B` restore
  offer (they are restored lazily, on wake).
- **Expiry** (Theo's choice): after 7 days asleep, the sleep note is deleted and the card
  disappears, without marking the session ended in a way that hides it from `H` (past sessions),
  where it can still be resumed. Log it.
- A wake that fails because the conversation is gone (deleted transcript, missing opencode
  session) deletes the note and says so on the card / in the log; it never starts a fresh,
  empty session in its place.

## Waking it

Wake = resume the same conversation through the existing resume path, detached unless Theo is
opening it (Claude: `claude --resume <id>` with its effort and billing; opencode:
`opencode --standalone --session <id>`), then delete the sleep note. The card then follows the
existing `restarting` → replacement flow.

Everything that reaches a session wakes it first, then proceeds as today:

- **Opening it** (Enter, `view`, the continuity view): wake, then open it once it is back.
- **`cockpit say`** — the single funnel for queued messages, the todo queue, `h` hand-to-Claude,
  the phone and the error hand-off: if the target is asleep, wake it, wait until it is at its
  prompt, then deliver. A delivery that cannot happen (wake failed) must surface as it would for
  a dead session today, never be dropped silently.
- **The todo queue** treats an asleep session of the project as eligible (it has been idle long
  enough by definition), wakes it and hands it the item.
- **The phone page** lists asleep sessions with an asleep state, and can wake one (explicit
  wake action) or message it (wake, then deliver). Today `remote.rs` only knows sessions from the
  snapshot and refuses others ("no such session"); it must also read the sleep notes.
- **Remote Control** (Theo's choice): a session with Remote Control on sleeps like any other.
  While asleep it shows offline in the Claude app; Theo wakes it from the cockpit (column or
  phone page). The sleep note records that Remote Control was on, and a wake turns it back on
  (`/rc`) once the session is at its prompt.
- **`r` restart** on an asleep card = wake. **`x` exit** on an asleep card = delete the note and
  mark it ended, as an exit does today.

## Settings

- `idle-sleep`: on/off, shown in the `S` menu ("Sleep idle sessions"), default on, also
  `cockpit setting idle-sleep on|off`.
- `idle-sleep-after`: minutes, default 60, minimum 30 (cards already read `Idle` after 30
  minutes; sleeping should come after that). Stored as a string setting like the launch settings,
  with its own validator in `cockpit`. Shown in the `S` menu next to the switch.

## Left to the builder

- Exact module and file layout, names of helpers.
- How to recognise sandbox helper processes versus background work.
- The wait/timeout for "back at its prompt" before delivering after a wake (reuse what restart
  already waits for).
- Test layout, following `apps/cockpit/tests` fake-command style; each anti-goal above needs a
  regression test (draft kept, background process kept, queued message delivered after wake,
  not in `ended-sessions`, survives reload).

## Calls made without Theo (check these)

- Team members, the session on screen and terminal sessions never sleep (pinned ones do, by Theo's later choice).
- Sleep notes live in `~/.local/state/cockpit/asleep/` and survive reboots; asleep cards are not
  part of the `B` restore offer.
- `x` on an asleep card ends it for good; `r` wakes it.
- Minimum of 30 minutes for `idle-sleep-after`.
- Codex sessions never sleep (their draft check cannot decide yet).
- Moving the selection onto an asleep card does not wake it; only opening, messaging, a todo,
  `r`, or the phone's wake does.
- The clone sweep (`K`) keeps clones that have an asleep session.
