# Independent Codex Quota Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read Codex quota using a dedicated ChatGPT sign-in, without changing OpenCode accounts or restarting agent sessions.

**Architecture:** Replace the quota reader's credential-file and SQLite access with a short-lived official Codex app-server subprocess over stdio. Give that process a dedicated `CODEX_HOME`; Codex owns browser login, token storage and refresh. Keep the existing runtime cache contract and polling schedule.

**Tech Stack:** Existing Bash wrapper, Python 3 standard library for JSON-RPC/process management, official installed Codex CLI, Rust script-entry-point tests.

**Spec:** Approved Option A in this conversation: independent browser sign-in, Codex-managed credentials, unchanged display/cache, regression tests, no OpenCode account changes or restarts. This section is the complete small-change design; there is no separate spec file.

## Global Constraints

- Never read or modify OpenCode credentials, or borrow the normal Codex CLI account.
- No custom OAuth implementation, password storage, browser cookies, model turns or command execution requests.
- Use `${XDG_DATA_HOME:-$HOME/.local/share}/codex-quota` as the dedicated Codex home, created privately (0700); ignore inherited `CODEX_HOME` for this integration.
- Call the documented installed vendor binary `$HOME/.local/npm/bin/codex` on the host, not the Greywall agent launcher. Tests use a fake binary at that exact path inside their scratch HOME. Never attempt live host access from inside the agent sandbox.
- No dependency/Makefile changes, installs, service restarts, git writes or deployment without separate authorization.
- Host-executed changes: the existing Waybar polling script and its new Python helper. Explicitly highlight these in the handoff.

## Review Focus

- A missing CLI or unsigned-in account must report a fixed actionable error, never switch accounts.
- Unsolicited notifications, EOF, malformed JSON and stalled subprocesses must not hang polling or leave children behind.
- A missing Codex bucket must not select a different product's rate limits or manufacture zero usage.
- Failed reads must preserve the successful cache bytes and timestamp; successful reads must clear the error.
- Process stderr, account emails, tokens and server error text must not leak into quota caches or polling output.

## References

- https://developers.openai.com/codex/app-server/ — initialize/initialized, managed `account/login/start`, `account/login/completed`, `account/read`, `account/rateLimits/read`.
- https://developers.openai.com/codex/auth/ — managed refresh, CODEX_HOME and credential-store configuration.
- Confirm the installed CLI version and supported flags through an approved host command before live integration. Do not assume current online docs exactly match it.

## Execution ledger

