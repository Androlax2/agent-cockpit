# Repository protection

**Status: partial audit for [#382](https://github.com/Androlax2/agent-cockpit/issues/382),
not live enforcement acceptance.** Repository settings and workflows were not changed.
The first priority is to verify the hidden settings with the owner, then review the
tag and release-environment proposal below before applying anything.

## Ownership and evidence

This document owns the forge controls for `Androlax2/agent-cockpit`, a public repository
preparing its first alpha. It does not establish Cockpit's runtime sandbox guarantees.
[Report 11](../design/greenfield/11-security-isolation-platforms.md) requires requested,
supported, effective and observed protections to remain distinct.

- [#155](https://github.com/Androlax2/agent-cockpit/issues/155) owns product Git policy,
  credential brokerage and workflow-edit approval.
- [#191](https://github.com/Androlax2/agent-cockpit/issues/191) and
  [#192](https://github.com/Androlax2/agent-cockpit/issues/192) own release jobs, App setup
  and publication gates. Coordinate environment/key changes there.
- [#376](https://github.com/Androlax2/agent-cockpit/issues/376) owns the plugin security gate.
  Vulnerability reports follow [SECURITY.md](../../SECURITY.md).

**Snapshot:** 2026-11-02, 04:46–04:52 UTC. Source inspected at
[`ea5458970720cf24dd807c21a1cdb808085ed5a4`](https://github.com/Androlax2/agent-cockpit/tree/ea5458970720cf24dd807c21a1cdb808085ed5a4).
Live metadata was read with the launcher's repository-scoped credential. A successful
paginated list establishes what that API exposed at that time. HTTP 401/403/404 and
omitted fields are visibility gaps, not evidence that a setting is disabled.

## Repeat the read-only collection

From a checkout with Python 3.11+ and existing authenticated GitHub CLI:

```sh
python3 tools/repository_security.py
```

The [collector](../../tools/repository_security.py) calls fixed `gh api` endpoints with
`--hostname github.com --method GET --include`, advancing page numbers when the response
has a next-page Link. It prints timestamped JSON, including successful pages and
discovered ruleset details even when a later page fails. It reads secret **names and
timestamps only**, and only the known non-secret `RELEASE_APP_CLIENT_ID` variable.
It never opens the host's App recovery record, authenticates as the App, reads secret
values, creates tags, dispatches workflows or applies settings.

Exit 0 means metadata collection completed, **not** that protections passed. Exit 1
retains successful observations alongside unknowns; 404 remains unknown even when an
independent complete listing explains it. Missing fields still need interpretation.
Each endpoint's collection has a 60-second deadline across its pages. Raw CLI errors are withheld; HTTP status or a
generic execution error is recorded. Do not commit the host output without reviewing it.
Agents need host approval to run this with the owner's existing host account; never
widen the sandbox credential to fill these gaps.

The snapshot run exited 1: public metadata succeeded, privileged reads returned 403,
and the `release` environment endpoints returned 404. The complete environment listing
contained only `github-pages`. A post-review read at 05:01 UTC confirmed those results
and fetched both branch-list pages. These are incomplete-audit results, not test failures.

## Observed controls and verification commands

All commands below are reads. Prefix each endpoint in the tables with
`repos/Androlax2/agent-cockpit`; for example:

```sh
gh api --hostname github.com --method GET --paginate --slurp repos/Androlax2/agent-cockpit/rulesets
gh api --hostname github.com --method GET repos/Androlax2/agent-cockpit/rulesets/26273395
```

### Branches and tags

| Setting | Observed state | GET endpoint / verification |
| --- | --- | --- |
| Repository/default branch/merge methods | Public; `main`; squash true, merge/rebase false | Repository root; fields `visibility`, `default_branch`, `allow_*_merge` |
| Effective main rules | Active ruleset `26273395`, `~DEFAULT_BRANCH`, no exclusions; deletion and non-fast-forward prohibited | `/rulesets`, `/rulesets/26273395`, `/rules/branches/main` |
| Pull requests | Required; allowed merge methods `["squash"]` | Ruleset's `pull_request.parameters` |
| Reviews | Required approvals `0`; code-owner review, stale-review dismissal, last-push approval and conversation resolution all false | Same parameters; [CODEOWNERS](../../.github/CODEOWNERS) assigns Theo but does not enforce review |
| Required checks | `changes`, `greenfield`, `greenfield-macos`, `greenfield-wsl2`, `docs`; strict/up-to-date false; enforcement on creation enabled | Ruleset's `required_status_checks.parameters`; no `integration_id` supplied for these contexts |
| Bypass | Explicit `bypass_actors: []`; caller bypass `never` | Ruleset detail; an **omitted** bypass field in a future response is unknown, not an empty list |
| Classic protection | HTTP 403; unknown whether additional classic rules exist | `/branches/main/protection` |
| Release tags | No tag-targeted ruleset in the complete listing | `/rulesets?per_page=100`, follow each ID; no tag creation/update/deletion attempt was made |
| Release branches | No `release/*` branch listed; five `release-plz/*` proposal branches were unprotected | `/branches?per_page=100` paginated; draft branches are not release channels |

The non-strict main check policy is intentional in the
[CI-cost decision](../decisions/2026-11-01-397-ci-cost.md). Do not enable strict mode,
extra approvals or a merge queue merely to improve a score. The agent review workflow
is a process requirement, not a GitHub-enforced required review.

### Actions, environments and security features

| Setting | Observed state | GET endpoint / verification |
| --- | --- | --- |
| Actions allowed sources | HTTP 403, unknown | `/actions/permissions`; if `allowed_actions` is `selected`, also `/actions/permissions/selected-actions` |
| Default token and PR approval permission | HTTP 403, unknown | `/actions/permissions/workflow`: `default_workflow_permissions`, `can_approve_pull_request_reviews` |
| Approval of outside contributors' workflows | HTTP 403, unknown | `/actions/permissions/fork-pr-contributor-approval`: `approval_policy` |
| Release environment/reviewer/bypass | Complete listing contained only `github-pages`; no `release` entry. Direct `release` read returned 404 | `/environments?per_page=100`, `/environments/release`: `protection_rules`, `can_admins_bypass`, `deployment_branch_policy` |
| Release refs | Direct read returned 404 | `/environments/release/deployment-branch-policies?per_page=100`; inspect both branch and tag types |
| Existing Pages environment | `github-pages`, admin bypass true, custom branch policy, no required reviewer; its only ref policy is branch `main` | `/environments`, `/environments/github-pages/deployment-branch-policies` (separate read) |
| Secret scanning and push protection | `security_and_analysis` omitted from repository response; unknown | Repository root, `security_and_analysis.secret_scanning` and `.secret_scanning_push_protection` |
| Dependabot alerts | HTTP 403, unknown | `/vulnerability-alerts`; authorized 204 means enabled; 404 alone is ambiguous |
| Dependabot security updates | HTTP 403, unknown; no tracked `.github/dependabot.yml` | `/automated-security-fixes`, field `enabled`; missing version-update config does not prove security updates are off |
| CodeQL/default setup | HTTP 403, unknown | `/code-scanning/default-setup`: `state`, `languages`, `query_suite` |
| Code scanning results | HTTP 403, unknown | `/code-scanning/analyses?per_page=100`: tool, category, commit/ref, errors/warnings; successful upload is not a clean analysis |
| Required scanning on main | No `code_scanning` rule or scanning check among the five required contexts | `/rulesets/26273395` and effective branch rules |
| Private vulnerability reporting | `enabled: true` | `/private-vulnerability-reporting` |
| Scorecard | No Scorecard workflow among the five registered workflows or in inspected source; no score/run measured | `/actions/workflows?per_page=100`; inspect source as below |
| Secret storage and App client ID | Repository secret metadata and exact variable reads returned 403; environment secret read returned 404 | `/actions/secrets?per_page=100`, `/environments/release/secrets?per_page=100`, `/actions/variables/RELEASE_APP_CLIENT_ID` |

### Workflow source boundaries

Recheck the tracked source, not just API settings:

```sh
git ls-files .github
git grep -n -E 'permissions:|uses:|persist-credentials:|pull_request_target|workflow_run|environment:|secrets\.' -- .github/workflows
```

All five workflows declare read-only top-level permissions. External `uses:` references
are pinned to 40-character commit SHAs; all checkouts disable persisted credentials.
No `pull_request_target` trigger exists in this snapshot.

| Source at snapshot | Credential/code boundary |
| --- | --- |
| [ci.yml:8–9](https://github.com/Androlax2/agent-cockpit/blob/ea5458970720cf24dd807c21a1cdb808085ed5a4/.github/workflows/ci.yml#L8-L9) | Ordinary PR tests request contents/read; path-dependent skipped jobs are not executed checks |
| [prerelease.yml:30–80](https://github.com/Androlax2/agent-cockpit/blob/ea5458970720cf24dd807c21a1cdb808085ed5a4/.github/workflows/prerelease.yml#L30-L80) | Manual dispatch defaults dry; rejects refs other than main; non-dry step mints an App contents/write token and creates the tag. No environment reference |
| [release-plz.yml:53–92](https://github.com/Androlax2/agent-cockpit/blob/ea5458970720cf24dd807c21a1cdb808085ed5a4/.github/workflows/release-plz.yml#L53-L92) | Main-only non-PR proposal job; generates before minting the repository-only App contents/write + PR/write token. No environment reference |
| [release.yml:283–377](https://github.com/Androlax2/agent-cockpit/blob/ea5458970720cf24dd807c21a1cdb808085ed5a4/.github/workflows/release.yml#L283-L377) | Tag-only draft gets job-scoped contents/write, id-token/write and attestations/write; publish gets contents/write and waits for gates. Uses `GITHUB_TOKEN`, not the App token; neither job references an environment |
| [docs.yml:27–44,139–152](https://github.com/Androlax2/agent-cockpit/blob/ea5458970720cf24dd807c21a1cdb808085ed5a4/.github/workflows/docs.yml#L27-L44) | `workflow_run` is filtered to a successful same-repository push; catalog checks out main, not PR code. PR builds use the merge ref with read permissions; deployment excludes PRs and uses `github-pages` with job-scoped pages/write + id-token/write |

[GitHub's fork policy](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflows-in-forked-repositories)
withholds repository secrets from ordinary fork `pull_request` runs and gives a read-only
token. This plus source inspection is design evidence, **not** an exercised fork-secret
acceptance test. No fork PR was opened, approved or run for this audit.

## Prioritized gaps and proposed external changes

These are **unapplied proposals**, not newly granted permissions or confirmed exploitation.

### 1. Release identity and immutable tags (high-priority control gap)

Source → effect: the App's contents/write token creates refs in `prerelease.yml:66–80`;
the complete ruleset list contains no tag restriction. Workflow checks restrict the
normal path but are not server-side restrictions on another use of a writing identity.

Propose two active tag rulesets matching `refs/tags/v*`, no exclusions:

| Proposed rule | Rules | Bypass |
| --- | --- | --- |
| Release tag creation | `creation` | Only `actor_type: Integration`, actual numeric Release App ID, `bypass_mode: always` |
| Release tag immutability | `update`, `deletion` | Empty list, including for the Release App |

**Never combine these into one App-bypass ruleset:** that would also let the App move
or delete tags. `non_fast_forward` alone does not prohibit every tag update. Read back
both rulesets and verify target, pattern, enforcement, all rules and bypass lists.
GitHub's [rules REST reference](https://docs.github.com/en/rest/repos/rules) defines
these operations and `Integration` IDs. Client ID and installation ID are not App ID.
No numeric ID is guessed in a setup payload.

The existing App serves **both** release-proposal drafts and prerelease tagging. A
ruleset bypass identifies an App, not a workflow/job: it cannot prove “only this one
workflow may create a tag.” Scoping the key to an approved environment and controlling
every consumer is necessary. A separate identity would be a new owner decision.

### 2. Release environment and credential placement (high-priority control gap)

Source → effect: repository-scoped `RELEASE_APP_PRIVATE_KEY` is consumed by the two
workflows above; the environment listing has no `release`, and draft/publish jobs
have no approval environment. The existing publication gate can refuse a release,
but it is not Theo's environment approval.

Propose a `release` environment with Theo (`Androlax2`) as required reviewer and admin
bypass disabled. Select branch `main` for tagging and tag pattern `v*` for publication;
branch/tag entries are separate. Every credential-consuming or publishing job must
reference the reviewed environment **before** it can receive credentials or write.
Creating an environment without wiring the jobs provides no protection.

Coordinate the single App key's migration with #192: moving it out of repository secrets
would also gate automatic proposal drafts. Keeping a repository copy would defeat the
intended environment boundary. Do not move/copy/delete keys or rerun
`setup_release_app.py configure` as a migration: that tool's contract is repository
storage and interrupted-write recovery, not environment migration.

Theo is the sole reviewer and may dispatch releases himself. Enabling “prevent
self-review” would then block his approval; settle that policy explicitly rather than
quietly adding a bypass. [Environment rules](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)
explain reviewer, self-review, administrator bypass and secret-release semantics.

### 3. Security scanning and Actions policy (unverified or missing integration)

After an owner metadata read, propose enabling whichever requested features are actually
off: secret scanning, repository push protection, Dependabot alerts/security updates.
Retain private reporting. Propose Actions default read permission, PR-approval permission
off and `all_external_contributors` approval. Read back each setting with the table's
endpoint; no PUT/PATCH command is part of the collector.

Required checks currently have no App binding. Review binding their observed check-run
producer IDs before any update; do not invent an integration ID or replace the existing
five checks. Additional CodeQL requirements follow successful baseline and PR scans,
not merely the existence of a workflow file. Changes to workflows/settings require the
normal host approval even if their diff is reviewed here.

## Verified CodeQL and Scorecard capabilities

Official sources retrieved 2026-11-02:

- [GitHub's supported-language list](https://docs.github.com/en/code-security/code-scanning/introduction-to-code-scanning/about-code-scanning-with-codeql)
  includes **Rust and GitHub Actions workflows**. An old “Rust unsupported” assumption
  is incorrect. [Rust build options](https://docs.github.com/en/code-security/reference/code-scanning/codeql/build-options-for-compiled-languages#building-rust)
  specify build mode **`none`**, using `rust-analyzer`, with `rustup`/`cargo` required.
  It can compile/run `build.rs` and macro code: `none` does not mean no code execution.
- Proposed integration: Rust and Actions analysis with read-only top-level permissions,
  only required job-scoped security-events write, pinned CodeQL actions, no release/App
  secrets, and ordinary `pull_request`/main/schedule events. Verify the Rust database
  covers both the intended greenfield workspace and any claimed legacy scope; don't
  infer complete coverage from one green job. Select required result categories/check
  names from actual runs, then apply a reviewed scanning rule/required checks.
- [OpenSSF's Action README](https://github.com/ossf/scorecard-action#readme) recommends
  the default `GITHUB_TOKEN`; rulesets can be inspected without an admin PAT. `push`
  and default-branch `schedule` are supported; PR/dispatch triggers are experimental,
  and running the Action on a fork repository is unsupported.
- A Scorecard badge requires `publish_results: true` and job-scoped `id-token: write`.
  This is external publication and needs approval. The producing job has restrictions:
  no `run` steps, env/defaults or containers, only the upstream allowed actions; no
  top-level env/defaults or write permissions. SARIF upload needs security-events write.
  Validate the exact pinned release supports the chosen inputs; do not copy a moving
  README's newest options into an older pin.
- [Scorecard checks](https://github.com/ossf/scorecard/blob/main/docs/checks.md) are
  heuristics, not an enforcement certificate. Its high-risk Code-Review check excludes
  AI/bot reviews; Branch-Protection scoring rewards human approvals and strictness absent
  here by policy. A successful Action does not mean all high-risk checks passed. Record
  the version, source SHA, per-check scores/reasons and publication result; obtain an
  explicit owner disposition for policy conflicts. Do not weaken guards or invent human
  approvals to make the score green.

There is no measured CodeQL or Scorecard run in this audit. No workflow, badge or scanner
dependency is added by this PR; activation and high-risk findings remain #382 acceptance.

## Credential inventory (metadata, not values)

| Credential / identity | Holder and boundary | Expiry / remaining verification |
| --- | --- | --- |
| Sandbox `GH_TOKEN` | Launcher-injected repository-scoped PAT; this audit used only metadata reads and issue/own-PR coordination | API response header reported `2027-01-28 23:00:00 UTC`; exact permission inventory unavailable; administrative reads refused |
| `agent-cockpit-release` App | Host registration/install/configuration verified on [#192](https://github.com/Androlax2/agent-cockpit/issues/192#issuecomment-6567550339): contents/write, pull_requests/write, implicit metadata/read, selected repository only | Numeric App ID and installation ID not available in this session; request non-secret metadata from release owner, never display private state |
| `RELEASE_APP_PRIVATE_KEY` | Repository Actions secret per host setup evidence; recovery record in the host's owner-only state directory; shared by proposal and tag workflows | No rotation date recorded in accessible evidence. Secret timestamps cannot prove key value or expiry; owner must record rotation/revocation policy |
| App installation tokens | Minted per workflow; prerelease requests contents/write, proposal requests contents/write + PR/write for this repository | GitHub [installation tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-an-installation-access-token-for-a-github-app) expire after one hour; actual token expiry not inspected or exercised |
| Workflow `GITHUB_TOKEN` | GitHub job credential; release draft/publish and Pages get the job permissions listed above | GitHub-managed lifetime; no long-lived publishing PAT evidenced in source |
| OIDC attestation identity | Release draft requests id-token/write for build provenance | Ephemeral identity, not a stored signing key; no attestation/publish exercised here |
| Other signing/publishing credentials | Repository metadata unreadable; no complete inventory claim | Owner read must list names, holders, purpose, scope, expiry/rotation and environment placement; never values |

The [Release App setup evidence](../RELEASING.md#release-app-setup) already exists;
do not recreate the App. `GET /repos/.../installation` requires App authentication
(this PAT returned 401), so it is not a way to prove installation scope with the sandbox
credential. Existing host evidence is cited rather than minting a token to repeat it.

## Remaining acceptance and morning handoff

- [ ] Owner reviews the metadata collector and runs it once with existing host access.
  Fill unknown settings and the non-secret App ID/rotation inventory; record the timestamp.
- [ ] Review exact desired-state changes against that fresh snapshot with the release
  owner, including shared-key proposal jobs and sole-reviewer policy. Apply only through
  explicit host approval; retain before/after metadata. There is no setup/apply mode here.
- [ ] Activate approved scanners; link CodeQL baseline/PR results, required rules and
  Scorecard's per-check outcome/publication. Keep policy exceptions explicit.
- [ ] Theo separately approves a bounded live enforcement procedure for unauthorized
  tag refusal and fork-secret isolation. No arbitrary test tag or fork execution is
  authorized by this document. Metadata and fixtures cannot close these criteria.

Independent full-delta review found loss of earlier pages on later-page failures.
The collector now fetches pages individually and retains completed pages and discovered
ruleset IDs on HTTP failure or timeout. The real CLI/fake-command regression reproduces
the HTTP failure; a real-main transport fixture covers timeout. Object-shaped environment
pagination is covered too. Exact commands and results are recorded on the PR.
The required named
`code-review medium` workflow was absent from the OpenCode catalog and inspected skill
trees (symlinks followed); the Codex skill path cited by the release owner was absent
in this session. That gate remains open, rather than relabelling independent review.
This work is **Part of #382**, not a closure or an application to legacy Cockpit.
