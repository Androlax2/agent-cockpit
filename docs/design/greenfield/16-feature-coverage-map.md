# 16. Feature coverage map

Status: proposed. Longer than the 120-line rule on purpose: this is the checklist that proves
every current feature has an owner in the greenfield design, or a reason to drop it.

Inventory sources: `README.md`, `docs/*.md`, `apps/**` (Rust), `config/bin/**`, hooks,
plugins, systemd units, `Makefile`, `tools/**` on `main` (2026-10-31).
Verdicts: **keep** (same behavior, new owner), **change** (behavior simplified or moved),
**polish** (deferred to the designed-UI milestone), **drop** (not a product feature),
**plugin** (first candidates for report 14).

## Sessions and providers

| Feature | Verdict | Owner (report) |
| --- | --- | --- |
| Start session (blank, with prompt, in own clone), background/detached, terminal-in-place | keep | sessions, projects, tmux (05) |
| Restart in place with same conversation; restart-all after config change (17 start files) | keep | sessions; config change detection in config (09) |
| Sleep after idle, wake on open/message/todo/phone, sleep refusals, asleep notes 7 d | keep | sessions; permits via messaging (07) |
| Reboot restore (`B`), ended/recent sessions, resume past session (`H`) | keep | sessions |
| Effort, model/variant pickers, billing toggle (credits vs subscription), `/compact`, Remote Control | keep | provider adapters expose as capabilities (08); UI generic |
| Stuck-exit capture, forced exit rounds | keep | processes + diagnostics (12) |
| Agent state from hooks/plugins/Codex bridge; pane scraping fallback (`osd classify`) | change | adapters own detection; structured events first, screen classification as adapter-internal fallback with recorded confidence; never task completion (05) |
| Per-agent adapter verbs (`new resume restart exit say prompt_start doctor …`) | keep | `cockpit-provider-api` trait + one bridge per adapter (08, 17) |
| Subscription fallback/routing between accounts (routes.json, gates in hooks, native evidence) | change | account routing at launch and at quota-stop only; no mid-turn gates (18) |
| Native Codex thread ids, OpenCode standalone server + plugin, Claude hooks/statusline | keep | inside each adapter (08) |
| Claude/OpenCode auto-update timers | change | adapter `update` capability run by `cockpit doctor --fix`/timer (13) |

## Terminal, navigation, viewer

| Feature | Verdict | Owner |
| --- | --- | --- |
| Manager tmux session, column + viewer pane, nested attach per agent session | keep | tmux on a private server (05) |
| Focus fade (`cockpit-fade`), Left-at-prompt-start (`cockpit-left`), kitty chords, loading/restarting screens | polish | tui (05) |
| Workspace 6 kitty window toggle, Hyprland/waybar signals, floating approval/guide windows | change | desktop integration client (19); approvals inside tmux panes by default |
| Image viewer (`I`), reply images | keep | sessions artifacts; renderer in tui/web |
| Contextual help `?`, card legend | keep | tui |

## Tasks, todo, queue

| Feature | Verdict | Owner |
| --- | --- | --- |
| Items, priorities, kinds, statuses, notes with images, drafts surviving reload | keep | tasks (06) |
| Proposals/Ideas (`y`/`n`), declined memory | keep | tasks: `proposed` status + decline memory |
| `blocked` reports to the dotfiles project; `stack` proposals to greywall/greyproxy | change | generic "report blocker to configured target project" (owner setting); stack = plain cross-project proposal |
| `--after` dependencies, `unwait`, completion summaries to the filing session | keep | tasks + messaging |
| Cross-project `add --project`, caps per project | keep | tasks; caps in config |
| Queue dispatch (idle 5 min, priority, viewed-session exclusion, designer routing), `Q` explanation | keep | scheduling (10) with explanations |
| Todo trailers closing items on apply; task trail; updates/reviewed; return notes; defer 20 min | keep | tasks + verification evidence (06); notes/defer in tui state |
| GitHub issues shown with todos; PR list; review session in fresh clone | keep | `cockpit-github` evidence source + projects |

## Approvals, host actions, agent channel

| Feature | Verdict | Owner |
| --- | --- | --- |
| `ask-to-run` (fish parse, approval pane, PTY run, redaction, phone answer), `ask-to-guide`, `--check` | keep | approvals + processes (17) |
| `cockpit-needs-input`, `session-label`, `session-doing`, todo CLI over the report socket | keep | one agent channel (17) |
| Per-project read-only answers dir, kernel peer identity, request copies host-side | keep | agent channel (17, 11) |
| `.handoff/` + `make handoff-install`, `make system` with diffs | keep | CLI host actions with review (17) |
| Security audit log (hash chain, verify, retention) | keep | `cockpit-audit` (11) |