- Workspace verified: parallel clone, branch `todo-cacc3bc2`; no pre-existing product edits.
- Pre-flight: Task 1 produces normalized quota JSON consumed unchanged by Task 2; login bypasses the polling lock. No interface conflict.
- Ruling: Keep progress here because the execution skill's workspace helper is absent (reported via claude-todo blocked). No commits or deployment are authorized; preserve the ledger for review instead of deleting it. Cost if wrong: documentation cleanup only.
- Task 1: implemented; RED: client target 5 failed / 2 passed (missing helper); GREEN: `cargo test --test codex_quota_client` → 9 passed. Includes test-process-only accelerated deadline and child reap verification.
- Task 2: implemented; RED: Waybar target 5 failed / 13 passed (old reader); GREEN: `cargo test --test codex_quota_client --test waybar_claude_scripts` → 9 passed / 18 passed. Old credential-selection tests were replaced because the approved independent-account contract explicitly removes selection/fallback.
- Intermediate verification: one combined test run ended with signal 50 after 17 Waybar tests. The remaining test passed alone; a subsequent complete combined run passed (9 + 18). Cause of that termination is unconfirmed.
- Host read-only prerequisite check: `codex-cli 0.161.0`, app-server supports `-c` and default stdio. No accounts changed by this check.
- `bash -n config/waybar/.config/waybar/scripts/codex-usage.sh` passed. Full workspace tests and independent review pending.
- Full debug workspace command exited 101: `a_recorded_text_marked_fp_is_kept_only_as_its_hash` and `verify_says_whether_the_chain_holds` failed in `cockpit/tests/claude_audit.rs`; output included `claude-audit is not built yet: make -C ~/dotfiles cockpit`. Re-running the repository's release-mode test command (`cargo test --release --quiet --manifest-path apps/Cargo.toml`), rather than changing unrelated tests or wrappers.
- Final review: independent reviewer found SIGTERM child cleanup (Important), empty XDG_DATA_HOME handling (reported Minor), and deeply nested JSON traceback (Minor).
- Final: Ruling: Grade empty XDG_DATA_HOME as Important because relative credential storage could place secrets in a checkout. Use the documented unset/empty fallback. Cost if wrong: environment compatibility only.
- Final: fixed helper termination cleanup and empty data-home fallback — `terminating_the_helper_reaps_its_stalled_server` and `empty_data_home_uses_the_default_private_directory` RED→GREEN; quota/Waybar targets 11/11 and 18/18 passed.
- Final: minor (deferred): extremely deeply nested protocol JSON can produce a traceback on direct helper invocation (RecursionError); normal polling suppresses stderr and retains the last good cache. No server data is interpolated in the traceback.
- Full release workspace command exited 101 on unrelated `yes_and_no_from_the_phone_press_the_dialog_keys` (`cockpit/tests/claude_remote.rs:228`): expected HTTP 200, got 409 with `this session asks nothing now`. Audit tests passed in release mode. No unrelated tests changed or disabled.
- Live verification approved by user: ran this checkout's `codex-usage.sh --login` through ask-to-run on the host. Dedicated browser login completed; immediate quota read succeeded. Safe cache inspection: primary used_percent=44, limit_window_seconds=604800, secondary=null (56% weekly quota remaining). OpenCode unchanged; files not installed/applied.
- Complete release inventory (`--no-fail-fast`) exited 101: 15 failures in four unrelated targets; quota tests passed. A final run clears inherited agent/session environment only, to distinguish test isolation failures without modifying unrelated product code or disabling tests.
- Final isolated release run exited 101: the same eight remote/session-command failures remained; desktop and hook failures cleared. The Waybar target was terminated by signal 50, as in the earlier intermittent run. Dedicated quota/client targets passed independently; no claim of a green workspace suite.
- User authorized commit/export and the reviewed apply workflow after successful live verification. Preserve the standalone traceback limitation as a deferred minor; no dependency, Makefile, service or OpenCode changes.
- Rebased onto the latest applied dotfiles; equivalent already-applied commits were skipped automatically. Final full release run after rebase: 1129 passed, 8 failed, exit 101. Only `cockpit --test claude_remote` (1) and `cockpit --test claude_sessions_script` (7) failed, with the same names below; quota/client and Waybar targets passed. An earlier post-rebase run reached the 120-second command timeout; the final run used a 360-second limit and completed.

### Full-suite failure inventory (verbatim names)

`cockpit --test claude_remote`: `yes_and_no_from_the_phone_press_the_dialog_keys`.

`cockpit --test claude_sessions_script`: `compact_is_typed_only_at_the_prompt`, `exit_types_slash_exit_at_an_idle_prompt`, `handoff_types_rc_at_an_idle_prompt`, `prompt_commands_pause_between_the_text_and_its_enter`, `restart_exits_then_resumes_the_same_conversation`, `say_refuses_while_a_dialog_is_open_and_keeps_the_text_out_of_the_log`, `say_sends_at_the_prompt_and_queues_while_working`.

`cockpit --test session_state_hook`: `a_session_started_from_a_shell_reports_even_under_the_daemon`.

`desktop --lib`: `tests::a_credits_session_is_marked_in_the_snapshot_and_the_reboot_record_until_it_switches_back`, `tests::a_request_held_or_refused_is_audited_without_its_command`, `tests::a_sessions_agent_is_in_the_snapshot_and_the_reboot_record_and_claude_by_default`, `tests::a_terminal_session_is_marked_in_the_snapshot_and_the_reboot_record`, `tests::sessions_are_remembered_across_reboots`, `tests::the_daemon_writes_its_messages_to_the_shared_log`. Errors included `Read-only file system (os error 30)` for inherited host state paths.

### Task 1: Dedicated login and quota protocol client

**Files:**
- Create: `config/waybar/.config/waybar/scripts/codex-quota.py`
- Create: `apps/cockpit/tests/codex_quota_client.rs`
- Reuse: `apps/cockpit/tests/common/sandbox.rs` (inspect actual test helper layout before editing).

