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
