# Fail-closed Git History Protection Implementation Plan

Execution status: Theo approved native implementation in this existing session, then a dedicated feature branch in the existing clone after native worktree creation failed. Progress/rulings: `.superpowers/sdd/2026-10-23-fail-closed-git-protection/progress.md`. App setup/grants, real builds/installs, GitHub mutations and rollout remain separately gated; no new agents/sessions.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Theo must approve this plan and an execution method first; no delegation or implementation is authorized now.

**Goal:** Gate every Cockpit agent repository's writes on trusted identity and verified all-branch history/default-branch PR protection, retaining enforced read-only access on failure.

**Architecture:** A host-only registry and verifier prepare a per-launch decision, which the shared launcher converts into writable or enforced read-only sandbox settings. A host supervisor owns the mandatory credential broker and bounded revalidation; agents never receive real GitHub write tokens. GitHub additive rulesets protect remote branches; verified Greywall mounts enforce failed-admission read-only access. Theo selected broker-first; the companion broker plan is a blocking dependency, not an optional later enhancement.

**Tech Stack:** Existing POSIX/Bash launchers, Python 3 standard library, Git/GitHub REST, keyring `secret-tool`, Greywall, Python unittest and Rust launcher integration tests. No new packages or dependency manifests.

**Spec:** `docs/superpowers/specs/2026-10-23-fail-closed-git-protection-design.md` (read in full before implementation).

**Required companion:** `docs/superpowers/specs/2026-10-23-git-credential-broker-design.md` and `docs/superpowers/plans/2026-10-23-git-credential-broker.md`. Order: this plan Tasks 1–2, dedicated-inspector Task 2a, Tasks 3/3a → broker B1–B3/B4a–e → this plan Tasks 4–6 and broker B5. Both documents remain subject to full review; architecture/workflow/timing decisions are not implementation or App-installation approval.

## Global Constraints

- Every repository used by an agent through Agent Cockpit, existing or future; not unrelated GitHub repositories.
- Protect every existing and future branch against non-fast-forward history replacement and deletion.
- Require PRs on the default branch, with zero mandatory reviewers in the new baseline; preserve existing CI requirements, reviews and stronger protections.
- Missing, disabled, bypassable, uninspectable or unsupported protections and API/network failures mean enforced read-only, never optimistic writes.
- No agent bypass; no agent Administration or Workflows write grants; never expose host inspection authority.
- Theo selected a dedicated GitHub App inspector, not the personal host login. Exact App permissions, registration/installation and private-key/token operations still require separate approval; failure or missing bypass visibility means read-only, never personal-credential fallback.
- No raw GitHub write-token export in any mode, fallback or compatibility switch. A verified broker is mandatory before writable launch.
- Normal new feature branches and append-only commits/pushes remain usable after verification, subject to existing repository policy and the user's Git authorization.
- Preserve inventory-backed native reads, issue/comment/review/PR and Project workflows through read-only credentials or typed host actions. Merge compatibility requires current canonical default-branch authorization and required CI; no blanket merge permission or generic authenticated tunnel.
- Theo approved requiring affected CI jobs to use append-only updates or new branch names and retain protected remote branches. Preserve job purpose/checks; concrete migrations still need review/testing and separate implementation authority. No silent job disablement or protection bypass.
- A complete local append-only Git gateway is a separate stage. Never claim that protected-mode local history is immutable.
- Approved timing bounds: 300-second scheduled revalidation / 600-second hard local evidence expiry / 30-second inspection deadline. Every upstream write requires a fresh complete check; any failed scheduled check immediately contains. Other resource limits and the selected inspector App's setup/exact grants are not approved.
- This plan authorizes no installs, builds, GitHub changes or Git writes. Implementation commits only on an authorized feature branch; no automatic pushing or merging.

## Review Focus

- A renamed/recreated repository or malicious remote/include must not borrow another project's identity or execute host code (Task 1).
- A successful-looking API response with hidden bypass actors or missing pagination must never enable writes (Task 2).
- Read-only mounts must defeat rename, shared gitdir and daemon/socket-mediated mutation, not just ordinary Git commands (Tasks 3–4).
- A dead supervisor, sleeping machine or expired evidence must not leave a managed writable sandbox alive (Task 5).
- Legacy tokens, sleeping sessions and Docker binds survive process restarts; migration must not call them revoked or isolated without evidence (Task 6).

