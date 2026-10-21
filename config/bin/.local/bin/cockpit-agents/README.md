# Agents

The cockpit runs sessions of several agent CLIs (Claude Code, opencode, Codex CLI). An agent needs four
pieces. `apps/cockpit/tests/agents.rs` checks that each piece exists for every agent.

1. **A file here, `<name>.sh`**: what `cockpit` and `cockpit-left` do with its
   sessions. `cockpit` sources every file in this folder and refuses to run if a file is
   missing a verb.
2. **A row in `apps/cockpit/src/agents.rs`**: what the column, the phone page and the OSD daemon
   need to know (label, badge, ids, features, start files).
3. **A greywall launcher and profile**: `greywall-agents/<name>` (sourcing
   `launcher-common.sh`) and `config/greywall/.config/greywall/learned/<name>.json`.
4. **A report bridge**: anything that runs `~/.claude/hooks/session-state.sh` with
   `COCKPIT_AGENT=<name>` and Claude-hook-shaped payloads (`SessionStart`,
   `UserPromptSubmit`, `PostToolUse`, `Notification`, `Stop`, `SessionEnd`). Claude Code has its
   hooks; opencode has `config/opencode/.config/opencode/plugins/cockpit.ts`; Codex has
   `cockpit-codex-hook` (`config/codex/.codex/hooks.json`), which reports namespaced `cdx_` ids.

Linked sessions add a fifth piece: a way to take an approved team message natively, never by
typing into the agent's screen (Claude: the background hook `cockpit-team watch` its launcher
adds; opencode: its plugin's `session.synthetic`; Codex: its Stop hook). An agent without one
refuses `say`.

## The verbs

Every name a file defines, verbs and helpers alike, carries the agent's name: the files share one
namespace. Each verb is `agent_<name>_<verb>`:

| verb | arguments | does |
|---|---|---|
| `label` | | prints its name for messages ("Claude Code") |
| `is_session_id` | id | true for an id it reports |
| `tmux_prefix` | | prints the prefix of its tmux sessions' names (the longest matching prefix wins) |
| `can` | feature | true if it has `effort`, `billing`, `compact`, `remote` or `redraw` |
| `new` | dir [args] | starts a session in the viewer pane |
| `new_with_prompt` | dir prompt | the same, with a first message |
| `background` | dir prompt | starts one out of sight (the error hand-off, `h` on a Todo item) |
| `detached` | dir prompt | starts one out of sight in a clone (`N`, `+`, the phone) |
| `resume` | dir id [effort] | brings a past session back in the viewer pane |
| `restart` | id dir [effort] [detached] [billing] | exits it and brings it back |
| `restore` | dir id | brings it back out of sight after a reboot |
| `exit` | tmux-session pane id | ends it, killing its tmux session if it stays |
| `say` | id pane message | types a one-line message into it, or refuses |
| `prompt_start` | | prints the regex of what is left of the cursor at the start of an empty prompt (Left) |

Optional: `doctor`, its section of `cockpit doctor` (`report ok|fail what [fix]`).

The manager's helpers are there when a verb runs: `view_session`, `label_panes`,
`detached_size`, `submit_to_pane`, `pane_at_prompt`, `claude_left` (waits for the pane to end),
`session_field`, `audit_event`, `log_event`, `require_plain_directory`. Prompts that start with `-`
come in as `Task: …`.