**Interfaces:**
- `python3 codex-quota.py`: print only normalized quota JSON to stdout; fixed errors to stderr, nonzero exit on failure.
- `python3 codex-quota.py --login`: explicit interactive login; print the authorization URL supplied by Codex and wait for completion. Login must never occur during polling.
- Both modes use the same dedicated home and vendor binary. Set Codex's documented credential store to `file` for this isolated home; Codex writes/refreshes credentials, the helper never reads them.
- Wire methods: initialize with clientInfo `{name: "dotfiles_codex_quota", version: "1"}`, wait for its response, send initialized, then account/read. Poll only a managed chatgpt account; request account/rateLimits/read. Login sends account/login/start with `{type: "chatgpt"}` and waits for the matching successful login notification.
- Quota result preserves `{rate_limit: {primary_window, secondary_window}}`; map `usedPercent` to `used_percent`, `windowDurationMins * 60` to `limit_window_seconds`, `resetsAt` to `reset_at`. Preserve null windows. Prefer `rateLimitsByLimitId.codex`; use the legacy single `rateLimits` only if the multi-bucket map is absent/null and its limitId is absent/null or codex. Reject a missing codex bucket in a populated map.
- Validate finite percentages in [0,100], positive window durations and numeric reset timestamps; reject malformed values instead of presenting fabricated availability. A result with no usable windows is an error.
- Poll deadline: 15 seconds total. Login deadline: 5 minutes. Bound JSON line size to 1 MiB, match request IDs, ignore notifications, and reject unexpected server requests. Suppress subprocess stderr, close pipes, terminate/wait (kill/wait if necessary) in all exits. Use stdio only; no listening socket.

- [ ] Write entry-point tests with a fake vendor CLI implementing interactive JSONL responses. Assert handshake ordering and dedicated home, successful mapping, managed login success/cancel/error, and rejection of API-key/missing accounts. The fake must reject any thread/turn/command methods.
- [ ] Run `cargo test --test codex_quota_client` from `apps/cockpit`; confirm failure because the helper does not exist.
- [ ] Implement the minimal standard-library protocol client and explicit login mode described above.
- [ ] Extend tests for notifications before responses, malformed messages, wrong response IDs, EOF, subprocess failure, oversized lines, timeout/cleanup, multi-bucket selection, null windows and invalid numbers. Assert secrets/account email/server error text are absent from polling stdout/stderr. Use a test-only subprocess driver or simulated clock for deadline tests rather than making each test wait 15 seconds; do not add an unvalidated production timeout override solely for tests.
- [ ] Run the client test target; all cases must pass.

### Task 2: Integrate polling and document sign-in

**Files:**
- Modify: `config/waybar/.config/waybar/scripts/codex-usage.sh`
- Modify: `apps/cockpit/tests/waybar_claude_scripts.rs`
- Modify: `README.md` (existing cockpit/usage documentation).

**Interfaces:**
- Preserve normal and `--force` behavior, 240-second cache reuse, runtime flock, atomic writes, cache filenames and error JSON shape.
- Add `codex-usage.sh --login` forwarding to the sibling helper before acquiring the polling cache lock; success must invalidate freshness or perform a forced read so the next display reflects the new sign-in.
- Invoke the sibling Python helper instead of sqlite/jq credential extraction/curl; never inspect credential files. Remove obsolete OpenCode/CLI credential fallback code.
- Forward only fixed helper failures into the existing error cache; preserve previous successful quota and timestamp on failure.

- [ ] Update script-entry-point fixtures to use a fake Codex app-server. Replace obsolete OpenCode account-selection tests with tests proving OpenCode stores and normal Codex credentials are untouched and never consulted, including a valid OpenCode token beside an unsigned-in quota account. This deliberately replaces the old authentication contract, not disables failing tests.
- [ ] Run `cargo test --test waybar_claude_scripts`; verify the new integration assertions fail on the existing reader.
- [ ] Replace the reader's credential/network section with helper invocation; retain locking, successful-cache TTL, forced refresh and atomic cache handling.
- [ ] Verify caching/forced reads, successful error clearing, authentication/network/malformed-response failures retaining last good bytes and timestamp, fixed safe errors and login forwarding. Keep existing Claude and chip tests unchanged.
- [ ] Document the one-time installed `codex-usage.sh --login` command, isolated storage, Codex-managed refresh, missing-binary handling and re-login after revocation. State no OpenCode/session restart is needed and that polling runs on the host.
- [ ] Run `cargo test --test codex_quota_client --test waybar_claude_scripts` from `apps/cockpit`, plus `bash -n config/waybar/.config/waybar/scripts/codex-usage.sh` from the repo root. Record verbatim results and `git diff --stat`.
- [ ] Stop for review before deployment. Once separately authorized, use the dotfiles apply workflow, approved host login and live forced read. Complete todo `cacc3bc2` only after a successful live quota read; no claim of completion while it still returns unknown.