## File map and proposed interfaces

New names below are **proposed**, not existing APIs. Keep one host helper rather than adding services/repository abstractions.

| File | Responsibility |
| --- | --- |
| Create `config/bin/.local/bin/greywall-agents/git-access.py` | Host registry, sanitized identity resolution, REST checks, decisions, supervisor; Python standard library only |
| Create `tools/tests/test_git_access.py` | Helper CLI tests with fake GitHub/keyring/clock/child processes; no real credentials/network |
| Create `tools/tests/test_agent_github.py` | Existing admin script's additive ruleset/enrollment/check entry points using fake host commands |
| Create `config/bin/.local/bin/greywall-agents/github-rate.py` | Shared principal/resource budget ledger and periodic-inspection coalescing; no cached write admission |
| Create `tools/tests/test_github_rate.py` | Multi-process quota/poll/queue tests; budget arithmetic and fail-closed exhaustion |
| Modify `tools/agent-github.sh` | Approved enrollment/protection remediation; replace unsafe name-based overwrite |
| Modify `config/bin/.local/bin/greywall-agents/launcher-common.sh` | Credential scrub, gate decision and sandbox settings/channel isolation |
| Modify `config/bin/.local/bin/greywall-agents/{claude,opencode,codex}` | Final sandbox invocation through the host supervisor, preserving each agent's arguments |
| Modify `apps/cockpit/tests/greywall_launcher.rs` | All-agent gate integration; extend existing `Launch` fixture |
| Modify `config/greywall/.config/greywall/greywall.json` | Hide registry/runtime/inspector state and protect them from writes |
| Modify `config/claude/.claude/hooks/agent-environment.sh` | Report actual verified/read-only mode and reason, not token-presence guesses |
| Modify `Makefile` | Accurate check/protect help and explicit host enrollment target; no launch-time remediation |
| Create `docs/security/git-access-runbook.md` | Migration inventory, approval sequence, containment, credential limitations, rollback |
| Conditional `packages/greywall/build.sh` | Approved verified prerequisite pin/checksum only, after Greywall coordination; do not guess a new pin |

Host registry: `~/.config/greywall-agents/git-repositories.json`, version 1 as specified. Runtime decisions: `$XDG_RUNTIME_DIR/greywall-agents/git-access/`, hidden from agents. Individual public reports contain no secrets and bind checkout/gitdir/common-dir identities, repository ID, canonical name, baseline IDs, role-specific credential fingerprints, mode, reason, evidence timestamp, measured inspection cost and configuration digest. Merge authority additionally binds canonical default policy source/blob; board authority binds enrolled board/content/field IDs. Private capability state is distinct from secret-free inspect output. They are host-owned and atomically replaced; stale files cannot authorize a launch.

Proposed helper CLI:

- `git-access.py enroll --checkout PATH --repository OWNER/NAME`: host-only reviewed enrollment, exits 0 only after confirmation and complete inspection; never derives approval from the checkout name. Invoked only from explicit admin tooling.
- Enrollment also requires explicit `--project`, `--repository-id`, `--inspector-app-id`, `--history-ruleset`, `--default-pr-ruleset`; the App/owner/repository-ID mapping must already be in host-reviewed `inspector_apps` within the same registry. No CLI credential/path override or basename-derived project authority. Confirmation is followed by a second fresh remote check before an atomic update; a changed registry aborts. These explicit fields resolve the original plan's missing bootstrap inputs without granting production setup authority.
- `git-access.py inspect --checkout PATH`: read-only JSON report; exit 0 for verified, 1 for denied/uninspectable, 2 for invalid invocation. No credential output.
- `git-access.py prepare --checkout PATH`: produces one host decision path on stdout; ordinary denial returns 0 with a read-only decision; inability to produce a safe decision returns 1.
- `git-access.py run --decision PATH -- GREYWALL ARGS...`: rechecks identity/evidence, starts the host-only broker/session channel and supervises one sandbox, returns the child's exit status; exits 77 on revocation/expired evidence. Reject tampered decision paths and unexpected command executables. It cannot expand mount scope or change mode to writable behind the launcher's back; it never exports a real write token. Broker per-session socket mount arguments are part of the reviewed runtime contract, not arbitrary agent-supplied paths.

