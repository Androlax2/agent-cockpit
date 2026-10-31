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
| Restart in place with same conversation; restart-all after config change (17 start files) | keep | sessions; config change detection in config (09): detection proven by `cockpit-config` test `start_file_changes::every_kind_of_change_to_a_read_file_is_reported` (#22); restart-all pending |
| Sleep after idle, wake on open/message/todo/phone, sleep refusals, asleep notes 7 d | keep | sessions; permits via messaging (07) |
| Reboot restore, ended/recent sessions, resume past session (`H`) | change | sessions: restore is automatic on daemon start, no `B` key (decided 2026-10-31) |
| Effort, model/variant pickers, billing toggle (credits vs subscription), `/compact`, Remote Control | change | capabilities in the adapters and CLI verbs only; no TUI keys (`e`, `$`, `C`, `m` dropped 2026-10-31) |
| Stuck-exit capture, forced exit rounds | keep | processes + diagnostics (12): forced rounds (`SIGTERM`, then `SIGKILL` to the whole containment, confirmed only when observed empty) proven by `cockpit-processes` test `lifecycle::termination_escalates_to_sigkill_when_sigterm_is_ignored` (#29); stuck-exit capture pending |
| Agent state from hooks/plugins/Codex bridge; pane scraping fallback (`osd classify`) | change | adapters own detection; structured events first, screen classification as adapter-internal fallback with recorded confidence; never task completion (05) |
| Per-agent adapter verbs (`new resume restart exit say prompt_start doctor …`) | keep | `cockpit-provider-api` trait + one bridge per adapter (08, 17) |
| Subscription fallback/routing between accounts (routes.json, gates in hooks, native evidence) | change | account routing at launch and at quota-stop only; no mid-turn gates (18) |
| Native Codex thread ids, OpenCode standalone server + plugin, Claude hooks/statusline | keep | inside each adapter (08) |
| Claude/OpenCode auto-update timers | change | adapter `update` capability run by `cockpit doctor --fix`/timer (13) |

## Terminal, navigation, viewer

| Feature | Verdict | Owner |
| --- | --- | --- |
| Manager tmux session, column + viewer pane, nested attach per agent session | keep | tmux on a private server (05); nested attach: `cockpit-tmux` `the_viewer_pane_attaches_to_an_agent_session_nested`, approval split: `the_approval_pane_opens_focused_below_the_viewer` (the manager session and column layout come with the TUI, #35) |
| Focus fade (`cockpit-fade`), Left-at-prompt-start (`cockpit-left`), kitty chords, loading/restarting screens | polish | tui (05) |
| Workspace 6 kitty window toggle, Hyprland/waybar signals, floating approval/guide windows | change | desktop integration client (19); approvals inside tmux panes by default |
| Image viewer (`I`), reply images | polish | dropped as is; redesigned in M6 (decided 2026-10-31) |
| Contextual help `?`, card legend | keep | tui |
| Team view (`k`), Chief answers (`c`), drop queued (`d`), next needing you (`n`) | change | `n` stays; `k`/`c` become basic lists in M4, designed in M6; `d` dropped |
| Settings view (`S`), web windows (`P`/`W`/`G`), daily digest (`D`) | polish | `cockpit config` CLI now; designed views and web-window approach in M6 |

## Tasks, todo, queue

| Feature | Verdict | Owner |
| --- | --- | --- |
| Items, priorities, kinds, statuses, notes with images, drafts surviving reload | keep | tasks (06) |
| Proposals/Ideas (`y`/`n`), declined memory | keep (core) / polish (view) | tasks: `proposed` status + decline memory; designed view in M6 |
| `blocked` reports to the dotfiles project; `stack` proposals to greywall/greyproxy | change | generic "report blocker to configured target project" (owner setting); stack = plain cross-project proposal |
| `--after` dependencies, `unwait`, completion summaries to the filing session | keep | tasks + messaging |
| Cross-project `add --project`, caps per project | keep | tasks; caps in config |
| Queue dispatch (idle 5 min, priority, viewed-session exclusion, designer routing), `Q` explanation | keep | scheduling (10) with explanations |
| Todo trailers closing items on apply | keep | tasks + verification evidence (06) |
| Task trail, updates/reviewed, return notes, defer 20 min, details view (`t`, `u`, `v`, `w`, `l`, `i`) | drop | unused (decided 2026-10-31); rebuilt only if M6 usage data asks |
| GitHub issues shown with todos; review session from a PR | keep | `cockpit-github` + projects; CLI verb for the review session |
| Pull requests tab | drop | not needed (decided 2026-10-31); PR state stays as task evidence |

## Approvals, host actions, agent channel

| Feature | Verdict | Owner |
| --- | --- | --- |
| `ask-to-run` (fish parse, approval pane, PTY run, redaction, phone answer), `ask-to-guide`, `--check` | keep | approvals + processes (17) |
| `cockpit-needs-input`, `session-label`, `session-doing`, todo CLI over the report socket | keep | one agent channel (17); label, doing and needs-input proven by `cockpit agent report` through `cockpit-daemon` `agent_channel::a_launched_agent_reports_and_the_owner_reads_it` and `cockpit-cli` `agent_report::a_report_goes_to_the_agent_socket_with_the_launch_token` (#32); the todo verb waits for its owner (#44) |
| Per-project read-only answers dir, kernel peer identity, request copies host-side | keep | agent channel (17, 11); peer identity proven by `cockpit-daemon` `agent_channel::a_token_leaked_to_another_process_tree_is_refused`, `a_session_named_in_the_payload_does_not_change_who_reports` and `a_nested_agent_reports_with_its_parent_sessions_grants`; answers dir by `cockpit-agent-channel` `answers::an_answer_is_written_whole_and_read_only` (sandbox mount pending, #61); request copies with approvals (#34) |
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
| Plan windows (5 h / 7 d), 80/95 % alerts, Codex quota, cost archive at API prices, time with agent, digest | keep (core) / polish (views) | `cockpit-usage` (18); Status panel and digest views designed in M6 |
| Provider incident status pages | keep | usage/status; adapters supply URLs |
| Waybar module, bar visibility | change | desktop client reads a status file (19) |

## Resources and safety

| Feature | Verdict | Owner |
| --- | --- | --- |
| Build shims (nice/SCHED_IDLE), heavy-gate admission, game guard core pinning, failsafe freeze/thaw, `freeze-while-gaming` | keep | resource-admission with Linux backend; macOS/WSL: nice + SIGSTOP only, reported honestly (10) |
| Disk ≥85 % warning, greyproxy-held requests warning, load reports | keep | diagnostics/readiness (09, 12) |
| Meeting hold (`khal`), idle lock prompt, "still there?", away mode, banners | change | desktop client (19); not in daemon |
| Error hand-off of Cockpit log errors to a repair session | drop | replaced by diagnostics + support bundle (12); bundle proven by `cockpit-diagnostics` `support_bundle::the_written_archive_holds_exactly_the_listed_entries` (verb wiring pending: the host-only socket exists since #32; the verb is not wired yet) |

## Projects, Git, GitHub, Docker

| Feature | Verdict | Owner |
| --- | --- | --- |
| Parallel clones (`--no-local`, trust, hooks, memory set-aside), clones sweep, `K` check | keep | projects (02). Proven by `cockpit-cli/tests/projects.rs` (#28): `a_parallel_clone_is_a_dissociated_local_clone_on_its_branch_with_origin_on_github`, `a_parallel_clone_of_a_trusted_project_inherits_its_trust`, `a_parallel_clone_gets_cockpits_lefthook_runner_and_a_review_clone_none`, `the_sweep_keeps_a_clone_that_holds_work_or_a_holder_naming_the_reason`, `checkout_occupancy_survives_a_daemon_crash`. Memory set-aside moves to the Claude adapter; the legacy script has no `K` clone check to port (DECISIONS 2026-10-31, #28) |
| Push with safe git config (GitHub-only origin, no rewrites, hooks off) | keep | projects + `cockpit-github` as a CLI verb; host git safety rules unchanged. Proven by `cockpit-cli/tests/projects.rs` (#28): `every_host_git_call_runs_with_hooks_off_and_program_settings_reset`, `a_push_to_an_origin_outside_github_is_refused`, `a_push_redirected_by_the_checkouts_own_configuration_is_refused`, `a_push_that_would_rewrite_the_remote_branch_is_refused`, `a_push_to_an_origin_with_a_second_url_is_refused`, `a_push_rewritten_off_github_by_the_owners_configuration_is_refused`, `a_push_url_rewritten_off_github_by_the_owners_configuration_is_refused`, `a_content_filter_a_clone_configures_does_not_run_on_host_git`, `a_signature_program_a_clone_configures_does_not_run_during_the_sweep` |
| lazygit key (`g`), push key (`U`), design-canvas key (`o`) | drop | unused keys (decided 2026-10-31) |
| Reviewed repos: clone vs running copy, `.outgoing` patches, `cockpit apply`, `after-apply` | drop | Theo-only tooling outside the product (decided 2026-10-31); lives in his dotfiles, uses public CLI verbs only |
| Per-project rootless Docker (system units, per-project users, nftables, relays), `agent-docker-build` CA trust, forwards | keep | containers + sandbox (11); system install optional, reported by readiness |
| GitHub tokens (repo-only, board), protect/check, PR review | keep | `cockpit-github` |

## Design and desktop integrations

| Feature | Verdict | Owner |
| --- | --- | --- |
| FigCraft per-checkout launch, port reservation, MCP wiring, design-review float, Hyprland lua | plugin | first plugin candidate (14); `design link` stays generic in sessions |
| Claude Design canvas link, `N` canvas mode | change | generic design link kept as data; no `o` key or canvas mode in the basic TUI |
| Clipboard image bridge (Wayland), `wl-paste` shim | keep | sandbox host service with per-platform backend (11) |
| Phone page: pairing, push, live screen, answers, wake, todos, PRs, usage, lock-time notifications | keep | `cockpit-web` client (19) |

## Operations

| Feature | Verdict | Owner |
| --- | --- | --- |
| `cockpit doctor`, health-check timer, `cockpit logs`, `reload`, `reset` | keep | readiness (09), diagnostics (12), daemon lifecycle (13); `COCKPIT_LOG_LEVEL` at daemon start proven by `cockpit-daemon` `startup::cockpit_log_level_decides_which_records_the_daemon_writes` (#27); `cockpit logs level` library proven by `cockpit-diagnostics` `level::an_override_reverts_when_it_expires` (verb wiring pending: the host-only socket exists since #32; the verb is not wired yet) |
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
