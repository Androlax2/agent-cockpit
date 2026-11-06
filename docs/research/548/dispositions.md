# Decision record dispositions (#548)

Appendix to [the decision-records research](../2026-11-06-548-decision-records.md). One row per
file under `docs/decisions/` as of `81f41047` (165 records). Tests and criteria are defined there.
The reasons are condensed from four independent read-through reviews; each cites a path or quote
from the record or the code. "Inbound" counts files outside `docs/decisions/` that name the record
(code comments, workflows, docs); every one must be repointed if the record moves.

| Record | Kind | Disposition | Reason | Inbound |
| --- | --- | --- | --- | --- |
| `2026-10-31-119-crate-graph-allow-lists` | ARCH | KEEP | Why allow-list beats forbidden edge ("a crate added later is forbidden until a decision names it") and the rejected alternative are in no comment. | 0 |
| `2026-10-31-127-platform-build-cache` | POLICY | MERGE into `2026-11-01-397-ci-cost.md` | Cache rules now govern three Linux jobs under 397; title and macOS table are stale. One CI-cache record, WSL2-uncached reasoning kept. | 1 |
| `2026-10-31-144-sessions-idle-config` | POLICY | KEEP | Fail-the-load vs legacy fallback and owner-only scope are not stated elsewhere. | 0 |
| `2026-10-31-146-reports-and-restart-all` | ARCH | KEEP | Multi-point record, all still true; rejected automatic restart and dropped-rows migration are unique. | 0 |
| `2026-10-31-156-approval-safety` | BEHAVIOR | KEEP | Guards against reopening grace/warning design; README carries the what, not the rejected `too-soon` code or storing warnings. | 0 |
| `2026-10-31-157-session-robustness` | ARCH | KEEP | Resume/restart takeover rules are live; its finished-label bullet is amended (cross-reference 273). | 0 |
| `2026-10-31-163-approvals-processes` | ARCH | KEEP | Merge target for 34-runs; the reservation-atomicity reason and rejected callback composition are not in code. | 0 |
| `2026-10-31-164-read-only-polls` | BEHAVIOR | KEEP | Merge target for 174; threshold choice and rejected "bump every poll" with measurement are unique. | 0 |
| `2026-10-31-174-read-first-status-polls` | IMPL | MERGE into `2026-10-31-164-read-only-polls.md` | Same subject and issue lineage as 164 (poll cost); small, so fold in as one "status polls" record. | 0 |
| `2026-10-31-182-question-sources` | BEHAVIOR | MERGE into `2026-11-01-213-question-resets.md` | Three records tell one story of the open-question lifecycle; fold into a single current-state record, keep #200 rejected options. | 0 |
| `2026-10-31-183-turn-outcome` | ARCH | KEEP | Distinct topic (outcome, not question); rejected "outcome independent of state" is unique. Note 213 amendment. | 0 |
| `2026-10-31-184-claude-launch-without-background-daemon` | ARCH | KEEP | Evidence-based security rationale (token inherited by shared daemon) appears nowhere else. | 0 |
| `2026-10-31-185-hook-bridge-bundle` | IMPL | MERGE into `2026-11-01-203-hook-bridge-check.md` | Both cover how the hook bridge ships and is verified; keep rejected `libexec/` and smoke-run reasons in one record. | 0 |
| `2026-10-31-197-tmux-last-session-close` | IMPL | KEEP | Merge target for 202; server.rs doc comment points here, so it is the anchor for "why" of the wait. | 1 |
| `2026-10-31-199-change-feed-projector` | ARCH | KEEP | Theo's decision with rejected alternatives; READMEs only point at it. | 2 |
| `2026-10-31-212-claude-rate-limit-is-a-failure` | ARCH | KEEP | README gives the rule in one line; the 2.1.316 ambiguity evidence and rejected text-matching are only here. | 0 |
| `2026-10-31-214-dialog-stacking` | BEHAVIOR | MERGE into `2026-11-01-213-question-resets.md` | Third step of the same question lifecycle; fold into the consolidated record. | 0 |
| `2026-10-31-217-stop-lane` | ARCH | KEEP | Merge target for 230; sizing reason (answer within TUI 30 s deadline) is unique. | 0 |
| `2026-10-31-219-claude-reports-bound-to-conversation` | ARCH | KEEP | Security-relevant boundary with measurements and rejected `SessionStart`-source acceptance; README gives only the outline. | 0 |
| `2026-10-31-221-end-stops-surviving-agent` | ARCH | KEEP | Distinct state-machine choice with rejected alternative; could be read together with 157/230 but is not a fragment of them. | 0 |
| `2026-10-31-226-finished-since-latest-launch` | BEHAVIOR | MERGE into `2026-11-01-269-working-launches-drop-finished.md` | Mechanism reversed by 273 (`finished_at` stays a time for people; the label no longer reads it). Fold 226 and 273 into 269 as one story, marking the reversal. | 0 |
| `2026-10-31-230-stop-refuses-during-launch` | ARCH | MERGE into `2026-10-31-217-stop-lane.md` | Same stop-lane rule; one record "how stops behave: own lane, refused during a launch". | 0 |
| `2026-10-31-268-agent-layout-variables` | ARCH | MERGE into `2026-11-01-303-agent-display-variables.md` | One "which daemon variables an agent inherits" record; 303 already cites 268. | 0 |
| `2026-10-31-270-self-run-commands-grant` | ARCH | MERGE into `2026-10-31-31-self-state-grant.md` | 270 says "exactly like change-state"; one "self permissions" record with 284 reads cleaner. | 1 |
| `2026-10-31-27-wsl2-proven-in-ci` | POLICY | KEEP | Cited by ci.yml, release.yml; Theo's no-Windows-machine decision and pins live nowhere else. | 3 |
| `2026-10-31-30-tmux-observation` | ARCH | KEEP | README states the behaviours; measurements and rejected `env -u`/`-e TMUX=` are only here. | 1 |
| `2026-10-31-31-self-state-grant` | ARCH | KEEP | Merge target (self permissions); option B/C rejection is unique. | 2 |
| `2026-10-31-31-session-lifecycle` | ARCH | KEEP | Foundational multi-topic record; only the apps/ citation will rot at cutover. | 0 |
| `2026-10-31-33-provider-contract` | ARCH | KEEP | Capability-contract rationale and rejected adapter-calls-tmux are unique. | 0 |
| `2026-10-31-34-approvals-dependencies` | DEP | KEEP | Dependency decisions are in scope; Theo's "option A, no hand-written replacements" and feature additions are not in the manifest. | 2 |
| `2026-10-31-34-approvals-runs` | ARCH | MERGE into `2026-10-31-163-approvals-processes.md` | Single "how approved runs execute" record; leader re-exec, shell, safe-Git, input rules still hold. | 0 |
| `2026-10-31-35-basic-tui` | ARCH | KEEP | Rest still holds (#176 edge, #178 pane region to M6); mark done already in text. | 0 |
| `2026-10-31-36-latency-workload` | IMPL | RELOCATE to crates/cockpit-daemon/tests/latency.rs.md | Test-harness technique already described in the header; add the three rejected alternatives there, and repoint the gate-doc link. | 2 |
| `2026-10-31-38-tasks-filing-rules` | ARCH | KEEP | Rejected `--as-agent`, hard-coded `dotfiles` and separate memory table are only here. | 0 |
| `2026-10-31-41-messaging-outbox-and-feed` | ARCH | KEEP | README restates the what; rejected polling/decoding/sequence counter and `retained_after` reason are unique. | 0 |
| `2026-10-31-42-resource-admission` | ARCH | KEEP | Cited by report 16; rejected in-memory permits and legacy Option C. | 1 |
| `2026-10-31-94-release-policy` | POLICY | SUPERSEDE by `2026-11-01-389-release-channels-follow-milestones.md` | Headline (alphas until M5, stable from M5) was reversed by 389 ("No stable release and no 1.0 planned"). | 0 |
| `2026-10-31-96-nightly-builds` | POLICY | MERGE into `2026-10-31-96-release-tooling.md` | 787 bytes, duplicated bullet in 96-tooling; keep "fourth tag shape" rejection there; update release.yml pointer. | 1 |
| `2026-10-31-96-release-tooling` | POLICY | KEEP | Cited by RELEASING.md, release.yml, cliff.toml; core rationale holds, stale details should be annotated. | 4 |
| `2026-11-01-100-first-run-checklist` | BEHAVIOR | KEEP | Rejected wizard and marker file; sandbox-consent-as-config-line choice not in code. | 0 |
| `2026-11-01-196-profiles` | ARCH | KEEP | Two-issue lane decision; only the variable rationale is duplicated in code. | 1 |
| `2026-11-01-202-tmux-new-session-on-an-exiting-server` | IMPL | MERGE into `2026-10-31-197-tmux-last-session-close.md` | One tmux "exiting server" story; both server.rs comments then point to one file. | 1 |
| `2026-11-01-203-hook-bridge-check` | ARCH | KEEP | Merge target for 185; option rejected (header check) and release-install tie-in unique. | 0 |
| `2026-11-01-213-question-resets` | BEHAVIOR | KEEP | Merge target for 182 and 214; current-state record for the open-question lifecycle. | 0 |
| `2026-11-01-215-asked-still-working-mark` | BEHAVIOR | MERGE into `2026-10-31-35-basic-tui.md` | Five-line TUI mark rule; only the rejected option is not in the README. Fold into the basic-TUI record. | 0 |
| `2026-11-01-252-interrupted-restart-kind` | ARCH | KEEP | Cited from sessions README. Rejected `launching_for` column and "only terminations overlap" measurements appear nowhere else. | 1 |
| `2026-11-01-257-flaky-test-signals` | IMPL | RELOCATE to crates/cockpit-daemon/tests/common/mod.rs.md | Test-wait technique; code already says why. Move the two Rejected bullets next to `once_logged`/capture.rs. | 0 |
| `2026-11-01-269-working-launches-drop-finished` | ARCH | KEEP | Anchor of the finished-label story; "upkeep vs working launch" and rejected B/C are in no code or README. | 0 |
| `2026-11-01-272-launch-step-timeout` | ARCH | KEEP | Anchor for the blocking-step story; the 30 s bound at one helper and "database waits unbounded" are unrecorded elsewhere. | 0 |
| `2026-11-01-273-finish-ordered-by-commits` | ARCH | MERGE into `2026-11-01-269-working-launches-drop-finished.md` | Direct continuation of 269's column; one story: finished label ordered by commits. 226 should join the same entry. | 0 |
| `2026-11-01-279-launch-root-wait` | ARCH | SUPERSEDE by `2026-11-01-316-root-before-agent.md` | Daemon-side 5 s wait was removed; 316 states "The daemon no longer waits for a root". Keep history, mark superseded. | 0 |
| `2026-11-01-284-restore-grants-self-permissions` | ARCH | MERGE into `2026-10-31-31-self-state-grant.md` | Same grant family as 270, which also folds into 31-self-state-grant. | 0 |
| `2026-11-01-285-asks-of-a-session-that-may-no-longer-ask` | ARCH | KEEP | Anchor; "no new crate edge, rows stay asking" is not in the README. | 0 |
| `2026-11-01-300-tui-e2e-harness` | IMPL | RELOCATE to crates/cockpit-tui/tests/e2e/harness.rs.md | Test-harness design; keep the test-owned tmux server, example daemon and crate-graph reasons as doc comments, drop stale bullets. | 0 |
| `2026-11-01-303-agent-display-variables` | BEHAVIOR | KEEP | Anchor of the desktop-session story; "never invent a default, fix the unit" is non-obvious. | 0 |
| `2026-11-01-305-desktop-session-opt-out` | BEHAVIOR | MERGE into `2026-11-01-303-agent-display-variables.md` | 303 already links it as the next step; one record "desktop session variables, opt-out, import". | 0 |
| `2026-11-01-307-desktop-session-import` | BEHAVIOR | MERGE into `2026-11-01-303-agent-display-variables.md` | Same story; in-memory-not-stored reasoning is short. | 0 |
| `2026-11-01-308-tmux-config-on-a-running-server` | ARCH | KEEP | Code points to it as `Why:`; the rule that new options must be checked live-safe lives only here. | 1 |
| `2026-11-01-313-verification-wiring` | ARCH | KEEP | README gives the what; the dirty-digest approach, `/bin/sh` and owner-only reasoning and known gaps are only here. | 0 |
| `2026-11-01-316-root-before-agent` | ARCH | KEEP | Current rule; file-not-FIFO and every-hosted-launch-held reasons absent from README. | 0 |
| `2026-11-01-317-pane-asks-for-tmux-keys` | ARCH | KEEP | Anchor of the approval-pane-keys story; rejected "daemon selects on non-empty queue" is unrecorded elsewhere. | 0 |
| `2026-11-01-324-terminal-open-fence` | ARCH | MERGE into `2026-11-01-272-launch-step-timeout.md` | Same issue family, same test file; rejected options A/B are short. | 0 |
| `2026-11-01-325-blocking-step-permits` | ARCH | MERGE into `2026-11-01-272-launch-step-timeout.md` | Permit bound is a continuation of the 272 helper; fold as dated entry. | 0 |
| `2026-11-01-326-artifacts-never-follow-links` | DEP+ARCH | MERGE into `2026-11-01-40-verification-receipts.md` | It replaces one rule of 40; read together as one record on artifact fingerprinting, with 40's clause marked amended. | 1 |
| `2026-11-01-330-tui-workflow-view` | BEHAVIOR | MERGE into `2026-10-31-35-basic-tui.md` | Waive-only completion is already in 39; remaining content is a key binding and refresh cadence. | 0 |
| `2026-11-01-333-closed-item-keeps-its-run` | ARCH | KEEP | Cited by README; rejected "cancel the run when its item closes" is only here. | 1 |
| `2026-11-01-336-macos-process-trees` | ARCH/DEP | KEEP | No-unsafe/no-dependency choice, whole-second tolerance and fail-closed clock step are absent from code comments. | 1 |
| `2026-11-01-337-tui-grace-outlasts-daemon` | BEHAVIOR | MERGE into `2026-11-01-317-pane-asks-for-tmux-keys.md` | One constant and one rejected option; part of the approval-pane grace story. | 0 |
| `2026-11-01-338-tmux-viewer-sized-before-attach` | IMPL | KEEP | Cited from server.rs; the rewrap evidence and rejected "start at viewer size" are not in code. | 1 |
| `2026-11-01-345-keys-return-to-viewer` | ARCH | MERGE into `2026-11-01-317-pane-asks-for-tmux-keys.md` | Completes 317's "take the keys" with "give them back"; one story. | 0 |
| `2026-11-01-347-show-verbs-read-dead-asks-as-expired` | ARCH | MERGE into `2026-11-01-285-asks-of-a-session-that-may-no-longer-ask.md` | Applies 285's rule to owner reads; both rejected alternatives are single sentences. | 0 |
| `2026-11-01-354-config-reload` | BEHAVIOR | KEEP | Theo decision with rejected alternatives; invalid-file-keeps-old-settings rule not stated elsewhere. | 0 |
| `2026-11-01-356-crossterm-dev-tty` | DEP | KEEP | Reason (edge-triggered mio reader drops a key after resize) is invisible in Cargo.toml and the README. | 0 |
| `2026-11-01-385-placement-delivery` | ARCH | KEEP | Large, load-bearing, held-reservation and never-repeat-typing reasoning unique. Candidate to fold with 43 at the family pass. | 0 |
| `2026-11-01-389-release-channels-follow-milestones` | POLICY | KEEP | Cited by RELEASING.md; 94's "stable from M5" should be marked superseded by it. | 0 |
| `2026-11-01-397-ci-cost` | POLICY | KEEP | Stays the CI-cost record (cited by ci.yml, CONTRIBUTING). Absorbs 127 and 397-draft-ci; the "no CI on draft pull requests" rejection is marked reversed. | 4 |
| `2026-11-01-39-workflow-presets-and-requirements` | ARCH | KEEP | Presets-in-code and "requirements met by verification or waived, never claimed" are the boundary with `cockpit-verification`. | 0 |
| `2026-11-01-408-codex-clear-reports` | ARCH | KEEP | README states the mechanism; "ledger grants no authority" and rejected successor-claims-predecessor are decision content. Weak. | 0 |
| `2026-11-01-40-verification-receipts` | ARCH | KEEP | Freshness computed at read, receipt identity rules and sha2 pin are not elsewhere. Mark symlink clause amended by 326. | 1 |
| `2026-11-01-410-coordinator-kit` | ARCH | KEEP | Many distinct decisions and cited by 713; heavy README overlap on mechanics but terms, audit and cap rules unique. | 2 |
| `2026-11-01-411-optional-providers-in-readiness` | BEHAVIOR | KEEP | README gives the rule; rejected "missing-prerequisite for each provider" is only here. Weak. | 0 |
| `2026-11-01-416-opencode-provider-endpoint` | ARCH | MERGE into `2026-11-01-431-provider-delivery-context.md` | 431 header already lists #416 and owns the retained launch metadata; fold as an entry. | 0 |
| `2026-11-01-419-brief-delivery-per-harness` | ARCH | KEEP | Anchor for #419; per-launch brief read and rejected system-prompt flag are unrecorded elsewhere. | 1 |
| `2026-11-01-419-native-skill-per-harness` | DEP | MERGE into `2026-11-01-419-brief-delivery-per-harness.md` | Brief record says "Not done here: native skill"; this closes it, same issue. Keep harness-doc citations. | 2 |
| `2026-11-01-427-bug-reports` | ARCH | KEEP | Claim cannot prove owner, opened-not-sent stays draft, daemon never touches network: absent from README. `crate_graph.rs:295` has the rule. | 0 |
| `2026-11-01-431-provider-delivery-context` | ARCH | KEEP | Persisting reach and original provider command per execution; lock held until blocking work returns. `delivery_context.rs` and schema tests exist. | 0 |
| `2026-11-01-435-desktop-endpoints-and-session-directory` | ARCH | KEEP | Rule "mount exactly the sockets variables name, never the owner's bus" still holds and is reused in research 546:360. Amend Greywall mechanism. | 0 |
| `2026-11-01-439-pane-waits-for-the-column-keys` | BEHAVIOR/IMPL | RELOCATE to crates/cockpit-tui/src/pane.rs.md | Rationale is in module docs and test names. Only "count, not a flag" reason is record-only; add one line to `pane.rs`. | 0 |
| `2026-11-01-43-scheduling-placements` | ARCH | SUPERSEDE by `2026-11-01-385-placement-delivery.md` | First and last bullets reversed by 385; rest duplicated by scheduling README and report 10. Mark superseded, link 385. | 0 |
| `2026-11-01-442-provider-sockets-opened-by-their-listener` | ARCH | KEEP | Rejected daemon-owned socket dir and `O_NOFOLLOW` alone; pid-reuse gap (#448) recorded only here and `server.rs:205`. | 0 |
| `2026-11-01-443-provider-sign-in` | ARCH | KEEP | "Ask the harness, never infer credential files"; rejected startup cache and raw error output. Trim version-provenance paragraph; READMEs hold it. | 0 |
| `2026-11-01-446-codex-socket-folder-mounted-from-the-session` | ARCH | KEEP | Principle holds: session's own folder at Codex path, source out of agent reach, never share host dir. Mark Greywall mechanism superseded by 828. | 0 |
| `2026-11-01-471-bug-reports-in-the-tui` | BEHAVIOR | MERGE into `2026-11-01-427-bug-reports.md` | Small UI choices (aggregate card, `b`/`B`, plain text until #465) of the same subsystem; one bug-report story across 427/471/847. | 0 |
| `2026-11-01-476-release-engine-pins` | DEP | REMOVE | Pins file and installer deleted (`ls crates/cockpit-sandbox`: no engine-pins.json). 828 names this record as superseded. | 0 |
| `2026-11-01-47-opencode-server-and-plugin` | ARCH | KEEP | README gives the what; Rejected alternatives (shared `opencode serve`, writing `~/.config/opencode`, HTTP crate) only here. Drop stale last bullet. | 0 |
| `2026-11-01-48-codex-delivery-through-a-private-app-server` | ARCH | KEEP | Option A of #407: private app-server; rejected shared server, outside-sandbox server, `thread/resume` watcher. Hand-rolled 300-line WebSocket reason only here. | 0 |
| `2026-11-01-48-codex-hook-trust-and-honest-capabilities` | ARCH/DEP | MERGE into `2026-11-01-48-codex-delivery-through-a-private-app-server.md` | Same issue and adapter. Keep rejected bypass-trust and `~/.codex/config.toml` writes; drop stale capabilities bullet. | 0 |
| `2026-11-01-49-agent-context-served-by-daemon` | ARCH | KEEP | Rejected CLI embedding bundle (drift, allow-list) and per-provider files; `crate_graph.rs:271` rule exists. | 0 |
| `2026-11-01-50-readiness-report` | ARCH | KEEP | Exit-1-only-on-blocking and rejected per-crate `readiness()` / CLI-side report not in README. Annotate two amended bullets. | 0 |
| `2026-11-01-61-sandbox-backend` | ARCH | SUPERSEDE by `2026-11-05-828-remove-sandbox-engines.md` | Greywall, pin and client-name bullets are dead; "refuse never weaken", evidence-in-launch-transaction survive in 828. Inbound: research 546:522. | 1 |
| `2026-11-02-102-documentation-generator` | DEP | KEEP | Why Astro+Fumadocs over mdBook/Starlight/Next.js is nowhere else. Trim approval log, validation and intended-implementation sections. Inbound: site/README. | 4 |
| `2026-11-02-120-unpinned-internal-paths` | DEP | KEEP | Reason (versioned path deps block prerelease resolution, stamping rewrites manifests) not in code. Alternative: expand the deny.toml comment. | 0 |
| `2026-11-02-191-candidate-release-gates` | POLICY | RELOCATE to docs/RELEASING.md#candidate-checks.md | 14 KB is a PR log (host approvals, run IDs, review notes). Durable: archive-based gate, no fallback baseline; RELEASING.md already has most. | 0 |
| `2026-11-02-192-exact-prerelease-proposals` | POLICY | KEEP | Option A verified-snapshot boundary, base-only channel identities, rejected stable-shaped drafts. Merge target for 192-history. | 2 |
| `2026-11-02-192-history-preserving-proposals` | POLICY | MERGE into `2026-11-02-192-exact-prerelease-proposals.md` | Fresh branches, retain history, rejected `release-pr`/force-with-lease are durable; "Generated Markdown correction" and review-provenance sections are task log. | 0 |
| `2026-11-02-192-release-app-setup` | POLICY | RELOCATE to docs/RELEASING.md#release-app-setup.md | Setup procedure already in RELEASING.md; only the "Rejected: PAT, broader scope, silent secret replace" line needs adding there. | 0 |
| `2026-11-02-339-product-name` | BEHAVIOR | KEEP | Theo's naming decision, no-domain rule, rejected names and Dorlek backup; `README.md` now says Torlek. | 1 |
| `2026-11-02-397-draft-ci` | POLICY | MERGE into `2026-11-01-397-ci-cost.md` | Same issue; reverses one bullet of the CI-cost record. Fold as dated amendment so a reader sees one policy. | 1 |
| `2026-11-02-417-opencode-event-capture` | IMPL | REMOVE | Fixture README and PROVIDER_CONFORMANCE row carry every point; this is test-harness technique, not a product decision. | 1 |
| `2026-11-02-419-native-input-before-signed-in-validation` | IMPL | REMOVE | Evidence-scope write-up; fixture README holds receipts, binaries, limits. No architectural choice beyond harness setup. | 0 |
| `2026-11-02-433-sandbox-observations` | IMPL | SUPERSEDE by `2026-11-05-433-live-enforcement-ci.md` | Greywall-era live runner, `engine-pins.json` and macOS notes dead; bounds survive in README. Inbound: research 546:523. | 1 |
| `2026-11-02-436-approval-routing-evidence` | IMPL | RELOCATE to crates/cockpit-codex/README.md | Only record-only decision: production delivery unchanged, no persistent approval owner. Add that sentence to README; fix gates link. | 2 |
| `2026-11-02-436-codex-0166-pin` | DEP | REMOVE | `CONFORMANCE_VERSION = "0.166.0"` in `lib.rs:30`; the pin-only-measured policy is in README and fixture README. Theo's Option B lives in #436. | 1 |
| `2026-11-02-45-completion-attempt` | ARCH | KEEP | Why agent requests must carry step+attempt and clients never refresh identity is absent from code and report 06. | 0 |
| `2026-11-02-470-managed-engine-install` | DEP | REMOVE | Installer, `engines/` layout and tar/flate2 deps removed; no `install` module in `cockpit-sandbox/src`. 828 states it. | 0 |
| `2026-11-02-493-archive-tui-smoke` | IMPL | REMOVE | 835 bytes; the check is the code. Rejected "open tmux manager" is self-evident from the headless smoke. | 0 |
| `2026-11-02-494-syntax-check-only-in-parse-only-shells` | BEHAVIOR | KEEP | Computed-not-stored `unchecked_shell` and rejected `/bin/sh -n` fallback / refuse-all are only here. Theo decision. | 0 |
| `2026-11-02-495-managed-greyproxy` | DEP | REMOVE | Managed proxy, ports 43080-43053, service units all deleted. Remaining value is old-state cleanup, documented in 828. | 2 |
| `2026-11-02-523-tmux-minimum` | DEP | KEEP | Reasons for rejecting 3.6-rc/next-3.6 and any lifecycle workaround, plus Ubuntu 26.04 CI choice. Linked from DEVELOPMENT.md. | 1 |
| `2026-11-02-537-stop-diagnostics` | ARCH | KEEP | Evidence limits and rejected zombie-ignoring/extended timeouts; core rule is in module doc, rest is not. Related story with 523. | 1 |
| `2026-11-02-568-alpha-platform-scope` | POLICY | KEEP | Theo's Option A scope decision with rejected macOS/Homebrew promise. Heavily referenced (RELEASING:281, acceptance docs, report 16). | 6 |
| `2026-11-02-98-user-readme` | BEHAVIOR | REMOVE | Task status plus stale name and install-pending notes; legacy-guide split already visible in `docs/legacy/` and USER-GUIDE. | 0 |
| `2026-11-03-406-clone-storage-report` | IMPL/ARCH | MERGE into `2026-11-03-406-target-pruning.md` | Measurement half of target pruning, same descriptor-based traversal; README has the bounds. Keep #764 limit lesson and rejected `du`/path-follow. | 1 |
| `2026-11-03-406-output-attribution` | ARCH | KEEP | Reservation owns attribution, claim-before-effects, pending result as intent not receipt. Not stated elsewhere. Sound; long. | 0 |
| `2026-11-03-406-target-pruning` | ARCH | KEEP | Immutable owner plan, mount proof before effects, abandon-without-effects. Referenced by projects README and report 16. Merge target for clone-storage-report. | 2 |
| `2026-11-03-44-task-handoff` | ARCH | KEEP | Rejected alternatives and "one placement transaction" rule are not stated in code; evidence tests exist. TUI key list is behaviour detail. | 1 |
| `2026-11-03-562-arch-packaging-alpha1` | POLICY | MERGE into `2026-11-06-562-committed-aur-recipe.md` | Durable residue is "not a tag prerequisite (circular checksums)"; rest is milestone status and an amended point. Inbound refs from RELEASING.md, release.yml, USER-GUIDE need repointing. | 7 |
| `2026-11-03-563-deb-apt-packaging` | DEP/POLICY | KEEP | Target of the deb merge; Rejected: dpkg-buildpackage / one suite are only here. Fold Nov-6 record in as a dated entry. | 2 |
| `2026-11-03-565-scoop-bucket-winget-needs-native-launcher` | POLICY | KEEP | Target of the Windows merge; rejected installer/uninstaller scripts and WSL default-distro fallback are only recorded here. | 1 |
| `2026-11-04-406-database-retention` | ARCH | KEEP | Referenced from 4 crate READMEs and coverage map 16. Settings defaults and batch sizes are code-visible. | 8 |
| `2026-11-04-406-memory-bounds` | ARCH | KEEP | Measurements and seven rejected alternatives are not in code; only coverage map 16 links it. | 1 |
| `2026-11-04-406-prune-verdict-and-unpushed-guard` | ARCH/BEHAVIOR | KEEP | Merge target for 883. Its "Known limit" paragraph is stale after 767/882; add a pointer. | 3 |
| `2026-11-04-406-run-output-retention` | ARCH | KEEP | Reason for "newest run per check" and fenced retirement is not in code; linked from `output_retention.rs`. | 2 |
| `2026-11-04-617-update-notice` | BEHAVIOR/DEP | KEEP | Linked from update.rs; rejected weekly/default-off/prompt/global-hook are not elsewhere. | 1 |
| `2026-11-04-701-session-model-effort` | ARCH/BEHAVIOR | KEEP | No inbound refs but rationale (no Torlek vocabulary, fixed at start) is not visible in tests. | 0 |
| `2026-11-04-702-claude-folder-trust` | ARCH | KEEP | Linked from 4 source files; measured vendor behaviour and rejections are only here. | 4 |
| `2026-11-04-703-claude-message-inbox` | ARCH | KEEP | Merge target for 897. Rejected MCP channel and typing are only here. | 2 |
| `2026-11-04-706-view-stamps-on-the-feed` | ARCH | KEEP | README gives the what; rejected per-view feed kinds and measured wakeups only here. | 0 |
| `2026-11-04-710-worker-questions-to-coordinator` | ARCH | KEEP | Read-time derivation and rejected timer/audit routing are not stated in code. Coordinator family with 703/713/739/897. | 0 |
| `2026-11-04-713-coordinator-ends-workers` | ARCH | KEEP | Why "finished label, not PR/outcome" is not in code. Part of coordinator family. | 1 |
| `2026-11-04-714-agent-edits-own-items` | ARCH | KEEP | Non-obvious audit-size and kind-not-editable reasons are not in code. | 1 |
| `2026-11-04-739-max-active-workers` | ARCH | KEEP | Why per_hour alone fails and why audit counting was rejected are not stated elsewhere. | 0 |
| `2026-11-04-754-per-project-github-token` | ARCH/DEP | SUPERSEDE by `2026-11-05-832-selective-tls-interception.md` | Partial: engine bullets and "Rejected: placeholder through greyproxy" are void. File convention, mode check and variable stripping still hold; mark them as such. | 2 |
| `2026-11-04-767-sweep-on-session-end` | ARCH/POLICY | KEEP | Linked from README and coverage map; rejected merge-tree/GitHub API reasons are only here. | 2 |
| `2026-11-05-433-live-enforcement-ci` | POLICY | KEEP | Different issue and topic from 997 (live enforcement job, not test sharding); the reviewer suggested merging into 997, not adopted. | 0 |
| `2026-11-05-674-network-requests` | ARCH | KEEP | Linked from launcher README; "once" = rest of session and the two-file channel rationale are only here. | 1 |
| `2026-11-05-828-remove-sandbox-engines` | ARCH/POLICY | KEEP | Time-boxed default (returns to `required` at stage 5) but still current. Make the four older records point here. | 0 |
| `2026-11-05-829-native-sandbox-launch` | ARCH | KEEP | Oversized (bundles five issues' decisions). Rationale for hand-off and absent-path rules is not in README. Trim status lines. | 0 |
| `2026-11-05-831-session-proxy-handoff-and-counters` | ARCH/DEP | KEEP | Merge target for 966. Dependency pin choice and rejected in-sandbox relay are not in README. | 0 |
| `2026-11-05-832-selective-tls-interception` | ARCH/DEP | KEEP | Linked from launcher README. Tmux-cannot-pass-fd and two-token reasons are unique. HTTP framing list is IMPL-heavy. | 1 |
| `2026-11-05-845-tmux-socket-by-path` | ARCH | KEEP | README carries the core reason; the two Rejected alternatives (runtime dir loses live sessions; do not create dir) are not. Weak keep. | 0 |
| `2026-11-05-847-opened-bug-report-drafts` | BEHAVIOR | MERGE into `2026-11-01-427-bug-reports.md` | Same subsystem story (427, 471, 847). Keep only Rejected lines (gh fingerprint match, fourth status). | 0 |
| `2026-11-05-848-image-paste-key` | ARCH/BEHAVIOR | KEEP | Linked from `clipboard.rs` and `tmux/server.rs`; the accepted-risk and rejected bridge reasoning is only here. | 2 |
| `2026-11-05-848-tmux-clipboard-copy-out` | ARCH/IMPL | KEEP | Version-conditional config logic and accepted risks are not obvious. Could join the image-paste record as one "clipboard" entry. | 0 |
| `2026-11-05-855-tui-phone-layout` | ARCH/BEHAVIOR | KEEP | tmux read-only breaks `send-keys` and "TUI width wrong once zoomed" are not visible in code. | 0 |
| `2026-11-05-882-sweep-asks-origin` | ARCH | KEEP | Safety rules (trusted origin URL, no fetch) and rejected fetch/GitHub API are only here. | 0 |
| `2026-11-05-883-per-clone-prune-evidence` | ARCH | MERGE into `2026-11-04-406-prune-verdict-and-unpushed-guard.md` | Small, same prune-evidence topic; becomes a dated "per clone" entry. | 0 |
| `2026-11-05-897-owner-send-overtake-note` | BEHAVIOR | MERGE into `2026-11-04-703-claude-message-inbox.md` | Small follow-up on the inbox model; content is the owner-vs-held-message rule of the same mechanism. | 0 |
| `2026-11-06-562-committed-aur-recipe` | POLICY | KEEP | Merge target for the Arch pair; reasoning on reviewed bytes and one workflow is unique. | 1 |
| `2026-11-06-563-apt-publication` | POLICY/ARCH | MERGE into `2026-11-03-563-deb-apt-packaging.md` | Same issue and channel; the older record already forward-links to it for hosting and key custody. | 0 |
| `2026-11-06-565-scoop-validated-in-windows-ci` | POLICY | MERGE into `2026-11-03-565-scoop-bucket-winget-needs-native-launcher.md` | Mostly restated in windows.md; keep only Rejected (file:// URLs, self-hosted runner) and the pinned-commit reason. | 2 |
| `2026-11-06-64-version-all-daemon-and-session-bundles` | ARCH | KEEP | Why the CLI cannot read the ledger and why recorded not probed bundle are not in code. | 0 |
| `2026-11-06-963-doctor-names-an-unapplied-sandbox-setting` | BEHAVIOR | KEEP | Disagreement with the issue's "restart" advice and the #354 link are only here. | 0 |
| `2026-11-06-966-proxy-refusal-hint` | BEHAVIOR/IMPL | MERGE into `2026-11-05-831-session-proxy-handoff-and-counters.md` | Small proxy-behaviour entry; keep client-prints-status-line fact and two Rejected items inside the proxy record. | 0 |
| `2026-11-06-997-sharded-greenfield-tests` | POLICY | KEEP | Merge target for 433. Measurements, rejected archive job and fourth shard, and the e2e `--workspace` build technique are not in ci.yml. | 1 |