All callers consume these CLI contracts; tests drive entry points, not undocumented functions. Helper subprocesses use a sanitized environment and fixed host program resolution. No registry path, inspector keyring identifier or executable is accepted from sandbox-supplied environment as authority.

### Task 1: Trusted identity and enrollment, without write release

**Files:** create helper and `tools/tests/test_git_access.py`; modify `tools/agent-github.sh` and `Makefile` only to expose explicit enrollment/check commands.

**Interfaces:** produces version-1 registry and `inspect` identity report (`mode`, `reason`, `repository_id`, `canonical_name`, `checkout`, `git_dir`, `common_dir`); consumes no agent write credential yet.

- [ ] Write CLI tests `test_unregistered_project_is_read_only`, `test_same_basename_never_selects_a_credential`, `test_local_mirror_needs_an_enrolled_edge`, `test_repository_recreation_changes_id`, `test_transfer_requires_reapproval`, `test_hostile_git_config_does_not_execute`, and `test_worktrees_share_identity_without_granting_siblings`. Assert denial/nonzero inspection, no registry writes during inspection, no executed payload and no token in output.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_git_access.py' -v`; verify new tests fail because the CLI does not exist.
- [ ] Implement `enroll`/identity portion of `inspect` using exact approved registry fields, sanitized Git discovery and Greywall-compatible metadata validation. Reject remote pushurl/rewrites, ambiguous roots/includes, mirror cycles and mismatching GitHub numeric IDs. Enroll only after host confirmation; atomic write and restrictive permissions.
- [ ] Add parameterized tests for subdirectory launch, linked worktree/gitfile, submodule, spaces, symlink swaps, separate common dir, non-Git/newly initialized repo, missing remote and inherited `GIT_DIR`/`GIT_CONFIG_*`; unsupported cases return explicit read-only reasons, never accidentally run checkout programs.
- [ ] Re-run the same command; expected all tests `OK`. Commit only the completed Task 1 files on an authorized feature branch, otherwise leave the diff for Theo.

### Task 2: Effective protection verification and non-destructive remediation

**Files:** helper; `tools/agent-github.sh`; new `tools/tests/test_agent_github.py`; extend helper tests.

**Interfaces:** `inspect` now verifies the spec's complete protection/credential contract; `prepare` emits a decision path. Existing `protect <project>` remains a separately approved host operation bound to the registry, not `origin`.

