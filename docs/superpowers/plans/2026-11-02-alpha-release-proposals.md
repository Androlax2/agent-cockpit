# Exact alpha release proposals implementation plan

> **For agentic workers:** Use `executing-plans` to implement this plan inline, task by task. Keep the existing #523 checkout and evidence intact; implementation belongs on a separate #192 branch.

**Goal:** The open draft proposes the exact next prerelease, beginning with `0.1.0-alpha.1`, and prerelease selection consumes that verified version.

**Architecture:** Keep release numbering and validation in `tools/release.py`, and immutable proposal generation/ownership in `tools/release_pr.py`. Use the existing root workspace core (`0.1.0`) with a sequential channel suffix; ordinary commits and replacement drafts do not increment it. Main is still the release source; proposal manifests describe the version that the existing build stamping step will install.

**Tech Stack:** Existing Python standard library, Git, pinned release-plz/git-cliff, Make and GitHub Actions.

**Spec:** Theo's 2026-11-02 request: the open draft must be the upcoming alpha prerelease, with the actual next version in its title, version bump and changelog. His follow-up confirmed `0.1.0-alpha.1`, then `.2` per release. This supersedes #192's pending bootstrap-source decision and #389's commit-count numbering for release tags.

## Global constraints

- Generate `0.1.0-alpha.1` before any alpha; after its tag exists, the next eligible main commit proposes `0.1.0-alpha.2`. New commits alone leave the number unchanged.
- Existing tags consume their numbers even when publication fails; never reuse or move a tag. Conflicting tags fail explicitly.
- Alpha drafts can exist before #52 closes; actual alpha tagging still requires #52. Preserve the #57 beta transition, using its own sequential suffix beginning at `beta.1`.
- Keep `release-plz/<base-sha>` proposals immutable; verify replacements before closing predecessors unmerged, retaining branches and commits.
- Do not merge a release proposal or create tags/releases. Workflow delivery requires the existing host approval; no new dependencies, installs, credentials or legacy-host application.
- Only generated proposal root version and local lockfile versions may change. Preserve member manifests, external dependencies and all existing delta validation.
- Keep #537, the five-minute candidate gate and process ownership checks unchanged. Do not run local load or duplicate full suites.

## Review focus

- Missing, multiple, edited or stale drafts must refuse prerelease selection, not fall back to bootstrap.
- PR title, marker, immutable head, manifest version and source base must agree before their version is trusted.
- A failed tagged release consumes its number; generating or superseding a draft does not.
- A release tag must remain checkable after its proposal closes; `check-tag` must not require a currently open PR.
- Versioned generated changelogs must retain history, escaping and deterministic timestamps; retrying the same source must not rewrite a proposal.

## Task 1: Sequential prerelease calculation and validation

**Files:** `tools/release.py`, `tools/tests/test_release.py`.

**Interfaces:** Retain the `next-prerelease` and `check-tag` command entry points. Introduce one shared next-version calculation taking core, channel and known release tags; proposal generation may call it without the publication-checklist gate.

- [ ] Add entry-point tests: seven ordinary commits and no release tags produce `0.1.0-alpha.1`; additional commits keep `.1`; existing alpha `.1` makes the next source `.2`; beta transitions to `.1`.
- [ ] Add refusal tests for reused tags, stale source, off-main commits, missing known gaps, closed channel, stable release, invalid sequence and a tag conflicting outside main. Keep prior policy tests that still apply; replace commit-count expectations explicitly.
- [ ] Run the focused release tests RED, then implement the shared numbering and tag validation. Keep nightly `dev.N` versions separate from release numbering.
- [ ] Run `python3 -m unittest discover -s tools/tests -p test_release.py` GREEN. Fixture Git writes remain confined to temporary repositories.

## Task 2: Exact-version immutable draft and version consumption

**Files:** `tools/release_pr.py`, `tools/release.py`, `tools/tests/test_release_pr.py`, `tools/tests/test_release.py`, `tools/tests/release_pr_conformance.py`, `.github/workflows/release-plz.yml`, `.github/workflows/prerelease.yml`, `cliff.toml` if the existing template requires deterministic proposal dates.

**Interfaces:** `prepare` receives the selected channel explicitly. `next-prerelease` obtains the sole verified open App-owned proposal for its exact main source and reads its full version from the immutable head, rather than calculating a different tag at dispatch time.

- [ ] Add proposal entry-point assertions that title, root workspace version, every local lock entry and changelog heading all contain `0.1.0-alpha.1`.
- [ ] Extend existing real-Git/fake-GitHub cases: same-source retry is identical; newer main replaces the draft without consuming a number; an existing alpha tag advances the number; legacy stable-shaped drafts can be recognized and retired safely after a verified new proposal.
- [ ] Test prerelease selection with no draft, duplicate drafts, wrong author/repository, changed head/title/body, stale base, wrong channel, wrong next number and inconsistent manifest/lock data. Every case refuses without writes.
- [ ] Run focused tests RED, then implement full prerelease validation and generation. Reuse existing version stamping and delta guards; keep tag/publication execution out of proposal code.
- [ ] Preserve historical `check-tag` validation without an open-draft dependency. Require the live draft only when selecting a future prerelease.
- [ ] Wire read-only proposal/checklist metadata access into the appropriate workflow steps and path filters. Token scope remains repository-only; the generator receives no App token.
- [ ] Adapt pinned-tool conformance to an alpha `.1`/`.2` sequence in a disposable repository, preserving deterministic retry, history, Markdown escaping and branch retention. Inspect installed tool options before choosing generator flags; install nothing.
- [ ] Run `make release-pr-check` and available pinned-tool conformance once. Record missing pinned tools as not-run rather than installing or claiming success.

## Task 3: Policy, review and delivery evidence

**Files:** `docs/RELEASING.md`, new `docs/decisions/2026-11-02-192-exact-prerelease-proposals.md`, relevant row of `docs/design/greenfield/16-feature-coverage-map.md`.

- [ ] Document exact draft versions, per-release sequence, consumed failed-tag numbers and unchanged approval/checklist gates. Mark the old bootstrap-source/commit-count policy superseded without altering historical evidence.
- [ ] Run focused release/proposal tests, Ruff check/format, workflow-syntax tests, docs checks and `git diff --check`; record literal outcomes. No broad Rust suite is needed for Python/workflow/docs-only changes unless review reveals a reason.
- [ ] Execute installed OpenCode `code-review medium` on the full #192 delta. Obtain the separately required bounded independent review through the coordinator using OpenAI; preserve both records distinctly.
- [ ] Commit only requested #192 files under existing feature-branch permission. Report the diff stat and host-executed workflow/Make changes. Request the normal workflow push through host approval after checking existing receipts.
- [ ] Inspect replacement draft CI before claiming live acceptance. Leave release-draft merge, tags and publication unperformed; #523 remains blocked independently by #537.

## Plan check and execution boundary

The three tasks cover the requested draft, numbering, consumer and durable checks together. All five review-focus cases have explicit tests above. Implementation is inline/OpenAI as requested for this lane; no Claude workers. Await Theo's plan confirmation before implementation, then use an isolated #192 checkout based on verified current main.
