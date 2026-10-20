# Provider Quota Summary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make pane `[0]` show whether Claude and Codex have subscription capacity left.

**Architecture:** Replace the Status panel's Claude usage bars with compact, provider-labelled quota rows. Retain the existing Claude fetcher; add a Codex fetcher following its cached, background-polled pattern. Read Codex credentials from OpenCode's existing OAuth account, falling back to Codex CLI credentials only when no OpenCode OAuth account exists; never refresh credentials.

**Tech Stack:** Rust/ratatui, Bash, jq, curl; existing dependencies only.

**Spec:** The approved Option A in this conversation: provider name, availability label, percentage left and reset countdown; separate short-term and weekly limits, automatic refresh, explicit unknown/stale states.

## Global Constraints

- Preserve cost, time-together, service-status rows, Claude budget alerts and the expanded usage report.
- No dependency-manifest changes, installation, host deployment or build commands without permission.
- Credentials must never enter logs, caches, command-line arguments or test output.
- No credential refresh, provider switching or automatic routing.
- Snapshot quota data during App refresh, not on keypress-driven redraws.
- Poll every 300 seconds; mark data older than 600 seconds stale.
- Available means more than 20% left; Low means 1–20%; Exhausted means 0%. Unknown/stale data must never say Available.
- Keep raw percentages for display, rejecting out-of-range or nonnumeric values rather than hiding invalid input through clamping.
- Do not commit or change branches without git authorization. Do not mark todo 90ca98ff done until implementation and verification finish.

## Review Focus

- One exhausted window must remain obvious even when another window has ample capacity.
- Missing or invalid quota data must not look like a fully available subscription.
- A failed refresh must not make old data appear newly fetched or erase the last successful response.
- An elapsed reset time must not imply a fresh quota until the provider confirms it.
- Narrow terminal widths must retain provider, status and remaining percentage before optional reset text.

---

### Task 1: Fetch Codex quota without modifying authentication

**Files:**
- Create: `config/waybar/.config/waybar/scripts/codex-usage.sh`
- Modify: `config/waybar/.config/waybar/scripts/claude-usage.sh` (explicit forced refresh)
- Test: `apps/cockpit/tests/waybar_claude_scripts.rs`
- Modify: `apps/cockpit/src/usage.rs` (`Refresher::tick`)

**Interfaces:**
- Consumes: OpenCode OAuth `openai` entry (`type`, `access`, `expires`, `accountId`) from `$XDG_DATA_HOME/opencode/auth.json`, defaulting to `~/.local/share/opencode/auth.json`; Codex fallback `$CODEX_HOME/auth.json`, defaulting to `~/.codex/auth.json`, with `tokens.access_token` and `tokens.account_id`.
- Produces: `$XDG_RUNTIME_DIR/codex-usage.json`, containing the successful response from `https://chatgpt.com/backend-api/wham/usage`. File modification time is the successful fetch time.
- Produces: a separately timestamped `codex-usage-error.json` with a fixed, sanitized reason when fetching fails; successful fetching clears that error.
- `Refresher::tick(force)` launches both provider scripts on the existing five-minute schedule; force requests bypass caches without deleting the previous successful response.