- [ ] Write `test_all_branch_floor_includes_future_nested_names`, `test_default_pr_has_zero_new_reviews`, `test_hidden_bypass_is_unknown`, `test_any_floor_bypass_denies`, `test_incomplete_pagination_denies`, and parameterized API failure tests (401/403/404/429/5xx, timeout/TLS, malformed/truncated JSON, unexpected redirects/schema). Assert no verified decision or writable broker capability on failure; real **write** token export is forbidden even on success.
- [ ] Run both new Python test files with unittest discovery; expected failures for the missing protection behavior.
- [ ] Implement complete paginated ruleset/detail inspection, numeric-ID matching and effective default/future branch queries using only Task 2a's dedicated App inspection role (fake responses suffice for this task's tests; no live credential is provisioned here). Encode exact `~ALL` and `~DEFAULT_BRANCH` floors with `bypass_actors: []`. Missing App capability/details/unknown bypass fails closed; stronger unrelated rules do not dilute the floor. No personal `gh` auth fallback.
- [ ] Write remediation tests `test_existing_ci_and_classic_rules_remain_unchanged`, `test_same_named_unowned_ruleset_is_not_overwritten`, `test_partial_install_never_reports_verified`, `test_existing_stronger_owned_rules_are_preserved`, `test_untrusted_legacy_pat_is_not_admitted`, `test_read_and_write_credentials_have_distinct_verified_roles`. Verify only reviewed baselines are mutated and no reviews/checks lowered. Credential setup must enumerate minimal action-specific grants from the broker spec, retain required Checks/Commit statuses/Actions reads and Issues/Pull requests workflows, and never expose Workflows/Administration/write tokens to agents.
- [ ] Implement additive remediation in `tools/agent-github.sh`, using helper inspection/enrollment rather than maintaining competing security policy. Fix its existing token setup's Workflows grant while touching that path; token replacement remains an explicit human action. Verify post-write state through the same inspector. Keep admin operations separate from `prepare`.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_git_access.py' -v` and `python3 -m unittest discover -s tools/tests -p 'test_agent_github.py' -v`; expected `OK` for both. Commit only authorized completed files.

### Task 2a: Dedicated App inspector authentication and permission proof gate

Controlled trial result: read grants failed closed on hidden bypass fields; after explicit **test-only** Administration-write approval, the actual App returned explicit empty lists and the evaluator passed. The runbook/ledger record exact IDs and approved requests. This supplies trial visibility evidence, not production App setup/grants, enrollment or rollout approval.

**Files:** existing proposed `git-access.py`, `tools/tests/test_git_access.py`, admin setup/check path in `tools/agent-github.sh`, runbook. Keep a plain host-only credential-provider function, not a new token service or dependency.

**Interfaces:** produces the inspector credential role consumed internally by the existing evaluator; external `inspect` returns only evidence/reason/cost, never an App key/JWT/token. Enrollment binds App/client ID, installation/owner/numeric repo IDs, explicit permission map and key epoch. Broker forwarding/native read/Project roles are unchanged.

- [ ] Write entry-point tests `test_inspector_requires_enrolled_app_not_personal_login`, `test_app_token_mint_has_explicit_repo_and_permissions`, `test_app_read_grants_do_not_imply_bypass_visibility`, `test_missing_bypass_never_escalates_permissions`, `test_suspended_or_uninstalled_repo_stays_read_only`, `test_app_key_jwt_and_tokens_never_enter_sandbox_or_logs`, and `test_opaque_variable_length_installation_tokens_are_supported`. Assert no personal credential fallback, grant widening, ruleset mutations or arbitrary token-mint action.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_git_access.py' -v`; expected new failures before the App provider exists.
- [ ] Implement host-only RS256 JWT signing with the installed OpenSSL binary, explicit `POST /app/installations/{id}/access_tokens` repo/permission narrowing, response grants/repo/expiry validation and protected per-repo authentication cache. JWT/installation token reuse never reuses a protection result. Reject expired/insufficient-lifetime tokens, wrong installation/owner/repo, credential/key-epoch change, mint failure and revoked/suspended installations without renewal/fallback. No new package/manifest or exposed signing/token service.
- [ ] Add deterministic fake-clock/mint tests proving one-hour server token expiry is separate from 600-second local evidence, each write still scans anew with a reused valid token, and mint/refresh consumes the complete 30-second check deadline and actual rate bucket. Synthetic test keys are fixtures, not real App setup. Re-run inspector suite; expected `OK`.
- [ ] Prepare exact permission/setup evidence for Theo: candidate Metadata read + Administration read; no assertion these reveal bypass actors. **Before any live trial**, obtain approval for the test App's registration/selected disposable installation/grants/key/mint operation. Prove full explicit bypass details with the actual installation token. If missing, stop read-only and request separate Administration-write approval with its settings-mutation risk; never auto-upgrade. Production registration/installations/grants require a further reviewed setup approval. No App bypass or personal inspector fallback.
- [ ] Record App ID/installation scope, tested grants and visibility evidence in the runbook; keep writes disabled until approved proof/setup succeeds. Commit only authorized implementation files; this task text is not authorization to execute setup now.

### Task 3: Prove the Greywall read-only dependency before integration

**Files:** runbook; conditional approved pin in `packages/greywall/build.sh`; launcher integration tests for dependency detection. Greywall implementation/test files must be identified by its owning project, not invented here.

**Interfaces:** consumes coordination evidence from `5b364923`; produces a verified minimum Greywall revision/capability and real-sandbox evidence that the spec's read-only settings are enforced. No fake-CLI test can replace this deliverable.

