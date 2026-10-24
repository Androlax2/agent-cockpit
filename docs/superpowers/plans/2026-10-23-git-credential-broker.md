# Host-only Git Credential Broker Implementation Plan

Execution status: native implementation approved in this existing session on `feat/fail-closed-git-protection`; parent execution ledger records progress. This does not authorize App setup/grants, real builds/installs, GitHub mutations, rollout or additional agents/sessions. Historical planning handoff notes below describe the earlier review state.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans after Theo approves the complete design and execution method. No implementation or delegation is authorized by the broker-first decision alone.

**Goal:** Keep GitHub write tokens outside every agent while preserving native Git, needed GitHub reads, issue/comment/review/PR/Project workflows and conditionally authorized merges through explicit broker actions.

**Architecture:** The parent host supervisor owns one broker/session socket; a sandbox-local HTTP adapter carries native Git requests without write credentials. A sandbox-only compatibility client preserves necessary `gh` forms: native reads use separately scoped read-only credentials, writes map to typed host actions. The broker pins canonical identity, freshly checks protections per write, enforces project merge/CI policy and a shared principal rate ledger, and revokes capabilities on failure. GitHub rules enforce ancestry; this is not the later local append-only gateway.

**Tech Stack:** Existing Python 3 standard library, Unix sockets, native Git smart HTTP, GitHub REST, keyring and Greywall; no dependency manifest changes.

**Spec:** `docs/superpowers/specs/2026-10-23-git-credential-broker-design.md` plus its parent `2026-10-23-fail-closed-git-protection-design.md`.

## Global Constraints

- Broker-first is selected; raw upstream write-token export is rejected, including emergency/compatibility fallbacks.
- Same existing/future Cockpit repository scope and canonical numeric identity as the parent spec; no unrelated GitHub repositories.
- Fresh complete effective protection check for every write; session lease/cache never substitutes for it.
- Preserve normal feature branches, append-only Git pushes, existing CI and the default PR floor; no agent bypass.
- Broker tokens are repo-scoped host-only; inspector authority is separate and never used for forwarding.
- Dedicated GitHub App inspection is selected and required; personal-login/PAT inspection fallback is forbidden. This does not change broker/native read/Project credential roles or authorize App creation, installation, keys, token minting or grant escalation.
- Controlled test-only setup has since been separately approved and exercised: Metadata+Administration read failed on omitted bypass fields; explicitly approved trial-only Administration write passed. Exact evidence is in the runbook/parent ledger. No production access or general grant escalation follows from that trial.
- Theo directed preserving needed read/issue/PR workflows; PR-create/list-only scope is superseded, not a pending acceptance question. Native scoped reads and explicit typed issue/comment/review/Project/PR actions replace generic authenticated writes. Merge support is conditional on existing default-branch project authorization and CI, never this plan's blanket permission.
- Approved timing defaults: scheduled checks every 300 seconds, hard local admission/evidence ceiling 600 seconds, complete inspection deadline 30 seconds. Every upstream mutative request needs its own fresh check; first failure contains immediately, not at 600 seconds. Preserve suspend/resume/supervisor-death controls. Selected App setup/exact grants, other resource limits and forwarding/read/Project grants remain unapproved; no atomic-write guarantee.
- CI migration direction is approved: affected jobs use append-only updates/new branch names and retain protected remote branches. This is planning policy, not authorization to edit jobs, disable them, change GitHub settings or bypass rules.
- No code/build/install/GitHub changes until explicit implementation approval. No new sessions or agents are authorized now.

## Review Focus

- Direct socket clients bypass the adapter, forge repo/session labels or replay revoked capabilities (B1).
- Encoded routes, malformed pkt-lines, chunks and mixed-ref pushes evade HTTP/command validation (B2).
- Legitimate native Git uses chunked uploads or SSH-spelled URLs and must not need a raw token (B3).
- API fields/opaque IDs/GraphQL or timeout retries become alternate writes, duplicate issues/reviews or lost project policy (B4a–d).
- Native read fields, stack/merge checks or shared-principal rate limits are silently lost to an optimistic compatibility shim (B4e/B5).

## File map and prerequisite order

All new names below are proposed. No new daemon/service unit or package is required. Do not duplicate registry/protection evaluation from parent `git-access.py`.

