# Billing provenance and cost split implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve verifiable billing evidence alongside token usage, then distinguish API-credit estimates, subscription-equivalent value, and unknown billing without changing existing totals.

**Architecture:** Keep the existing `replies` archive and token-pricing calculation. Store per-reply billing evidence in a separate SQLite table, imported from small metadata-only records in the already-writable Claude cache. Capture evidence while a request is running; never reconstruct historical billing from a session's current settings.

**Tech Stack:** Existing Rust, rusqlite, Bash hooks, and OpenCode V2 TypeScript plugin; no new dependencies.

**Spec:** The approved scope is the following Global Constraints section. The terminal presentation is owned by the recruited Designer and must be approved before UI implementation; this plan covers the prerequisite data capture and reporting API only.

**Approved presentation:** Theo approved Option A in `DESIGN.md`, using frames 2 and 4 of `design-plans/cost-reporting-split.figcraft`; frame 3 is rejected. The existing Rust terminal renderers and test fixtures remain authoritative, with no browser or Storybook setup.

**Execution corrections:** Claude capture uses the Python-standard-library helper in the already-readable hook folder rather than exposing the sidebar binary to sandboxes. OpenCode captures request-bound route/auth evidence at dispatch, then binds it to `session.step.started.assistantMessageID`; installed V2 context does not expose an assistant ID at dispatch. OpenCode writes the tiny immutable metadata record synchronously because a short-lived native run can terminate asynchronous helper processes. These corrections and their RED→GREEN/live evidence are recorded in the execution ledger.

## Global Constraints

- Preserve existing token counts, prices, reply keys, totals, and archive retention.
- Billing modes are exactly `api`, `subscription`, and `unknown`; missing or conflicting evidence means `unknown`.
- An API figure is a token-cost estimate, not an invoice or verified credit deduction.
- Historical records remain unknown unless matched to evidence recorded for that exact reply.
- Do not infer billing from a nonzero OpenCode cost, model name, or current session flag.
- Never save credentials, authorization headers, prompts, or conversation text in billing records.
- No dependency manifests, sandbox permissions, Makefiles, or authentication behavior change.
- Host-executed launcher changes require explicit mention in the implementation summary.

## Review Focus

- Restarting one conversation with another billing mode must not relabel earlier replies.
- Changing model or provider within OpenCode must not reuse another provider's billing evidence.
- Late metadata and repeated import must not lose usage or count a reply twice.
- Unknown model prices must remain unpriced, not appear as free API usage.
- Corrupt or conflicting metadata must be visible and must not become a confident billing claim.

---

### Task 1: Archive per-reply evidence and expose grouped totals

**Files:**
- Modify: `apps/cockpit/src/costs.rs`
- Test: `apps/cockpit/tests/costs.rs`

**Interfaces:**
- Consumes: existing `replies.key`, `replies.day`, and nullable `replies.cost`.
- Produces: `Billing` enum (`Api`, `Subscription`, `Unknown`), and `BillingTotals` containing `api`, `subscription`, and `unknown`, each of type `Totals`.
- Produces: `archived_billing_totals(&Connection) -> rusqlite::Result<BillingTotals>` and `Costs::billing_totals(&self) -> BillingTotals`.
- Evidence schema: `reply_billing(key text primary key, billing text not null check (billing in ('api', 'subscription', 'unknown')))`. Keep `replies` unchanged so old archive rows and positional fixtures remain valid.

- [ ] **Step 1: Write failing archive tests.** Insert three one-million-output-token Opus 5.5 replies. Attribute one to API, one to subscription, and leave one without evidence. Assert each bucket is $20 and the original combined total is $60. Assert evidence added after the reply changes its bucket without changing the combined total. Assert conflicting evidence becomes unknown and stays unknown on repeated import. Assert unpriced models stay unpriced.
- [ ] **Step 2: Run** `cargo test --manifest-path apps/Cargo.toml -p cockpit --test costs` and confirm the new tests fail because billing support is missing.
- [ ] **Step 3: Implement** the additional table, `Billing`, grouped day aggregation, and the background state's grouped totals. Use the same local-day/week/month boundaries as existing totals. Use explicit column lists and retain all existing replies.
- [ ] **Step 4: Run** the same test command and the existing cost unit tests; confirm both pass.

### Task 2: Add metadata-only cache capture and import

**Files:**
- Create: `apps/cockpit/src/billing.rs`
- Modify: `apps/cockpit/src/lib.rs`
- Modify: `apps/cockpit/src/paths.rs`
- Modify: `apps/cockpit/src/bin/cockpit-sidebar.rs`
- Modify: `apps/cockpit/src/costs.rs`
- Test: `apps/cockpit/tests/costs.rs`

**Interfaces:**
- Consumes: per-reply JSON with exactly `key` and `billing`.
- Produces: `python3 config/claude/.claude/hooks/billing-capture.py record-billing`, reading one JSON record from stdin and atomically saving it in `~/.cache/claude/billing/` under a content-addressed SHA-1 filename. The digest is a filename, not authentication.
- Produces: the same helper's `record-transcript-billing <api|subscription> <since-rfc3339> <transcript-path>`, emitting evidence only for valid Claude replies timestamped at or after the verified launch start.
- Produces: `billing::import(&mut Connection) -> std::io::Result<usize>`, called during normal archive refresh and the existing costs/digest reports. Bad files warn without rolling back valid evidence.