- [ ] Record the merged metadata fix's state and inspect its actual deployed capability with the Greywall owner. Do not mark its already-completed todo unfinished or assume its commit is installed.
- [ ] Before coding integration, agree the minimal read-only contract: whole checkout + resolved metadata roots immutable against rename/replacement; no writes from learned allow paths; no missing-hook initialization writes; no sibling grants; embedded Docker/automation sockets masked. If the current binary cannot enforce it, request a separately approved Greywall prerequisite and stop write rollout until it ships. Do not implement a local append-only gateway here.
- [ ] Add launcher test `read_only_dependency_missing_never_starts_a_writable_sandbox`: unavailable capability yields a named launch refusal and no writable broker capability, not an optimistic fallback.
- [ ] Obtain approved host execution of real Greywall tests that attempt direct file/ref writes, rename/unlink, packed-refs replacement, linked-worktree common-dir changes and socket access; assert filesystem denial and successful status/diff/log/source reads. Use scratch fixtures only, never probe the live sandbox for escapes.
- [ ] With explicit install/build approval, update only the verified revision/checksum and existing package check when needed, run the repository's pin tests and approved Greywall host tests. Record exact output and capability evidence in the runbook. Do not cut over while this task is blocked.

### Task 3a: Shared inspection budget before broker integration

**Files:** proposed `github-rate.py` / `tools/tests/test_github_rate.py`; parent `git-access.py` inspection entry point; runbook.

**Interfaces:** consumes Tasks 2/2a's complete App inspector and installation/principal/resource enrollment; produces the host-locked ledger and scheduled-only poll coalescing used by broker B1/B4e and Task 5. `inspect_access(checkout: pathlib.Path, *, purpose: str) -> dict` has exactly `inspection/admission/scheduled/write` purposes; clients cannot select purpose over the socket. Inspector installation bucket(s), App-auth mint buckets and user forwarding/read pools stay distinct; tokens from one installation do not create new quotas.

- [ ] Write `test_scheduled_checks_coalesce_by_repository_not_session`, `test_each_write_still_spends_a_fresh_inspection`, `test_pat_pool_is_shared_across_tokens`, `test_app_installation_bucket_is_shared_across_repo_tokens`, `test_app_mint_cost_is_not_free`, `test_rate_exhaustion_never_renews_or_forwards`, `test_read_snapshot_is_not_merge_evidence`, and multi-process reservation tests. Assert total examples 1,660/3,080/1,008/3,412/2,920 and selected-App splits 1,400+260 / 2,660+420 / 2,912+500 before actual mint overhead. Include all pages/merge/refresh costs and historical 60-second comparisons. No cached success authorizes a write.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_github_rate.py' -v`; verify failures before the ledger exists.
- [ ] Implement hidden host-locked principal/resource accounting, complete-check admission, header-based server reconciliation and scheduled-only polling coalescing. Preserve per-session local metadata checks; queue waits never extend leases. Unknown/exhausted quota denies write admission, never truncates a protection scan. Native/host read costs and additional merge guards must be represented in capacity reports, not assumed free.
- [ ] Rerun new rate/inspector suites; expected `OK`. Use approved 300/600/30 timings and selected dedicated App principal, require measured per-installation/forwarding/auth-bucket headroom before writes, and leave other resource limits and App setup/grants gated on Theo approval. Commit only authorized completed files.

### Task 4: Enforce admission in every agent launcher

**Files:** shared/per-agent launchers, Greywall config, hook, existing Rust launcher tests; helper `prepare`/`run` integration.

**Interfaces:** shared launcher consumes the decision path and mode; final invocations call `run`. Broker writes use only the revocable repo/session capability and own socket. Optionally export an independently enrolled truly read-only `GH_TOKEN` for native reads after scrubbing inherited tokens; any ignore-secret exception is exclusively for that proved read role, never the old/write PAT. Actual `GH_PROJECT_TOKEN` remains host-only. Broker Tasks B1–B4 (including B4a–e) and Greywall Task 3 must pass before writable admission exists.