| File | Responsibility |
| --- | --- |
| Create `config/bin/.local/bin/greywall-agents/git-broker.py` | Host socket framing, session capability checks, fixed Git policy and approved action dispatch |
| Create `config/bin/.local/bin/greywall-agents/github-actions.py` | Plain typed repository/collaboration/Project handlers and fixed GraphQL templates; no proxy facade |
| Create `config/bin/.local/bin/greywall-agents/git-adapter.py` | Untrusted sandbox-local smart-HTTP adapter |
| Create `config/bin/.local/bin/greywall-agents/gh-compat.py` | Sandbox-only `gh` command parser: native read delegation, typed write/Project actions, local JSON/jq formatting |
| Generated sandbox shim `~/.cache/agent-shims/bin/gh` | Installed trusted compatibility entry only inside sandbox PATH; never replace human/host `gh` |
| Create `tools/tests/test_git_broker.py` | Host policy/socket tests with synthetic tokens and local fake upstream |
| Create `tools/tests/test_git_adapter.py` | Real Git/HTTP compatibility tests and typed CLI tests |
| Create `tools/tests/test_github_actions.py`, `tools/tests/test_gh_compat.py` | Typed issue/comment/review/PR/Project/merge guards and actual command/field regression fixtures |
| Parent `github-rate.py` / `test_github_rate.py` | Shared principal budget/periodic coalescing, no write-result caching |
| Modify parent `git-access.py`, shared launcher and `greywall_launcher.rs` | Supervisor lifecycle/socket mounts/URL overlay, in parent Tasks 4–5 |
| Test `apps/cockpit/src/tests.rs`, `apps/cockpit/tests/claude_remote.rs` | Preserve current host PR/issue fields, review-request fallback, phone cache and canonical local-mirror association; source changes only if required to preserve that behavior |
| Modify parent `docs/security/git-access-runbook.md` | Compatibility, inspector failures, revocation, uncertain outcomes and containment |

Order: parent Tasks 1–2, dedicated App Task 2a, Tasks 3/3a → B1–B3 and B4a–e (no writable rollout) → parent Tasks 4–6 with B5. B4a–e replace former narrow B4; each has its own test/review cycle. Full review/execution choice, approved App setup/grants and visibility proof, compatibility migration acceptance and Greywall capability are prerequisite gates; settled timings/App architecture do not waive them.

## Proposed interfaces

- Parent `git-access.py inspect --checkout PATH` remains the public CLI check. Add shared in-process entry point `inspect_access(checkout: pathlib.Path, *, purpose: str) -> dict`, with purpose exactly `inspection/admission/scheduled/write` and a secret-free report including cost/principal-resource identities. Broker imports the trusted sibling, never a checkout module. Only `scheduled` can coalesce completed periodic evidence; every `write` reserves a new complete inspection. Client socket cannot select purpose or skip policy.
- `git-broker.py serve --decision PATH --socket PATH`: host supervisor only; reads session/capability/credential binding from hidden host decision state, serves the spec's framed v1 messages and revokes on parent death. No control/mint action on the sandbox socket, and no write, Project or inspector credential ever leaves the host. No credential in argv. **Amendment (Theo, 2026-10-24, option A):** the one exception is bare `credential.read` (no body, no selectors), which returns only the separately enrolled read-role token so native `gh` keeps working past the read App's one-hour token lifetime; an expiring token is refreshed through a funded admission inspection, and a failed refresh refuses the token and contains the session. Accepted limitation: a token already handed out stays valid until it expires (at most one hour) after revocation.
- `git-adapter.py serve --socket PATH --capability-file PATH`: inside sandbox; binds namespace-local loopback port 0 and prints its selected Git URL once through a supervisor-owned readiness channel. Never selects an upstream destination. The capability file is per-session ephemeral sandbox state, never a real GitHub token.
- `gh-compat.py -- ARGS...`: preserves the inventoried native `gh` forms/JSON/exit statuses. Resolves installed real `gh` from trusted host-prepared state, delegates read forms only with the verified read-role credential, or emits one of the spec's typed actions. The host never opens agent `--body-file`/output paths or executes client jq. No mutative fallthrough to a raw-token/native helper.
- `github-actions.py` exposes `perform_action(binding: dict, action: str, fields: dict) -> dict` to the installed broker only; exact action/field/target/response schemas are the spec's table, not arbitrary route forwarding. Drive tests through broker socket and compatibility CLI entry points; do not build service/repository abstractions around plain handlers.
- Wire actions/fields, resource limits, one-request-per-connection framing and fixed upstream routes are exactly those in the spec. The broker ignores no unknown request fields: it rejects them.

