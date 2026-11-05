# Repository protection

Forge controls for `Androlax2/agent-cockpit` before outsiders contribute (the alpha, #97).
Owner: [#382](https://github.com/Androlax2/agent-cockpit/issues/382). This page does not
cover Torlek's runtime isolation ([report 11](../design/greenfield/11-security-isolation-platforms.md)),
product Git policy ([#155](https://github.com/Androlax2/agent-cockpit/issues/155)), the plugin
gate ([#376](https://github.com/Androlax2/agent-cockpit/issues/376)) or release jobs
([#191](https://github.com/Androlax2/agent-cockpit/issues/191),
[#192](https://github.com/Androlax2/agent-cockpit/issues/192), [Releasing](../RELEASING.md)).
Vulnerability reports follow [SECURITY.md](../../SECURITY.md).

**State on 2026-11-05** (read with `gh api`, GET only): every setting below is applied except
the two marked **pending**, which wait on the owner running a script
([Owner actions](#owner-actions)). Agents never change repository settings; Theo applies them
through `torlek ask run`.

## Verify

Every command is a read. Set the prefix once:

```sh
R=repos/Androlax2/agent-cockpit
```

`python3 tools/repository_security.py` collects the same metadata in one JSON document
([collector](../../tools/repository_security.py)); its exit 0 means collection completed, not
that protections pass. An HTTP 403/404 is an unknown, never a disabled setting.

### Branches and merging

| Setting | Required state | Verify |
| --- | --- | --- |
| Merge methods | Squash only | `gh api $R --jq '[.allow_squash_merge,.allow_merge_commit,.allow_rebase_merge]'` → `[true,false,false]` |
| `main: CI before merge` (ruleset `26273395`) | Active on `~DEFAULT_BRANCH`; deletion and non-fast-forward blocked; PR required, squash only, 0 approvals; checks `changes`, `greenfield`, `greenfield-wsl2`, `docs`, not strict; `bypass_actors: []` | `gh api $R/rulesets/26273395 --jq '{enforcement,bypass_actors,rules}'` |
| `main: CodeQL merge protection` (ruleset `26420673`) | Active on `~DEFAULT_BRANCH`; CodeQL results required, merge blocked on errors and on high or critical security alerts; `bypass_actors: []` | `gh api $R/rulesets/26420673 --jq '{enforcement,bypass_actors,rules}'` |
| Release branches | None exist; release proposals are draft PRs from `release-plz/*`, not channels | `gh api "$R/branches?per_page=100" --paginate --jq '.[].name'` |

Zero approvals and non-strict checks are deliberate: Theo is the sole maintainer, agents push
under his account and GitHub never lets an author approve their own PR (decided 2026-11-05
in #382, Q1); strictness is settled in the [CI-cost decision](../decisions/2026-11-01-397-ci-cost.md).
A rule with `bypass_actors: []` binds administrators too. An omitted `bypass_actors` field in
a response is an unknown, not an empty list.

### Release tags

| Setting | Required state | Verify |
| --- | --- | --- |
| `release: v tags created by Release App only` (`26382178`) | Active on `refs/tags/v*`; rule `creation`; sole bypass `Integration 5573576` (Release App) | `gh api $R/rulesets/26382178 --jq '{enforcement,conditions,bypass_actors,rules}'` |
| `release: v tags never moved or deleted` (`26382177`) | Active on `refs/tags/v*`; rules `update`, `deletion`; `bypass_actors: []`, the Release App included | `gh api $R/rulesets/26382177 --jq '{enforcement,conditions,bypass_actors,rules}'` |
| A tag from anyone else is refused | **Pending**: `python3 tools/repository_settings.py probe-tag-refusal` exits 0 | Same command (see [Owner actions](#owner-actions)) |

The two rulesets stay separate: one ruleset with an App bypass would also let the App move or
delete tags. A bypass names an App, not a workflow, so the Release App key is also confined to
the `release` environment below.

### Actions

| Setting | Required state | Verify |
| --- | --- | --- |
| Allowed actions | Enabled, `allowed_actions: all`, `sha_pinning_required: true` (**pending**) | `gh api $R/actions/permissions` |
| Default `GITHUB_TOKEN` | `read`; Actions cannot approve PRs | `gh api $R/actions/permissions/workflow` → `{"default_workflow_permissions":"read","can_approve_pull_request_reviews":false}` |
| Outside contributors' runs | Every external contributor needs approval | `gh api $R/actions/permissions/fork-pr-contributor-approval` → `all_external_contributors` |
| Workflow permissions | Each workflow's top level is `contents: read`; writes are job-scoped | `git grep -n -A2 '^permissions:' -- .github/workflows` |
| Pinned actions | Every `uses:` is a full commit SHA | `python3 -m unittest discover -s tools/tests -p 'test_workflow_scanners.py'` |
| No privileged PR triggers | No `pull_request_target`; `workflow_run` (docs) checks out `main`, filtered to same-repository pushes | `git grep -n -e pull_request_target -e workflow_run -- .github/workflows` |

### Environments and secrets

| Setting | Required state | Verify |
| --- | --- | --- |
| Repository secrets | None | `gh api $R/actions/secrets --jq .total_count` → `0` |
| `release` environment | Reviewer `Androlax2`, self-review allowed, admin bypass off; deploys only from branch `main` and tags `v*` | `gh api $R/environments/release --jq '{can_admins_bypass,protection_rules}'` and `gh api $R/environments/release/deployment-branch-policies --jq '[.branch_policies[]\|{name,type}]'` |
| `release` secrets | `RELEASE_APP_PRIVATE_KEY` only | `gh api $R/environments/release/secrets --jq '[.secrets[].name]'` |
| `release-proposal` environment | No reviewer (automatic proposals); deploys only from `main` | `gh api $R/environments/release-proposal/deployment-branch-policies --jq '[.branch_policies[]\|{name,type}]'` |
| `release-proposal` secrets and variables | `PROPOSAL_APP_PRIVATE_KEY`, `PROPOSAL_GIT_SSH_KEY`; variable `PROPOSAL_APP_CLIENT_ID` | `gh api $R/environments/release-proposal/secrets --jq '[.secrets[].name]'` |
| `github-pages` environment | Deploys only from `main`; no secrets | `gh api $R/environments/github-pages/deployment-branch-policies --jq '[.branch_policies[].name]'` |

Self-review stays allowed because Theo both dispatches and approves releases. The
`release-proposal` and `github-pages` environments keep GitHub's default admin bypass: only
Theo is an administrator, and the REST API cannot change it (the `release` setting was made in
the web UI).

**Fork pull requests cannot read secrets.** Theo accepted this structural proof in place of a
live fork test, which would need a second GitHub account (2026-11-05, #382 Q3). Each part
has its own check:

1. No repository secrets: `gh api $R/actions/secrets --jq .total_count` → `0`.
2. Environment secrets reach only `main` or `v*` refs, never a fork PR's
   `refs/pull/N/merge`: for each of `release`, `release-proposal` and `github-pages`,
   `gh api $R/environments/<name>/deployment-branch-policies --jq '[.branch_policies[]|{name,type}]'`
   lists only `main` (branch) and, for `release`, `v*` (tag).
3. No privileged trigger runs PR code with secrets:
   `git grep -n pull_request_target -- .github/workflows` prints nothing. GitHub gives an
   ordinary fork `pull_request` run no secrets and a read-only token
   ([GitHub](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflows-in-forked-repositories)).
4. Every outside contributor's run waits for approval:
   `gh api $R/actions/permissions/fork-pr-contributor-approval` → `all_external_contributors`.

### Security features

| Setting | Required state | Verify |
| --- | --- | --- |
| Secret scanning, push protection | Both enabled | `gh api $R --jq .security_and_analysis` |
| Dependabot alerts | Enabled | `gh api -i $R/vulnerability-alerts` → `HTTP/2.0 204` (404 means disabled or unreadable) |
| Dependabot security updates | Enabled | `gh api $R/automated-security-fixes` → `{"enabled":true,...}` |
| Dependabot version updates | Monthly, one grouped PR per ecosystem (Actions, root Cargo workspace), at most 2 open in all | [dependabot.yml](../../.github/dependabot.yml) |
| Private vulnerability reporting | Enabled | `gh api $R/private-vulnerability-reporting` |
| CodeQL | [codeql.yml](../../.github/workflows/codeql.yml): Rust and Actions on ready PRs, `main` pushes and weekly; default setup stays `not-configured` (it would conflict) | `gh api "$R/code-scanning/analyses?per_page=10" --jq '.[]\|[.ref,.category,.results_count,.error]'` |
| Scorecard | [scorecard.yml](../../.github/workflows/scorecard.yml) on `main` pushes and weekly; results published to api.scorecard.dev for the README badge | `gh run list -w Scorecard -L 1` and the [viewer](https://scorecard.dev/viewer/?uri=github.com/Androlax2/agent-cockpit) |

CodeQL Rust analysis uses `build-mode: none`, which still runs build scripts and procedural
macros; the job holds only `contents: read` and `security-events: write`, no environment.
Scorecard publication requires the official action with job-scoped `id-token: write`, no
`run` steps and only allowed actions; `test_workflow_scanners.py` keeps that shape. The action
is pinned to commit `fa7117db83c52d787ae5a89f350999701505d78e` (v2.5.0), but its metadata pulls
the container `ghcr.io/ossf/scorecard-action:v2.5.0` by a tag that upstream could re-push. That
is the price of the badge: publication accepts only the official action, not the
digest-pinned `docker://` image used before. The job holds no secret and only a read token;
its OIDC token proves the run to api.scorecard.dev and nothing else.

## Scorecard results

Scorecard v5.6.0 on `main` at `bf2d77dbb8ab` (run
[42054062034](https://github.com/Androlax2/agent-cockpit/actions/runs/42054062034),
2026-11-05), aggregate 5.4. Risk levels from
[Scorecard's checks](https://github.com/ossf/scorecard/blob/main/docs/checks.md). Scores are
heuristics, not proof of enforcement.

| Check | Risk | Score | Disposition |
| --- | --- | --- | --- |
| Dangerous-Workflow | Critical | 10 | Passes |
| Token-Permissions | High | 10 | Passes |
| Binary-Artifacts | High | 10 | Passes |
| Vulnerabilities | High | 9 | One open alert: KaTeX GHSA-238p-pmpm-9mq7 (low) in `tools/docs-lint` via the Markdown linter; needs KaTeX ≥ 0.18.2 |
| Branch-Protection | High | 3 | Accepted exception: 0 approvals, no code-owner review, non-strict checks (Theo, 2026-11-05) |
| Code-Review | High | 0 | Accepted exception: sole maintainer, agent-reviewed PRs (Theo, 2026-11-05) |
| Dependency-Update-Tool | High | 0 | Fixed by [dependabot.yml](../../.github/dependabot.yml) from the next run |
| Maintained | High | 0 | Time-based: the repository is under 90 days old; passes on its own after 2027-01-19 |
| Signed-Releases | High | 0 | Releases carry GitHub attestations but no signature or SLSA provenance file among the release assets, which is what Scorecard reads; release lane (#191) |
| Pinned-Dependencies | Medium | 9 | False positive: `packages/greyproxy/settings.sh:19` pipes JSON into an inline Python one-liner, not a downloaded script |
| SAST | Medium | 10 | Passes |
| Security-Policy | Medium | 10 | Passes |
| Fuzzing | Medium | 0 | No fuzzing; not planned for the alpha |
| Packaging | Medium | −1 | Not detected (inconclusive) |
| License, CI-Tests | Low | 10 | Pass |
| Contributors | Low | 3 | One organization; not actionable |
| CII-Best-Practices | Low | 0 | No badge; not planned for the alpha |

Issue #382's acceptance "no high-risk check failing" therefore holds only with the two accepted
exceptions, once Dependabot runs and the repository ages; Signed-Releases stays open on #191.

Open CodeQL alerts on `main` (2026-11-05): 45 `rust/cleartext-logging` (high),
4 `rust/hard-coded-cryptographic-value` (critical), 1 `actions/cache-poisoning/poisonable-step`
(high). They predate the CodeQL ruleset, which blocks only alerts a PR introduces, and need
triage. `gh api "$R/code-scanning/alerts?state=open&per_page=100" --paginate --jq '.[].rule.id' | sort | uniq -c`

## Credentials

Metadata only; no value is ever recorded.

| Credential | Holder and scope | Expiry and rotation |
| --- | --- | --- |
| Release App `agent-cockpit-release`, App ID `5573576` | Contents write, pull requests write; this repository only. Key: `release` environment secret, updated 2026-11-03T00:22:07Z; recovery state on Theo's host | App keys do not expire; rotate by adding a key in the App settings, replacing the secret, then deleting the old key. No rotation date set |
| Proposal App `agent-cockpit-proposal`, App ID `5582369`, installation `177891581` | Contents read, pull requests write, metadata read; this repository only. Key: `release-proposal` secret | Same as above; recovery state `~/.local/state/cockpit-proposal-app` |
| Deploy key `168493357` (`release-proposal Git-only push`) | Write; no ruleset bypass, so it cannot touch `main` or `v*` tags. Private half: `release-proposal` secret `PROPOSAL_GIT_SSH_KEY` | Does not expire; rotate by adding a new key, replacing the secret, deleting the old one |
| App installation tokens | Minted per job from the keys above | One hour |
| `GITHUB_TOKEN` | Per job; `contents: read` by default, writes only where a job declares them | End of the job |
| OIDC tokens | Release attestations and Scorecard publication (`id-token: write`) | Minutes; no stored key |
| Per-project `GH_TOKEN` for agent sessions | Fine-grained token in `~/.config/torlek/github-tokens/agent-cockpit` ([decision](../decisions/2026-11-04-754-per-project-github-token.md)); contents and pull requests on this repository | Set at creation; read it with `gh api -i user \| grep -i github-authentication-token-expiration` using that token |
| Theo's own `gh` login | Owner; the only administrator | Theo's account settings |

Verify the deploy key with `gh api $R/keys --jq '.[]|[.id,.title,.read_only]'`. The Apps and
their installations need App authentication to read; their IDs come from the host receipts in
[#382](https://github.com/Androlax2/agent-cockpit/issues/382) and
[#192](https://github.com/Androlax2/agent-cockpit/issues/192).

## Owner actions

Run each through `torlek ask run` on the host with Theo's `gh` login, after this page's PR is
merged. Both are idempotent and read their result back; [tests](../../tools/tests/test_repository_settings.py)
drive them through a fake `gh`.

1. `python3 tools/repository_settings.py require-action-sha-pinning`: turns on
   `sha_pinning_required`, keeping the allowed-actions policy. Exit 0 when set or already set.
   Every workflow on `main` must already pin by SHA, or its runs will fail: on an up-to-date
   `main` checkout, run `python3 -m unittest discover -s tools/tests -p 'test_workflow_scanners.py'`
   first.
2. `python3 tools/repository_settings.py probe-tag-refusal`: tries to create
   `refs/tags/v0.0.0-ruleset-probe` as Theo, on the repository's first commit `3abf9222`, which
   holds no workflow: a wrongly created tag starts no Release run. Exit 0 when the creation is
   refused and ruleset `26382178` still reads as the Release App's creation rule. Exit 1 when
   the tag is created or already exists, or the ruleset differs (the tag rules are broken; the
   immutability ruleset keeps a created probe tag until they are fixed). Exit 2 when
   inconclusive, such as a timeout: then check whether the tag exists.