- [ ] Extend `Launch` with fake host inspection/registry fixtures and table-driven tests over `AGENTS`. Add `failed_git_admission_keeps_reads_but_no_write_channels`, `inherited_tokens_never_bypass_the_gate`, `unregistered_future_projects_never_borrow_tokens`, `verified_feature_work_keeps_agent_arguments` and `native_gh_receives_only_separate_read_credentials`. Assert no GH/GITHUB **write** token, real Project write token, SSH agent/askpass or inspector secret leaks; no Docker/automation writer on denial; private reads work only with the separately verified read role. Test compatibility shim cannot promote a read token or expose broker writes via unknown actions.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test greywall_launcher`; expected new assertion failures (implementation-stage build authorization required).
- [ ] Replace unconditional repo/Project write-token export with `prepare`; scrub inherited authentication before other launcher work, then provision only reviewed read-role credentials and scoped broker capabilities. Build enforced read-only settings/channel masks when denied; hide registry/runtime/inspector credential files in shared config. Preserve existing read protections, answer sockets, provider credentials and native GitHub read workflows. Do not replace host desktop/phone `gh` with the sandbox compatibility client.
- [ ] Change each final `exec greywall` to helper `run`, preserving argument boundaries and exit statuses. Validate decision again immediately before capability creation; changed evidence stops/relaunches read-only rather than enabling an old writable settings file. Install the broker's sandbox-local Git adapter and exact enrolled URL overlay without changing persistent remotes. Host-approved Git authorization remains separate from security eligibility.
- [ ] Update `describe_github()` to report verified/read-only reason, canonical identity and missing read credential without suggesting that creating a write token fixes missing protection. Make admission diagnostics available to all agents on stderr; no new UI design work.
- [ ] Run all-agent launcher tests and both new Python suites; expected success. Re-run approved real-sandbox fallback tests from Task 3 through these actual launchers. Commit only authorized completed files.

### Task 5: Bounded revalidation and supervised containment

**Files:** helper and helper tests; proposed shared `github-rate.py` / `test_github_rate.py`; launcher tests; runbook.

**Interfaces:** `run` owns its child sandbox and verified decision, polls at reviewed bounds, exits 77 with a reason after containment. A subsequent checked launcher resumes the conversation read-only; never mutate mounts in place.

- [ ] Write fake-clock/process tests `test_check_failure_stops_the_entire_sandbox`, `test_no_stale_success_after_network_error`, `test_sleep_or_clock_rollback_never_extends_evidence`, `test_shared_common_dir_change_invalidates_all_related_admissions`, `test_child_dies_if_supervisor_dies`, and `test_copied_pat_is_not_claimed_revoked`. Assert scheduled checks at 300 seconds, inspection timeout at 30 seconds, immediate containment on first failure/timeout (not waiting for 600), independently enforced hard expiry at 600 seconds, and no managed writer after supervisor death. An attempted write at second 1, 299 or 599 still requires its own fresh scan.
- [ ] Run the helper suite; expected new tests fail before supervisor enforcement exists.
- [ ] Implement monotonic deadlines, bounded REST subprocesses, root/config/registry/credential invalidation, OS-backed parent-death/process-tree containment and explicit revocation diagnostics. Do not accept agent-provided PID/session identity as containment authority. Revalidation must also reconcile wall/suspend discontinuities, not merely monotonic elapsed time.
- [ ] Add tests for checker hang, identity swap between prepare/run, credential rotation, machine resume, expired queued launch and one failed sibling session; unaffected repositories retain service. Re-check repository/default-branch/baseline identity each time; never renew from a cached success after an API error.
- [ ] Integrate Task 3a's rate ledger into scheduled supervisor checks and broker admission. Test simultaneous same-repo sessions share only scheduled scans while every write still checks freshly; denied budget cannot renew evidence or leave an expired managed writer alive.
- [ ] Integrate containment with existing Cockpit restart/resume behavior only after verifying its process discovery still finds the real agent under the supervisor. Test all agents' exit/restart/sleep/wake entry points; no duplicate or orphan session. Document external writers as quarantined, not killed/revoked by assertion.
- [ ] Run helper, broker and launcher suites; expected success. Host-approved real process-tree tests must prove supervisor death behavior and immediate broker capability invalidation before rollout. Verify each new mediated write performs a fresh protection check even inside a valid session lease. Record in-flight/administrator-race limitations visibly; do not substitute raw token export when the broker is unavailable. Commit only authorized completed files.

### Task 6: Existing-project migration, operator review and staged cutover

**Files:** runbook, admin check/report path in helper/script, Makefile help; regression tests in both Python files and launcher suite.

**Interfaces:** `inspect` produces a read-only inventory/report for every Cockpit project and linked checkout, including missing-token projects. Enrollment/remediation/revocation/drain are separate approved host actions. No account-wide auto-mutation command.

- [ ] Add inventory tests `test_projects_without_tokens_are_not_skipped`, `test_unrelated_github_repositories_are_not_targets`, `test_parallel_sleeping_sessions_and_submodules_are_inventoried`, `test_active_external_writers_remain_quarantined`, and `test_rollback_never_restores_unconditional_exports`. Test missing/recreated registry state fails closed.
- [ ] Run the new Python/launcher tests, confirm failures for missing inventory/cutover behavior, then implement only the report and documented operator flow. Include the broker spec's actual command/JSON-field compatibility matrix; preserve Cockpit desktop/phone/digest and queued issue reads; retain per-project issue type/label/milestone/Project restrictions. Apply Theo's approved CI migration direction in the handoff: append-only updates/new branch names, retain protected remote branches. Record concrete per-job replacement/review/test evidence and remaining stack/fork gates **before affected cutover**; never disable jobs or weaken checks. List role credential/merge-policy provenance, legacy-token revocation, active/wakeable/external writers, inspector budget/headroom and read-only rollback.
- [ ] Obtain explicit approval before invoking builds/installs, GitHub remediation or revocation. Apply via normal reviewed host workflow, with all changed host-run shell scripts, Makefile, package pin and configuration called out. Drain/freeze legacy sessions without deleting their work; old mounts are not repaired by a new binary alone.
- [ ] With separately approved disposable GitHub repository and test credentials, verify new/nested branch creation and fast-forward push succeed; non-fast-forward/force-with-lease/ref-API replacement, branch deletion and direct default update fail. Confirm ordinary PR merge follows existing CI and the new baseline adds no mandatory reviewer. Do not assume Git's rejection of a plain non-force push tests server protection; exercise GitHub's actual enforcement. Do not delete the protected test branches without separate administrator approval.
- [ ] Verify one unprotected/private/unsupported repository remains useful read-only and receives only a separately scoped read credential if available. Check moved sessions, new projects and resumed sessions repeat admission. Verify stale exposed credentials were actually revoked server-side, not merely removed locally; record any unverifiable item as blocked.
- [ ] With explicit build approval run `make test`; expected all Python and Rust tests pass. Record failures verbatim and stop cutover on security regression. Show `git diff --stat`. Commit only authorized completed files; pushing, PR opening and rollout remain separate approvals.

## Separate later stage: local append-only gateway

Do not fold it into these tasks. Its own spec must mediate all canonical ref/object mutations, Git alternatives and direct filesystem writes, shared common-dir races, fetch/index/worktree semantics, library/API/ref writes, daemon writers and crash recovery while retaining ordinary branches/append-only commits. Neither pinned `.git/config`, Git hooks nor no-force GitHub rules satisfy that local guarantee.

## Planning review and handoff

Self-review: all approved requirements map to Tasks 1–6; security decisions and the read-only dependency are explicit in the spec. No product code, manifest, Git history, GitHub settings or installed configuration was changed during planning. No implementation tests were run. No separate session/reviewer was spawned, per the brief; this is not an independent security review.

STOP for Theo's full review and execution choice. Broker-first, workflows, CI append-only/new-branch/retained-branch policy, 300/600/30 timings and dedicated App inspector are decided; personal-login inspector fallback, raw write tokens and reduced v1 are rejected. Most important remaining setup approval is exact App permissions/selected-repository installation after visibility proof; other role grants/resources, concrete CI acceptance and stack/fork gates remain. Do not reopen choices or implement/dispatch workers from a review notification.

Report back to dotfiles session `ses_e98198cd4ffez1uHcbITHGzyW6` through `claude-todo add --project dotfiles` with this planning session ID, absolute plan/spec paths and outstanding decisions; use a matching original task's `done --summary` instead only if one is actually found. The earlier missing queued submission is not evidence of an existing task.