### B1: Host channel, capability binding and credential separation

**Files:** new broker/test file; shared verifier entry point in parent helper.

**Interfaces:** produces `serve` framed request validation; consumes parent decision and `inspect_access()`.

- [ ] Write CLI/socket tests `test_missing_or_wrong_capability_denies`, `test_client_cannot_select_repository_or_credential`, `test_duplicate_unknown_and_oversized_fields_deny`, `test_revoked_generation_denies`, `test_sibling_socket_is_not_published`, and `test_upstream_and_inspector_credentials_never_leave_host`. Assert zero upstream calls on denial and absence of synthetic tokens in client/env/log/error output.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_git_broker.py' -v`; expected failures before `serve` exists.
- [ ] Implement one request per connection, exact bounded framing and constant-time capability comparison, typed schema rejection and hidden host binding lookup. Use installed trusted helper paths, restrictive socket/control directories and per-session revocation; never trust peer UID/session text alone.
- [ ] Add cross-session, truncated body, trailing-byte, expired admission, socket replacement and broker restart tests. A new process never accepts an old generation, even at a reused socket path.
- [ ] Re-run suite; expected `OK`. Commit only authorized completed files, otherwise leave the diff.

### B2: Fixed Git policy and per-write checks

**Files:** broker/test file; no host checkout or pack execution.

**Interfaces:** implements `git.advertise`, `git.fetch`, `git.push` with spec-fixed upstream destinations; every write calls `inspect_access()` immediately before forwarding.

- [ ] Add `test_each_push_rechecks_even_inside_session_lease`, `test_failed_check_forwards_no_write`, `test_delete_default_tag_and_duplicate_refs_deny`, `test_malformed_or_unsupported_receive_commands_deny`, `test_redirect_or_client_auth_header_never_receives_credentials`, and `test_large_slow_spool_is_checked_after_upload`. Assert unchanged fake upstream state on denial.
- [ ] Run broker suite; expected new tests fail.
- [ ] Implement pkt-line command-prefix validation and exact receive/upload routes with fixed host authentication, TLS/CA/proxy settings and no redirects. Construct Content-Type/Git-Protocol from allowed typed fields. Spool privately with reviewed size/time/concurrency bounds; no shell, hooks, pack decompression or generic upstream URL proxy.
- [ ] Add fake-upstream tests proving accepted new/nested feature updates reach only the enrolled repository and unsupported actions/ref APIs never do. Model GitHub enforcing non-fast-forward rejection; do not pretend broker prefix validation proves ancestry.
- [ ] Run suite; expected `OK`. Commit only authorized completed files.

### B3: Native Git adapter without credential export

**Files:** new adapter/test file; parent launcher integration fixture and overlay generation.

**Interfaces:** produces namespace-local HTTP Git URL and framed Git actions; consumes only own session socket/capability. Parent supervisor injects trusted URL overlays after clearing inherited overrides.

- [ ] Write real-Git fixture tests `test_native_fetch_and_feature_push_need_no_token`, `test_nested_branch_and_chunked_push_work`, `test_https_and_ssh_spelling_map_only_enrolled_repository`, `test_local_mirror_is_never_a_writable_upstream`, and `test_changed_overlay_cannot_gain_direct_upstream_authority`. Use temporary repositories/local upstream only, never real host credentials or GitHub.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_git_adapter.py' -v`; expected failures before adapter exists.
- [ ] Implement bounded Content-Length/chunked HTTP decoding, exact route/query validation and socket framing. Bind only sandbox-local loopback; no host TCP port or Greyproxy admin API. Add readiness/child lifecycle handling through the supervisor without confusing agent PID discovery.
- [ ] Add adversarial cases for chunk ambiguity, length conflicts, encoded traversal, extra query keys, header injection, compressed bodies, unsupported protocol and fake authenticated upstream errors. Direct hostile framed messages must also fail host tests; adapter validation alone is not evidence.
- [ ] Run both suites; expected `OK`. In parent all-agent launcher tests assert no real GH/GITHUB **write** token or write-capable credential helper is present; a separate proved read-only role may reach native `gh`. Only the own broker socket directory is mounted. Commit only authorized completed files.