## Teams, Chief, session links

| Feature | Verdict | Owner |
| --- | --- | --- |
| Roles, link approval, recruit, QA fresh copy, handoffs, clarifications, escalation | change | thin behaviors over tasks/workflows/messaging (07) |
| Session links (`link-propose/send/assess`, per-TODO budgets) | change | folded into messaging (07) |
| Chief: pick/pause/resume, answer stopped agents with habits, escalations, overrule, start_session, give_todo, inspect, supervised tasks, digest | change | judgment only; one wake path; habits kept (07) |
| Chief stop bundle (masked last messages, open form) | keep | agent-context assembles it (08) |

## Usage, cost, quota, status

| Feature | Verdict | Owner |
| --- | --- | --- |
| Plan windows (5 h / 7 d), 80/95 % alerts, Codex quota, cost archive at API prices, time with agent, digest | keep | `cockpit-usage` (18) |
| Provider incident status pages | keep | usage/status; adapters supply URLs |
| Waybar module, bar visibility | change | desktop client reads a status file (19) |

## Resources and safety

| Feature | Verdict | Owner |
| --- | --- | --- |
| Build shims (nice/SCHED_IDLE), heavy-gate admission, game guard core pinning, failsafe freeze/thaw, `freeze-while-gaming` | keep | resource-admission with Linux backend; macOS/WSL: nice + SIGSTOP only, reported honestly (10) |
| Disk ≥85 % warning, greyproxy-held requests warning, load reports | keep | diagnostics/readiness (09, 12) |
| Meeting hold (`khal`), idle lock prompt, "still there?", away mode, banners | change | desktop client (19); not in daemon |
| Error hand-off of Cockpit log errors to a repair session | drop | replaced by diagnostics + support bundle (12) |

## Projects, Git, GitHub, Docker

| Feature | Verdict | Owner |
| --- | --- | --- |
| Parallel clones (`--no-local`, trust, hooks, memory set-aside), clones sweep, `K` check | keep | projects (02) |
| lazygit in sandbox, `U` push with safe git config (GitHub-only origin, no rewrites, hooks off) | keep | projects + `cockpit-github`; host git safety rules unchanged |
| Reviewed repos: clone vs running copy, `.outgoing` patches, `cockpit apply`, `after-apply` | change | optional "protected checkout" policy in projects; apply stays host-reviewed (15 open) |
| Per-project rootless Docker (system units, per-project users, nftables, relays), `agent-docker-build` CA trust, forwards | keep | containers + sandbox (11); system install optional, reported by readiness |
| GitHub tokens (repo-only, board), protect/check, PR review | keep | `cockpit-github` |

## Design and desktop integrations

| Feature | Verdict | Owner |
| --- | --- | --- |
| FigCraft per-checkout launch, port reservation, MCP wiring, design-review float, Hyprland lua | plugin | first plugin candidate (14); `design link` stays generic in sessions |
| Claude Design canvas link, `N` canvas mode | keep | sessions design link (generic URL) |
| Clipboard image bridge (Wayland), `wl-paste` shim | keep | sandbox host service with per-platform backend (11) |
| Phone page: pairing, push, live screen, answers, wake, todos, PRs, usage, lock-time notifications | keep | `cockpit-web` client (19) |

## Operations

| Feature | Verdict | Owner |
| --- | --- | --- |
| `cockpit doctor`, health-check timer, `cockpit logs`, `reload`, `reset` | keep | readiness (09), diagnostics (12), daemon lifecycle (13) |
| Name migration (`claude-*` aliases, inode-preserving renames) | drop | greenfield has one name |
| Volume/brightness OSD island | drop | not a Cockpit feature; stays in Theo's dotfiles |
| Demo fixture + screenshot capture, Arch package staging, `make ci` quality gates | keep | release (13) |
| Greywall/greyproxy forks built from `packages/` with patches | keep | Cockpit's own forks, pinned and built by the release bundle (11, 13) |

## Gaps this map found in reports 01–15

- No owner for usage/quota/accounts, GitHub integration, audit log, settings/config, desktop
  notifications: added to report 02 and new reports 17–19.
- Report 05's "never screen scraping" was too absolute: refined as above.
- Report 13 assumed overlap during upgrade; Turso's single opener makes it stop-then-start.
- The agent↔daemon channel (answers dir, launch tokens, report socket) had no report: 17.
