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
| Start session (blank, with prompt, in own clone), background/detached, terminal-in-place | keep | sessions, projects, tmux (05): `cockpit session new` proven by `cockpit-daemon` `sessions::a_new_session_runs_its_agent_in_place_with_its_first_message`, `a_new_session_in_a_clone_occupies_a_parallel_clone_on_its_branch` and `cockpit-cli` `sessions::session_new_sends_an_absolute_dir_with_its_options_and_prints_the_session` (#31); agents run detached on the private tmux server; terminal-in-place with the TUI (#35) |
| Restart in place with same conversation; restart-all after config change (17 start files) | keep | sessions; config change detection in config (09): detection proven by `cockpit-config` test `start_file_changes::every_kind_of_change_to_a_read_file_is_reported` (#22); restart in place by `cockpit-daemon` `sessions::restart_stops_the_agent_and_resumes_the_same_conversation_in_place`, restart-all by `restart_all_restarts_every_running_session` (#31); triggering restart-all from a detected change pending |
| Sleep after idle, wake on open/message/todo/phone, sleep refusals, asleep notes 7 d | keep | sessions; permits via messaging (07): idle sleep proven by `cockpit-daemon` `sessions::an_agent_that_waits_at_its_prompt_past_the_idle_window_is_put_to_sleep`, refusals by `a_working_agent_is_not_put_to_sleep_however_long_it_works` and `a_session_whose_provider_cannot_resume_is_refused_sleep_and_keeps_running`, wake by `sleep_stops_the_agent_and_wake_resumes_its_conversation`, 7-day expiry by `an_asleep_session_past_the_expiry_ends` (#31); wake on open, message, todo and phone with their callers (#35, messaging) |
| Reboot restore, ended/recent sessions, resume past session (`H`) | change | sessions: restore is automatic on daemon start, no `B` key (decided 2026-10-31): proven by `cockpit-daemon` `sessions::on_daemon_start_a_session_whose_agent_is_gone_is_reopened_without_a_request`, `on_daemon_start_a_session_whose_agent_still_runs_keeps_it`, `a_failed_restore_is_reported_and_not_retried`, `a_stop_interrupted_by_a_daemon_crash_is_finished_on_start`; ended sessions and resume by `an_agent_that_exits_ends_its_session_and_resume_reopens_the_conversation` and `cockpit-cli` `sessions::session_list_ended_asks_for_ended_sessions_and_prints_one_line_each` (#31) |
| Effort, model/variant pickers, billing toggle (credits vs subscription), `/compact`, Remote Control | change | capabilities in the adapters and CLI verbs only; no TUI keys (`e`, `$`, `C`, `m` dropped 2026-10-31) |
| Stuck-exit capture, forced exit rounds | keep | processes + diagnostics (12): forced rounds (`SIGTERM`, then `SIGKILL` to the whole containment, confirmed only when observed empty) proven by `cockpit-processes` test `lifecycle::termination_escalates_to_sigkill_when_sigterm_is_ignored` (#29); a stop that cannot be confirmed keeps the session `running` with its reason, never recorded as stopped (#31, `cockpit-sessions` README); capturing the stuck agent's state for diagnostics pending |
| Agent state from hooks/plugins/Codex bridge; pane scraping fallback (`osd classify`) | change | adapters own detection; structured events first, screen classification as adapter-internal fallback with recorded confidence; never task completion (05) |
| Per-agent adapter verbs (`new resume restart exit say prompt_start doctor …`) | keep | `cockpit-provider-api` trait + one bridge per adapter (08, 17): the baseline contract (launch a new or resumed conversation, `resume` capability) exists, kept from sessions' adapters by the `sessions -> provider adapters` crate-graph rule (#31); the adapters and the rest of the verbs with #33 |
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
| `ask-to-run` (fish parse, approval pane, PTY run, redaction, phone answer), `ask-to-guide`, `--check` | keep | approvals + processes (17); binding and consume-once by `cockpit-approvals` `runs::an_offline_approval_of_an_edited_request_fails_its_revision_then_the_replay_runs_the_edit` (report 04, walkthrough 3) and `runs::an_approval_is_consumed_once`; shell parse, PTY run, exit code, redaction, hooks off and input by `cockpit-daemon` `approvals::a_command_the_shell_cannot_parse_is_refused_before_the_owner_is_asked`, `an_approved_command_runs_in_a_terminal_and_returns_its_exit_code`, `secrets_in_the_output_are_redacted_before_they_reach_the_agent`, `git_in_an_approved_run_has_hooks_off`, `the_owner_answers_a_command_waiting_for_input`; display rules by `the_owner_sees_the_request_in_the_queue_with_the_display_rules_applied`; guides by `a_guide_is_walked_one_step_at_a_time_and_the_agent_reads_the_answers`; exit codes and `--check` by `cockpit-cli` `ask::each_answer_keeps_its_exit_code`, `an_unreachable_daemon_exits_126_with_the_fix`, `ask_guide_sends_standard_input_and_exits_3_when_the_owner_is_stuck`, `ask_check_reports_a_refusal_with_what_unblocks_it` (approval pane with the TUI, #35; phone answer with notifications; full-output answers file with the mount, #61) |
| Approval safety from the legacy todos (#156): hidden-character warnings, folded long commands, changed build files, `timeout` hint, PTY prompts, waiting pane first, asynchronous ask, approval grace | keep | approvals (17); warning kinds and fold by `cockpit-approvals` `display::each_kind_of_hiding_character_is_named_in_a_warning`, `a_command_of_many_lines_is_folded_with_the_lines_left_out_counted` and `cockpit-daemon` `approvals::a_long_command_reaches_the_owner_folded_with_what_is_left_out`; changed build files by `cockpit-projects` `changed_files::changed_files_are_the_uncommitted_ones_and_those_of_unpushed_commits`, `cockpit-approvals` `hints::each_build_tool_names_the_changed_files_it_runs` and `cockpit-daemon` `approvals::the_owner_is_warned_of_changed_build_files_the_command_runs_as_they_are_now`; `timeout` hint by `hints::timeout_without_foreground_gets_the_hint` and `approvals::a_timeout_without_foreground_carries_the_hint`; PTY prompts by `approvals::a_password_prompt_reads_the_typed_line_with_echo_off` and `a_prompt_in_a_pipeline_stage_reads_the_terminal_in_the_foreground_group`; waiting pane first by `approvals::a_new_request_queues_behind_a_command_waiting_for_input`; asynchronous ask by `approvals::an_agent_waiting_on_an_approval_keeps_working_on_the_channel` and `cockpit-cli` `ask::ask_run_no_wait_files_the_request_and_returns_at_once_with_how_to_wait`; grace by `runs::an_approval_right_after_the_ask_is_refused_until_the_grace_has_passed`, `runs::an_edit_starts_the_grace_again` and `approvals::an_approval_at_once_after_the_ask_is_refused_with_when_to_approve`; CLI rendering by `ask::the_owner_reads_a_request_with_its_warnings`, `the_owner_is_warned_of_the_changed_build_files_a_command_runs`, `the_owner_gets_the_timeout_hint_before_approving`, `the_queue_folds_a_long_command_behind_an_explicit_more` (deliberate key, pane grace, focus guard and their rendering with the TUI, #35, decided in #166) |
| `cockpit-needs-input`, `session-label`, `session-doing`, todo CLI over the report socket | keep | one agent channel (17); label, doing and needs-input proven by `cockpit agent report` through `cockpit-daemon` `agent_channel::a_launched_agent_reports_and_the_owner_reads_it` and `cockpit-cli` `agent_report::a_report_goes_to_the_agent_socket_with_the_launch_token` (#32); the todo verb waits for its owner (#44) |
| Per-project read-only answers dir, kernel peer identity, request copies host-side | keep | agent channel (17, 11); peer identity proven by `cockpit-daemon` `agent_channel::a_token_leaked_to_another_process_tree_is_refused`, `a_session_named_in_the_payload_does_not_change_who_reports` and `a_nested_agent_reports_with_its_parent_sessions_grants`; answers dir by `cockpit-agent-channel` `answers::an_answer_is_written_whole_and_read_only` (sandbox mount pending, #61); request copies by `cockpit-daemon` `approvals::an_approval_of_a_stale_revision_is_refused_and_the_replay_runs_the_edit` (the daemon runs its own copy, never re-read from the sandbox) |
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
| FigCraft per-checkout launch, port reservation, MCP wiring, design-review float, Hyprland lua | plugin | first plugin candidate (14); `design link` stays generic in sessions: `cockpit session link`, proven by `cockpit-daemon` `sessions::a_design_link_is_kept_on_the_session_and_anything_but_a_url_is_refused` (#31) |
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
