# Release proposals use fresh branches and retain all published history

Issue #192. Theo approved local generation with release-plz update, fresh normal pushes,
verified replacement drafts and retention of all branches/commits on 2026-11-02. He accepts
new PR numbers, repeated CI and retained branches instead of rewritten release PR history.

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
  the later bump. #389 still governs pre-release versioning; #192's older source wording is
  being clarified separately, not silently treated as approval to alter that policy.
- **Rejected:** native `release-plz release-pr` updates, force-with-lease, branch deletion,
  and local merges. Each violates the accepted history policy or adds unnecessary complexity.

Evidence: [isolated orchestration tests](../../tools/tests/test_release_pr.py),
[real-tool conformance](../../tools/tests/release_pr_conformance.py), and
[workflow](../../.github/workflows/release-plz.yml). Live draft workflow acceptance is not
claimed by fixture tests. Workflow pushes remain host-reviewed through `ask-to-run`.

## Generated Markdown correction

The first live draft (#514) and same-base retry preserved the exact branch/head, but
its docs CI found MD022 (header separation) and MD033 (a historical subject containing
an angle-bracket placeholder). Fix the shared git-cliff template rather than changing
the immutable draft, rewriting historical commits, or relaxing Markdown rules. The
shared postprocessor (`tools/changelog.py`) escapes prose while preserving matching
backtick code spans, including commands containing placeholders. The real-tool fixture
exercises both forms; command-entry tests cover escaped/unmatched backticks and multi-backtick
spans. Both release workflow path filters include the postprocessor. Full-history
output is checked with the unchanged `make docs-ci` in a disposable checkout before
review; replacement-draft CI still needs measured evidence.

## Review provenance for the Markdown correction

The first independent fresh-agent review covered the full delta at `8ba5d573a9c55272baf2cc5e5ceddcbe1e4f6b66` and found the
code-span entity regression. That was a fallback review, not `$code-review`. After
Theo clarified Codex skill discovery, the skill was found by following
`~/.codex/skills/code-review/SKILL.md`; the earlier unavailable-workflow report is
corrected. `$code-review medium` ran its eight inline angles on `570fe35..a9fca50`,
without subagents or a verifier pass. It found escaped opening backtick runs,
Markdown links and already escaped angle brackets changed by the postprocessor.
All three were reproduced with failing command-entry fixtures and corrected; the
focused tests and real-tool conformance then passed. This inline skill execution
is recorded separately from independent review.