### B4a: Preserve reads and typed issue lifecycle/relationships

**Files:** proposed action/compatibility modules, `test_github_actions.py`, `test_gh_compat.py`, parent launcher tests/runbook.

**Interfaces:** produces the spec's repository/issue/label/milestone/type and issue parent/child/blocker actions; consumes B1 socket and parent role-specific enrollment. Native read-role `gh` preserves necessary repo/PR/check/status/run/log/diff fields.

- [ ] Create inventory fixtures from the spec's evidence: Cockpit PR/issue fields, queued `gh issue view`, Opusline `gh issue create --type --label --milestone`, Sentry list/close/reopen with comment. Write `test_native_reads_have_no_write_authority`, `test_issue_types_labels_and_milestone_are_preserved`, `test_cross_repo_opaque_ids_cannot_mutate`, `test_issue_parent_and_blockers_are_not_silently_lost`, `test_partial_pagination_is_not_empty`, and `test_pr_cannot_be_closed_via_issue_edit_bypass`. Assert exact fields/routes, no token escalation and unchanged target on denial.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_github_actions.py' -v` and corresponding `test_gh_compat.py`; expected new failures.
- [ ] Implement typed issue handlers and read delegation with endpoint-minimal grants, target/ID ownership validation and separate read-role token enrollment. Repo issue types use documented Metadata-read `R/issue-types`, not a new organization-admin grant. Existing labels only; verify GitHub did not silently drop required type/milestone/label fields. A composite close/reopen with comment checks each mutative request and reports partial outcome honestly.
- [ ] Add negative fixtures for write token masquerading as read token, missing read credentials, read-only fallback, unknown API fields, arbitrary GraphQL/refs/Contents/admin methods, malicious body-file path and uncertain issue creation. Reconcile creates without automatic duplication; unsupported necessary forms are named migration exceptions.
- [ ] Re-run both suites; expected `OK`. Commit only authorized completed files.

### B4b: PR lifecycle, comments and reviews without CI bypass

**Files:** action/compatibility modules and their tests; broker dispatch/runbook.

**Interfaces:** produces typed PR create/edit/draft/ready/close/reopen/update-branch, comment create/edit and review create/submit/edit/reply actions; consumes B4a verified read/head/target lookups and B1 check-before-write path.

- [ ] Write `test_pr_json_and_discussion_fields_remain_compatible`, `test_review_event_and_inline_comments_pin_current_head`, `test_comment_edit_requires_owned_actor_and_repo`, `test_ready_and_draft_use_only_fixed_templates`, `test_update_branch_is_merge_forward_not_rebase`, `test_checks_statuses_and_review_dismissal_have_no_write_action`, and `test_uncertain_comment_review_or_pr_is_not_replayed`. Assert same GitHub actor, no self-approval workaround, no metadata mutation altering required checks, no temp_clone_token in projected responses.
- [ ] Run the actions/compatibility suites; expected new failures.
- [ ] Implement the spec's exact action/field whitelist; no raw query text. Support body stdin/files, existing metadata deltas and JSON fields without altering parent protections. Review/PR/comment IDs must belong to the same target, reviews bind `commit_id`, owned review/comment edits verify author. Never dismiss required reviews, fabricate a second reviewer or delete refs while closing a PR.
- [ ] Add per-project automation-trigger fixtures (issue_comment/label/review handlers) and cross-repo/fork exceptions. Preserve actual workflow intent but require already approved task/automation authority; do not auto-lend a host token for upstream fork PRs. Every upstream mutative request must invoke a fresh protection check.
- [ ] Run suites; expected `OK`. Commit only authorized completed files.

### B4c: Projects compatibility with host-only board authority

**Files:** action/compatibility modules, their tests, parent registry/token setup/runbook.

**Interfaces:** produces `project.get/items.list/fields.list`, `project.item.add/field_set/move` through fixed templates; consumes explicitly enrolled owner/board/repo/item/field/option IDs, never a client-selected account credential.

- [ ] Write `test_project_workflow_preserves_priority_effort_and_order`, `test_other_repo_items_and_fields_are_not_exposed_or_mutated`, `test_opusline_automatic_pr_transitions_and_manual_triage_are_distinct`, `test_real_project_write_token_stays_host_only`, `test_client_graphql_text_or_extra_mutation_is_rejected`. Assert board reads/field options/item ownership, documented Backlog/Next triage/refill remains supported and manual In progress/Done does not replace automation; ambiguous policy requires enrollment review, not blanket Status denial. No broader account/code authority.
- [ ] Run the actions/compatibility suites; expected failures.
- [ ] Implement only spec-fixed Project templates and typed variables, with per-operation fresh checks. Remove real `GH_PROJECT_TOKEN` export/placeholder path at launcher integration. Prove minimum Project read/write credential grants; a project-only classic credential may remain host-side only when required. Do not broaden a code token or permit new boards/fields/options/deletions.
- [ ] Add partial-page, removed/renamed field, wrong option/predecessor, inaccessible board and cross-repo relationship tests. Name missing authority/features as setup exceptions and block affected workflow cutover, never silently drop project tracking.
- [ ] Re-run suites; expected `OK`. Commit only authorized completed files.

### B4d: Conditional authorized merge/stack compatibility

**Files:** action/compatibility modules, broker tests, parent merge-policy enrollment/runbook.

**Interfaces:** produces `pull_request.merge` / conditional `stack.merge` and fixed queue/status reads. Consumes trusted current canonical-default policy provenance, current member set/base/head, full required CI/reviews and fresh protection inspection; no merge action is authorized by this planning message.

- [ ] Write `test_working_copy_or_message_cannot_grant_merge`, `test_runway_candidate_needs_default_policy_proof`, `test_missing_failed_pending_or_stale_required_ci_denies`, `test_required_review_or_integration_identity_cannot_be_bypassed`, `test_expected_head_change_invalidates_merge`, `test_admin_delete_or_premature_auto_merge_flags_deny`, and `test_stack_merge_is_not_faked_with_sequential_pr_merges`.
- [ ] Run suites; expected failures.
- [ ] Implement the spec's guard and fixed documented ordinary/stack/queue mapping only after protocol review of installed `gh stack v0.1.0` and target GitHub support. For ordinary merge use expected-head CAS; preserve required queue semantics; all stack members need independently satisfied policy/check guards. No `--admin`, branch deletion, review dismissal, bypass_rules, status forging or auto-enqueue before required head CI passes. Queue-generated group checks remain enforced server-side; resolve any pre-enqueue/group-check ambiguity with Theo rather than disabling CI.
- [ ] Add missing/default-policy change, queue unknown, mixed-authority members, uncertain async response and native extension compatibility fixtures. Read/submit append-only stack workflows must pass; pushed rebase/sync conflicts remain explicit migration exceptions for an approved new-branch/append-only alternative. No claim of native stack fidelity or atomicity without actual endpoint/extension proof.
- [ ] Run suites; expected `OK` for supported/reviewed paths and explicit fail-closed exceptions elsewhere. Stop affected merge-flow cutover if semantics cannot be preserved; bring the exception to Theo, not a raw token. Commit only authorized completed files.

### B4e: Consumer compatibility and request/rate acceptance

**Files:** compatibility client/tests, parent shared rate helper/tests, Cockpit/phone read regression tests and runbook.

**Interfaces:** produces fixture-backed `gh` compatibility for required forms and a capacity report using the spec's per-principal formula. Consumes B4a–d action catalog and parent rate ledger; it must not replace existing host human `gh` or account-wide read-review fallback.

- [ ] Write `test_cockpit_json_fields_and_issue_prompt_still_work`, `test_phone_and_digest_keep_native_read_cache`, `test_read_only_gh_api_cannot_promote_to_generic_write_proxy`, `test_jq_body_file_pagination_and_exit_status_stay_client_side`, and `test_unknown_necessary_command_blocks_cutover_visibly`. Use actual inventoried command forms, not handwaved replacement commands.
- [ ] Run the Python suites and, with implementation-stage build approval, existing Cockpit/phone read tests; expect new failures before integration.
- [ ] Integrate sandbox-only shim/native read delegation, role-specific credential setup and canonical local-mirror lookup only where needed to preserve current consumers. Verify host `reviews.rs`/phone requested-review search remains read-only and unmodified in authority; no broad host token enters agents. Preserve error/incomplete/dated read states rather than empty/green success.
- [ ] Test total formula `C*(12R+A+W+D+L)+W+H+M`: 1,660/3,080 full examples, 1,008 scheduled-only at 12 repos, full transfer/event/guard 3,412 and noncoalesced 2,920. Selected App bucket splits are 1,400+260 / 2,660+420 / 2,912+500 before mint overhead. Assert shared installation quota across repo-scoped tokens, independent installation/owner buckets, unchanged same-user PAT pooling and actual mint/header bucket costs. Include GraphQL/native aggregation/pages/long-transfer/merge costs without double-counting submissions. Historical 60-second examples are comparisons. Later approved acceptance measures actual costs; insufficient headroom fails read-only without changing timings, adopting personal-login fallback or inferring App setup/grant approval.
- [ ] Re-run all action/compatibility/rate and relevant host consumer tests; expected success. For the inventoried conflicting CI jobs, require reviewed append-only/new-branch replacements and retained protected remote branches, per Theo's approved policy. Acceptance must cover repeated runs, deploy consumers' branch assumptions, superseded PR closure without branch deletion, no added CI failures/retry loops and unchanged required checks. Track per-job ownership and implementation authority separately; unresolved migrations block affected cutover, not justify a bypass or disabled job. Resolve remaining fork/stack protocol gates explicitly. Commit only authorized completed files.

### B5: Integrated containment and independent acceptance

**Files:** parent supervisor/launcher tests, broker tests and runbook; use parent rollout approval flow.

**Interfaces:** failed broker check revokes capability and notifies host supervisor; all brokers bound to invalidated common-dir/repository identity stop authorizing new writes. Existing in-flight writes report uncertain outcome and require reconciliation.

- [ ] Add fake-clock fixtures for check due at 300 seconds, failure at 300/timeout by 330 triggering immediate containment, and independent 600-second hard local expiry when checker progress stalls. Test fresh scans for each write at 1/299/599 seconds (two writes never share completed evidence), queued writes cannot extend expiry, suspend/resume invalidates and supervisor death stops broker/sandbox. Preserve credential rotation/common-dir invalidation, rate-budget failures and unknown in-flight outcomes; no success renewal from failure.
- [ ] Run all new Python suites and parent launcher tests under implementation-stage build authorization; expected success. Verify agent restart/sleep/wake/process discovery across Claude/OpenCode/Codex with no orphan brokers or duplicate sessions.
- [ ] Obtain approval for real Greywall socket/mount isolation and disposable GitHub tests. Demonstrate native branch/fast-forward push, private PR/check/status/issue reads, issue type/label/milestone/relations, comments/reviews/PR and enrolled Project workflows. Test authorized versus unauthorized merge with expected-head/CI guards only under separately valid test authorization. Reject deletion/default update/non-fast-forward and the next write after inspection/budget failure. Do not claim simulated rules prove GitHub enforcement or interrupted writes rolled back.
- [ ] Complete parent legacy-token revocation and external-writer quarantine before cutover. An old copied PAT bypasses the new broker until GitHub revokes it; closing the new socket is not remediation for that old credential.
- [ ] Obtain an independently reviewed security assessment before deployment; no reviewer/session is spawned without authorization. With explicit build approval run `make test`, retain exact output/failures, show combined diff stat, then stop for rollout approval. Rollback remains enforced read-only.

## Planning handoff

Self-review only: broker-first, workflows, CI append-only/new-branch/retained-branch policy, 300/600/30 timings and dedicated GitHub App inspection are covered; no implementation/tests/build/install/GitHub mutation/new git authority/session. Timing/App fixtures above are planned tests, not implementation files. Most important remaining setup approval: **exact inspector App permissions and selected-repository installation after visibility proof**, not the already-settled principal choice. Full plan/execution approval, other role grants/resources, concrete CI replacements and stack/fork proof still gate rollout. Report to dotfiles `ses_e98198cd4ffez1uHcbITHGzyW6` referencing review `2ab52cd9`, planning session `ses_e97dc8f63ffeiCCB89exbN4RJ9` and all four absolute paths. Check lists, verify delivery and do not assume absent queued reports exist or complete another session's review.
