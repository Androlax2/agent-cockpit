# History-preserving release proposal plan

> Execute inline; one independent fresh full-delta review before workflow push/merge.

**Goal:** Implement Theo's approved #192 proposal: release-plz updates local files, a normal
commit/push creates `release-plz/<base-sha>`, and a verified replacement draft precedes
cross-linking/closing older automation-owned drafts. Never rewrite/delete remote history,
merge drafts, tag, publish, or expand App permissions.

**Owner:** `tools/release_pr.py` owns proposal generation and GitHub draft reconciliation.
It calls existing Git, gh, release-plz and git-cliff; no product crate/dependency is added.

## Scope and decisions

- Add `release-plz.toml` with git-only mode, one inherited workspace version, changelog and
  semver checks disabled, dependency updates off, and every tag/release/publish switch off.
- Pin release-plz 0.4.2 and existing git-cliff 2.14.2. Host request c7c123b309ac approved
  SHA-256-verified isolated binaries; a minimal offline fixture proved initial 0.1.0 and
  post-tag fix bump to 0.1.1. Fixture tags never enter the real repository.
- Use the installed repository-only Release App, contents/write and pull_requests/write.
  Mint its token only after generation. Checkout persists no credentials.
- Serialize one workflow group; refuse stale main revisions before remote mutation.
- Keep proposal commits deterministic. Validate only workspace version/lock updates and
  CHANGELOG changes. External dependencies and member manifests must remain unchanged.
- On retry, verify an existing exact branch/PR and reuse it; refuse unexpected heads,
  human-authored PRs, non-drafts, closed targets and ambiguous results. Never force-push.
- A replacement must be fetched and verified before each superseded draft is linked/closed.
  Preserve closed PR branches; retain drafts edited/promoted by humans.
- #389 prerelease source conflicts with stale #192 text; ask one question and continue the
  independent proposal automation. No version/publication policy change is implicit.

## Tasks

1. Write failing tests at the real Python command entry point with temporary Git/bare
   repositories and a fake gh API: create, repeat, partial push/create/close, changed heads,
   stale main, forged ownership, ready/closed PR refusal and no force/delete/tag operations.
2. Implement `prepare` (local files and commit only) and `publish` (normal push and draft
   reconciliation). Validate generated files and remote ownership/head before side effects.
   Test real release-plz/git-cliff against an unpublished miniature workspace, including
   initial version, conventional fix bump, changelog and reproducibility. No live services.
3. Add `.github/workflows/release-plz.yml` on main pushes, dispatch and PR dry-run validation.
   A read-only job tests/generates; the guarded main-only write step mints a scoped App token
   and publishes drafts. No release-plz release/release-pr invocation or tag/publication step.
4. Document the approved decision, actual App setup evidence, retry/retention behavior and
   current stable prohibition. Update coverage map and stale #96 deferral text.
5. Run focused Python tests and tool conformance, Ruff, workflow/static/docs checks. Obtain
   one independent full-delta review. Workflow push through ask-to-run; normal reviewed PR
   and CI-gated squash auto-merge. Keep live validation claims open until actually measured.

## Review focus

Wrong ownership/base/head, partial remote writes and retries, cross-link/close ordering,
main advancing during a run, credentials exposed to untrusted PR code, package dependency
changes, and accidental reachability of tag/publication or history-rewrite paths.