- [ ] **Step 1: Write failing CLI tests.** Drive both commands through the real executable with a temporary HOME. Assert only reply key and billing mode are persisted, old transcript replies preceding `since` are excluded, repeated capture is idempotent, malformed input exits nonzero, and absent metadata leaves costs unknown. Include a two-launch conversation fixture with a $20 subscription reply followed by a $20 API reply; assert the earlier reply stays subscription.
- [ ] **Step 2: Run** the targeted costs tests and confirm failure at the missing command boundary.
- [ ] **Step 3: Implement** bounded input validation, atomic cache writes, transcript identity parsing matching the existing Rust archive keys, and archive import. Never follow symlinked metadata files. Conflicting modes become explicit unknown evidence; repeated capture cannot erase that conflict. Report read/parse/write failures with context rather than silently classifying them.
- [ ] **Step 4: Run** targeted tests and confirm the original no-source-history backfill tests still pass.

### Task 3: Capture Claude billing at the launch boundary

**Files:**
- Modify: `config/bin/.local/bin/greywall-agents/claude`
- Modify: `config/claude/.claude/hooks/session-state.sh`
- Test: `apps/cockpit/tests/greywall_launcher.rs`
- Test: existing hook test harness under `apps/cockpit/tests/`

**Interfaces:**
- Consumes: explicit launcher `COCKPIT_CREDITS` and the already-validated API key launch path.
- Produces: non-secret `COCKPIT_BILLING` (`api` or `subscription`) and `COCKPIT_BILLING_STARTED_AT` (RFC3339 UTC) exported immediately before launch.
- Consumes: Claude Stop-hook `transcript_path`; invokes `record-transcript-billing` with those launch facts, recovering them from the nearest launcher ancestor when Claude scrubs the hook environment.

- [ ] **Step 1: Write failing launch/hook tests.** A credits launch records API mode only after key validation; an ordinary launch records subscription. A resumed transcript with older replies only attributes the new launch's replies. A hook missing verified launch facts records nothing rather than assuming subscription. Simulate the scrubbed hook environment using the existing ancestor-environment test pattern.
- [ ] **Step 2: Run** the affected launcher and hook tests and observe their expected failures.
- [ ] **Step 3: Implement** explicit launch metadata and Stop-hook capture without changing the existing card's `credits` flag, API key handling, or session-state reports. Capture failure must be logged with context and leave usage visible as unknown.
- [ ] **Step 4: Run** affected tests. Explicitly flag these launcher/hook edits as host-executed changes in the handoff.

### Task 4: Capture OpenCode funding without reading secrets

**Files:**
- Modify: `config/opencode/.config/opencode/plugins/cockpit.ts`
- Test: `apps/cockpit/tests/opencode_team.rs` and the existing OpenCode plugin harness used there

**Interfaces:**
- Consumes: OpenCode V2 `ctx.session.hook('http.request', ...)` and the native `session.step.started` event. Never read or resolve saved credentials to infer an already-assembled request's billing.
- Produces: metadata for the exact `opencode:<assistantMessageID>` key, with request-bound billing snapped before dispatch and provider/model consistency checked at the native step event. Conflicting retry evidence becomes unknown.
- Native Anthropic HTTPS API-key requests are API; ambiguous Anthropic Bearer requests remain unknown. OpenAI HTTPS API and ChatGPT service routes, with authorization present, distinguish API and subscription service. Other routes/providers are unknown. Neither category is an invoice assertion.
- References: `https://opencode.ai/v2/docs/build/plugins`, `https://opencode.ai/v2/docs/providers`, and the installed 2.1.3 runtime. Confirm native timing against a local mock provider without a paid request before claiming coverage.

- [ ] **Step 1: Write failing real-plugin harness tests.** Cover API versus plan routes, two providers in the same session, a billing switch across messages, missing/mismatched native reply IDs, conflicting retries, child sessions, and metadata write failure. Assert emitted metadata contains no credentials and does not modify requests or responses.
- [ ] **Step 2: Run** those plugin tests and confirm failure because the capture hook is not registered.
- [ ] **Step 3: Implement** the request snapshot/native-step binding and synchronous atomic metadata publication using Node standard-library APIs. Preserve the event stream and cleanup behavior; release the added hook on cleanup. Uncertain cases must not generate a guessed billing label.
- [ ] **Step 4: Run** plugin tests and the full repository test command when authorized. Perform an isolated installed-runtime check against a local mock provider, validating the persisted evidence key against the native assistant row, without a paid API request.

### Task 5: Approved terminal presentation

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/costs.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** `Costs::billing_totals()` includes recorded/unpriced reply counts; `Costs::is_loading()` distinguishes the initial scan from no recorded usage. Reuse the existing header, Status rows, money formatter, wrapping, terminal styles, and render fixtures.

- [ ] Write and watch failing tests for the API-only header, historical-only unknown billing, all three categories including zero values, narrow wrapping, column overlap with large amounts, no usage, loading, unpriced estimates, and archive errors.
- [ ] Implement the exact approved caption, category labels, explanatory notes, and All-time API value rename from `DESIGN.md`. Keep header height, quotas, Active time, and health ordering unchanged.
- [ ] Run the eight focused spending/header render tests, then the full repository suite. Compare against ready frames 2 and 4; mark only those frames completed after verification.

### Finish and presentation handoff

- [ ] Run `git diff --check`; show `git diff --stat`.
- [ ] Run the project test command, `make test`, only with current-message authorization for its build step; otherwise request that authorization rather than substituting an unreported partial suite.
- [ ] Integrate the Designer-approved labels/placement in a separate presentation task using `Costs::billing_totals()`. Preserve the existing combined total and expose unknown coverage; do not call the API bucket an invoice.
- [ ] Commit only verified requested changes on the feature branch, with `Todo: f17bb5e5` when the whole requested split is complete.
- [ ] Use `cockpit-team handoff` for fresh QA before applying. Export and request the reviewed host apply using the repository's `CLAUDE.md` workflow. Do not mark the feature complete while UI, verification, or QA is pending.