- [x] Write script-entry-point tests using scratch credentials and a curl stand-in: a successful fetch persists primary/secondary windows; valid recent cache avoids fetching; forced refresh fetches; absent/expired credentials report unknown without network requests; failed HTTP/malformed JSON leaves the previous cache unchanged; success clears the error. Assert credentials remain unchanged and no token appears in output/cache. For Claude, test that forced refresh bypasses a fresh cache but preserves it on HTTP failure.
- [x] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test waybar_claude_scripts`; observe expected failures because the script and force handling do not exist.
- [x] Implement the script with a runtime-file lock, atomic replacement and a 15-second HTTP timeout. Feed authorization headers through curl's stdin, not argv; do not follow redirects. Validate that the response contains rate-limit data before caching. Use fixed error messages rather than credential-bearing stderr or response bodies. Prefer an existing OpenCode OAuth account even if expired, so fallback cannot silently select another account.
- [x] Extend `Refresher::tick` to launch the Codex fetcher and pass an explicit `--force` argument to both scripts when requested. Teach the Claude script to recognize it as its second argument instead of deleting the successful cache in Rust. Preserve its normal Waybar output and polling behavior.
- [x] Repeat the script tests and verify they pass.

### Task 2: Render compact remaining-capacity rows in Status

**Files:**
- Modify: `apps/cockpit/src/usage.rs`
- Modify: `apps/cockpit/src/paths.rs`
- Modify: `apps/cockpit/src/app.rs`
- Test: `apps/cockpit/src/tests.rs`

**Interfaces:**
- Consumes: existing `usage::Window` values for Claude; Codex `rate_limit.primary_window` and `secondary_window` with `used_percent`, `limit_window_seconds`, `reset_at` (Unix seconds), plus `allowed`/`limit_reached` flags when supplied.
- Produces: `usage::QuotaRow { label: String, state: String, remaining: Option<i64>, resets: String }` and `usage::quota_rows() -> Vec<QuotaRow>`; App stores the returned snapshot alongside its existing Claude windows.
- Labels identify provider and duration, e.g. `Claude · 5h`, `Claude · week`, `Codex · 5h`, `Codex · week`. Preserve scoped Claude limits and actual Codex durations rather than assuming every window is five hours or a week.
- A provider without windows still gets one `Unknown` row. Known rows show `Available · 72% left`, `Low · 14% left`, or `Exhausted · 0% left`; stale rows say `Stale · last 72% left`. Reset countdown is optional and lowest priority at narrow widths.

- [x] Add App-entry-point tests for provider-labelled remaining percentages, short-term and weekly rows, thresholds (used 0/79/80/99/100), null/invalid percentages, independent exhausted windows, stale caches, failed refreshes, passed reset timestamps and missing providers. Use fixed fixtures and file timestamps, never live accounts or network.
- [x] Render the ratatui buffer at widths 40 and 60; assert both providers and their status/remaining values survive without overlap. Verify non-quota Status rows and Claude budget alerts still work.
- [x] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib`; observe the new behavior tests fail against the current bars.
- [x] Implement path helpers and quota parsing in `usage.rs`. Derive remaining from valid used percentages. Determine freshness from successful cache timestamps and reset timestamps; a newer error means Unknown without a previous cache, Stale with one. Respect explicit provider blocking flags even if percentages appear available.
- [x] Snapshot rows in `App::refresh`; replace only the usage-bar section of `App::build_status`. Use existing colors as a secondary cue and text as the primary signal. Update layout expectations for the always-visible two-provider state without weakening layout assertions.
- [x] Run the focused tests, then `cargo test --manifest-path apps/Cargo.toml` for the entire workspace. Report failures by name; do not disable tests.
- [x] Run `git diff --check` and show the diff stat, including new files. Review that the new script runs outside the sandbox and explicitly identify it in the handoff.
- [x] Once all verification passes, run `claude-todo done 90ca98ff`. Host application/build/reload remains a separately approved action.

## Verification outcome

- Final command: `env -u OPENCODE -u CLAUDE_SESSIONS_STATE_HOME cargo test --release --quiet --manifest-path apps/Cargo.toml`: **927 passed, 0 failed, 0 ignored**.
- Script tests: **18 passed**; quota-specific App tests: **10 passed**. Bash syntax and `git diff --check` passed.
- Fresh review found invalid HTTP-success Claude payloads overwriting the previous cache. A regression test reproduced it; validation now preserves the successful cache and records failure instead.
- Earlier workspace failures were missing release executables and inherited agent/state environment variables. The user approved building local `desktop/osd`; verification then used scratch-only fixture state. No tests were disabled or unrelated fixtures changed. Test-isolation follow-ups were reported separately.
- Existing cost, time, Claude service status, budget alerts and expanded usage report remain. The Claude and Codex polling scripts run outside the agent sandbox when applied; neither refreshes credentials. No live application, reload, install or git write was performed.
