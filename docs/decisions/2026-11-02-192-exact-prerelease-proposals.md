# Draft the exact next prerelease and consume its verified version

Issue #192. Decided by Theo on 2026-11-02; implemented by the release worker.

- **Exact version:** the open draft's title, workspace version, local lock entries and
  generated changelog all name the upcoming release, beginning with `0.1.0-alpha.1`.
- **Sequential suffix:** `.2` follows the first tag, not the next commit or replacement
  draft. Failed publication still consumes a tag's number. Beta starts at `.1` separately.
- **Source stays main:** the draft is not merged. Dispatch verifies the sole open App-owned
  immutable proposal for current main, consumes its version and tags its source. Builds
  stamp that same version; ordinary main keeps the bootstrap core.
- **Fail closed:** absent, multiple, edited, foreign, stale or inconsistent proposals
  cannot select a release. Existing checklist gates and candidate acceptance are unchanged.
  Historical tag checking does not depend on an open draft.
- **Verified-snapshot boundary:** Theo approved Option A after independent review.
  Final verification captures source SHA, proposal number/head and exact release tag
  after write-token creation. The workflow records and uses that snapshot in the same
  shell step. Later main or proposal changes do not revoke it. This is explicitly
  not an atomic transaction across GitHub refs and PR metadata; observed discrepancies
  during verification refuse, and unsuccessful verification cannot request a tag.
- **History stays immutable:** verified replacements close predecessors unmerged and
  retain branches and commits. Legacy stable-shaped drafts can be retired, never consumed.
- **Channel transitions keep base-only identities:** Theo chose Option A for beta:
  after #57 closes, a deliberate reviewed main commit triggers the beta proposal.
  Same-source alpha-to-beta replacement refuses and preserves the existing draft;
  version-qualified branch names are not introduced.
- **Rejected:** stable-shaped `0.1.0` proposals hide the actual next alpha. Commit-count
  suffixes advance without releases. Both earlier policies are superseded for prereleases;
  nightly `dev.N` numbering retains its commit count.

Evidence: [release policy tests](../../tools/tests/test_release.py),
[real-Git proposal fixtures](../../tools/tests/test_release_pr.py), and the separate
[pinned-tool conformance check](../../tools/tests/release_pr_conformance.py).
Local fixtures do not establish live workflow acceptance; that needs linked CI evidence.

## Revisited 2026-11-02 (#192, history-preserving proposals)

Theo approved local generation with release-plz update, fresh normal pushes, verified
replacement drafts and retention of all branches and commits on 2026-11-02. He accepts new PR
numbers, repeated CI and retained branches instead of rewritten release PR history. Release
proposals use fresh branches and retain all published history.

- **Local generation, separate publication of drafts.** `release-plz update` calculates
  files without the App token. `tools/release_pr.py publish` only pushes a new proposal
  commit and manages drafts; it cannot tag, publish a package or publish a GitHub Release.
- **One deterministic proposal per main revision.** The branch is
  `release-plz/<full-base-sha>`. Same-base retries require exactly the expected branch/head
  and an open draft owned by the Release App. Unexpected or closed targets stop the run.
  The commit date comes from its base; git-cliff renders an unreleased section without a
  wall-clock release date. Local workspace version/lock/changelog edits are the only changes.
- **Verify the replacement before retiring the old draft.** Cross-link and close only
  unchanged App-owned drafts with a base in the current main history. Re-fetch the
  replacement before closing; retain human-edited/non-draft/unrelated PRs and every branch.
  Deduplicate comments after partial failures. Main advancing makes an old run refuse.
- **Reuse the repository-only Release App.** Its installation and credential configuration
  were [verified live](https://github.com/Androlax2/agent-cockpit/issues/192#issuecomment-6567550339)
  after #510. The workflow requests contents/write and pull_requests/write on only this
  repository, after read-only generation. PR checks receive neither App secrets nor writes.
- **Existing stable prohibition remains.** All tag/release/publish config switches are off.
  An initial unpublished workspace stays 0.1.0; a disposable post-tag fix fixture proves
  the later bump. #389 still governs pre-release versioning.
- **Generated Markdown is fixed in the shared git-cliff template,** through the
  postprocessor `tools/changelog.py` (it escapes prose and keeps matching backtick code
  spans), never in the immutable draft, in rewritten commits or by relaxing Markdown rules.
  Both release workflows' path filters include the postprocessor, so a change to it runs them.
- **Rejected:** native `release-plz release-pr` updates, force-with-lease, branch deletion,
  and local merges. Each violates the accepted history policy or adds unnecessary complexity.

Evidence: [isolated orchestration tests](../../tools/tests/test_release_pr.py),
[real-tool conformance](../../tools/tests/release_pr_conformance.py), and
[workflow](../../.github/workflows/release-plz.yml). Live draft workflow acceptance is not
claimed by fixture tests. Workflow pushes remain host-reviewed through `ask-to-run`.
