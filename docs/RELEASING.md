# Releasing

How Torlek is versioned and shipped (issue #94), modelled on
[AprilNEA/OpenLogi](https://github.com/AprilNEA/OpenLogi)'s release system. The release
bundle itself, its packaging and its gate are [report 13](design/greenfield/13-release-upgrades.md);
this file is the policy every tag follows. Releases are cut from the greenfield workspace
(root [Cargo.toml](../Cargo.toml)); `apps/` is never tagged.

Issue #96 picked the tools ([decision](decisions/2026-10-31-96-release-tooling.md)): the
[release](../.github/workflows/release.yml) and [pre-release](../.github/workflows/prerelease.yml)
workflows with [tools/release.py](../tools/release.py) build, check and publish, git-cliff
writes the notes, and release-plz prepares the release PR (#192); no
cargo-dist. The version source, tag grammar, changelog and channel promises here are the
contract.

The release workflow runs on release tags. Its nightly schedule and a manual dispatch, the
build before a tag, run it as a dry run: built and installed, never attested or released. A
pull request starts it only when it changes the release machinery itself (the workflow's
`pull_request` paths); `ci.yml` builds and tests crate changes.

## Release App setup

The pre-release workflow consumes repository variable `RELEASE_APP_CLIENT_ID` and
`RELEASE_APP_PRIVATE_KEY` through the protected `release` environment. Its installation
token requests contents/write on `Androlax2/agent-cockpit` only. Release App `5573576`
(`agent-cockpit-release`) is the sole `v*` creation bypass; separate rules prohibit
every tag update/deletion, including by that App.

Tagging, release drafting and publication reference `release`, whose reviewer is Theo
(`39646949`), with administrator bypass disabled and only branch `main`/tag `v*` allowed.
A manual prerelease dry run also requires approval. Different jobs or runs may require
separate approvals. Automatic proposal drafts use the independent `release-proposal`
environment and require no reviewer.

The existing Release App is already registered. The bootstrap instructions below describe
its historical repository-secret setup; do not replay `configure` after migration,
because its storage contract is repository-level. Preserve its private recovery record.
The repository key has not yet been migrated or removed: environment references alone
do not establish environment-only storage. Migration, old-run/token-expiry evidence and
live negative acceptance remain separately approved work under #382. See the measured
[repository protections](security/repository.md).

An operator uses [tools/setup_release_app.py](../tools/setup_release_app.py) on their host
with existing Python 3.11+, OpenSSL and GitHub CLI, authenticated as `Androlax2`. The host
credential must be able to read repository Actions variables/secret metadata and create
the variable/secret. The App itself gets no Actions, secrets or administration permission.
Run one operator at a time, with no concurrent credential/settings changes. The script
supports POSIX hosts with `fcntl`; Windows users need WSL. It installs nothing.

For agents, each host command requires `ask-to-run`. Ask browser readiness through
`cockpit-needs-input` before `ask-to-guide`. Registration and installation each need human
browser confirmation; this is not unattended App creation.

1. Run `python3 tools/setup_release_app.py register` through host approval. Open its loopback
   URL on the same host, signed into GitHub as `Androlax2`, then confirm registration.
   The form registers a private App with contents/write and pull_requests/write only
   (GitHub adds metadata/read), no events and an inactive webhook. GitHub may require a
   unique App name; the returned identity is recorded, not guessed.
2. Use the printed App installation link. Select **Only select repositories**, choose only
   `agent-cockpit`, and confirm installation. Do not select all repositories.
3. Run `python3 tools/setup_release_app.py configure` through host approval. It authenticates
   the saved key, verifies App/installation permissions and owner, and requires exactly one
   selected repository: `Androlax2/agent-cockpit`. A temporary metadata-only installation
   token checks the complete installation repository list and is revoked before configuration.
   The client ID becomes the repository variable; `gh secret set` receives the PEM on stdin
   and encrypts it locally. No key, token, conversion code or raw API/CLI error is printed.

The private recovery record is `~/.local/state/cockpit-release-app/state.json` (directory
0700, file 0600); `--state-dir` can choose another private location outside Git checkouts.
Keep it private and backed up according to the host's credential policy. It retains the
App key for scope checks and recovery; unused client/webhook secrets are discarded. A
crash while signing may leave a private `.signing-key-*` file in that directory. Never
paste or commit these files, and do not display the record during troubleshooting.

Repeated successful registration prints the existing installation link. Repeated
configuration verifies scope and the recorded secret timestamp, without writing anything.
GitHub cannot return secret values, so this is not a claim that an existing secret's value
was read or compared. Mismatched variables, unrecorded secrets and changed configuration
are refused. An interruption during registration/conversion or secret upload also refuses
automatic retry: inspect GitHub App settings and repository configuration, preserve the
private record, and obtain a reviewed recovery action. Do not delete the record and rerun
blindly; a registered App or uploaded secret may already exist. No credential rotation or
deletion is automated. GitHub secret writes have no conditional-create operation; the
preflight check cannot protect against a different operator writing concurrently.

This setup creates no tags, release PRs or releases and dispatches no workflows. Fixture
tests cover the command entry points and transports. The repository-only App setup and
unchanged second verification passed through host approval on 2026-11-02;
[the live evidence is recorded on #192](https://github.com/Androlax2/agent-cockpit/issues/192#issuecomment-6567550339).

Official API references: [manifest registration](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest),
[App REST endpoints](https://docs.github.com/en/rest/apps/apps),
[App installation](https://docs.github.com/en/apps/using-github-apps/installing-your-own-github-app),
[JWT signing](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-json-web-token-jwt-for-a-github-app),
and [GitHub CLI secret input](https://cli.github.com/manual/gh_secret_set).

## Release proposal drafts

On a push to `main`, [release-plz.yml](../.github/workflows/release-plz.yml) tests the
proposal code and pinned generators, then prepares a draft using
[tools/release_pr.py](../tools/release_pr.py). Manual dispatch defaults to a local dry run.
PR runs test only with read permissions. The main-only `proposal` job references
`release-proposal`, restricted to branch `main`. It prepares files before creating any
App token, then uses `PROPOSAL_APP_CLIENT_ID` and `PROPOSAL_APP_PRIVATE_KEY` to mint a
contents/read + pull_requests/write token for this repository only. Its `GITHUB_TOKEN`
stays read-only. `PROPOSAL_GIT_SSH_KEY` is injected only into the publication step for
the fixed-repository SSH branch push, with a pinned GitHub host key, strict checking,
no agent forwarding and temporary-file cleanup.

The verified proposal App is `5582369`, slug `agent-cockpit-proposal`, bot
`agent-cockpit-proposal[bot]`, numeric bot ID `351595931`. Its Git-only deploy key is
`168493357`; it has no ruleset bypass and no release REST credential. Bootstrap and
scope verification use [tools/setup_proposal_app.py](../tools/setup_proposal_app.py)
and separate private state `~/.local/state/cockpit-proposal-app`, never the Release
App's recovery record. Proposal registration, verification and environment-only
configuration are already complete; do not replay them as setup.

The draft carries the **exact upcoming prerelease** in its title, workspace manifest,
local lockfile entries and changelog heading: first `0.1.0-alpha.1`, then `.2` after the
first tag. Ordinary commits and replacement drafts do not consume numbers. A tag consumes
its number even if publication fails. Beta numbering starts separately at `0.1.0-beta.1`.
The workflow reads the Beta 1 checklist (#57) to select its channel; alpha proposals can
exist while #52 is open, but actual alpha tagging still waits for #52 to close.

`release-plz update` 0.4.2 runs in git-only mode, then the shared release numbering policy
stamps the exact version and `cargo update --workspace --offline` updates local lock entries.
`git-cliff` 2.14.2 generates the complete versioned changelog; its proposed release timestamp
is fixed to the source commit before rendering. Semver checks and dependency updates are off;
member manifests and external lockfile dependencies may not change. Generation runs offline
after fetching existing locked dependencies. Tools must already exist; the script installs nothing.

The prepared commit uses the base commit's timestamp, so a fresh checkout of the same base
reproduces the same head. It is pushed normally to `release-plz/<full-base-sha>`. A repeat
reuses only the exact branch and open automation-owned draft; a different head, closed PR,
human author or promoted non-draft target causes refusal. An interrupted push or PR creation
can resume without replacing history. A lost response is not proof that the write failed.

**A channel transition requires a deliberate new main commit.** After the Beta 1
checklist (#57) closes, merge a reviewed beta-transition change, such as its release
policy or known-gap update, through the normal protected PR process. That push to
main triggers generation of `0.1.0-beta.1` at a new `release-plz/<full-base-sha>`
identity. Closing #57 alone does not regenerate a proposal. A manual beta proposal
for an already-used alpha source refuses rather than rewriting its branch; the alpha
draft stays intact until a verified replacement exists. Use a meaningful transition
change, not an unrelated commit solely to obtain another SHA.

Only after fetching and verifying the replacement draft does the script cross-link and
close older drafts owned by `agent-cockpit-proposal[bot]` with numeric ID `351595931`,
matching source/head markers and an ancestor base. It checks again before closing.
Existing old-author drafts require separate exact owner-approved cleanup after replacement
verification; no old/new author allowlist is introduced. Edited, promoted or unrelated drafts
are retained. Comments are deduplicated on retries. All old branches and commits remain;
there is no deletion, force push, rebase, merge or automatic approval. If `main` advances,
the stale run refuses instead of superseding current work. The workflow serializes its
runs; operators should leave automation branches untouched and put feedback on the PR.

These are review proposals, not releases. `release-plz release-pr` is never called, since
its update path rewrites history. Tag creation, registry publication and GitHub Releases
are disabled in [release-plz.toml](../release-plz.toml), and there is no merge-triggered
publication workflow. Do not merge the draft: pre-release dispatch verifies and consumes
its version, tags its exact main source and lets the build stamp that version. Dispatch
requires exactly one open proposal, verified App ownership, repository, immutable head,
marker, parent, current source, title, version files and changelog. Missing, multiple,
edited, stale or wrong-version proposals refuse; there is no bootstrap fallback. Older
stable-shaped proposals can be retired after a verified replacement, but cannot be consumed.

**The final successful verification freezes a release snapshot**, not a transaction
across GitHub resources. For live tagging, the workflow obtains its write token first, reads the checklist
states and validates the sole proposal, including its final metadata, branch head and
current main. It records `tag`, `channel`, `revision`, `proposal_number` and `proposal_head`
in the workflow output, then uses that captured revision for the tag request in the same
shell step. A failed verification prevents the request. Dry runs record the same snapshot
without requesting a tag.

Changes observed during verification refuse selection. Once verification succeeds, later
main advancement or proposal edits, closure or replacement do **not** revoke that snapshot;
the tag still names its captured source. This is Theo's approved snapshot boundary, not a
claim of atomic latest-main selection. GitHub reads and create-reference are separate
operations; an Actions concurrency group does not lock human edits or merges. Existing
tag uniqueness and publication/checklist gates still apply.

Verification: `make release-pr-check` uses real temporary Git repositories, a bare remote
and fake GitHub; `make release-pr-conformance` additionally runs both pinned tools on a
throwaway unpublished workspace and verifies alpha `.1`/`.2`, changelog, draft
replacement and deterministic retry. It creates tags only in that disposable fixture.
A live draft workflow run must be linked separately before claiming live acceptance.

## Cut a release

Each prerelease draft's "How to release" section carries these steps with its exact tag and
source commit (#669). Do not merge the draft.

1. Dry run: `gh workflow run prerelease.yml -R Androlax2/agent-cockpit --ref main -f dry_run=true`,
   then approve the `release` environment on the
   [run](https://github.com/Androlax2/agent-cockpit/actions/workflows/prerelease.yml). It
   prints the tag and source it would create and creates nothing.
2. Release: `gh workflow run prerelease.yml -R Androlax2/agent-cockpit --ref main -f dry_run=false`,
   then approve the `release` environment on the run. It tags the draft's main source.
3. The tag starts the Release workflow: approve its `release` jobs as its gates pass
   ([runs](https://github.com/Androlax2/agent-cockpit/actions/workflows/release.yml)). It
   drafts, then publishes the release.

Among its checks, the dispatch refuses before tagging unless the Alpha 1 checklist (#52) is
closed (once the Beta 1 checklist, #57, closes, only a beta is cut), an alpha's source has
`docs/KNOWN-GAPS.md`, `main` is still the draft's base and the draft is the only open,
unedited proposal, and the draft's version is the next prerelease number. The full list is
under [Tags](#tags). Drafts published before #669 lack the section; the next
proposal supersedes them as usual, while dispatch consumes only a draft that carries it.

## Versions

- Torlek uses [Semantic Versioning](https://semver.org/) and stays on `0.x` until 1.0.
  Under `0.x`, as in Cargo, a breaking change raises the minor number (`0.3.2` → `0.4.0`)
  and anything else raises the patch number (`0.3.2` → `0.3.3`).
- There is one version: `[workspace.package] version` in the root `Cargo.toml`, inherited by
  every crate (`version.workspace = true`). Internal path dependencies have no version pins.
  `tools/release.py stamp VERSION` changes only that root version line; Cargo then updates
  workspace package entries in `Cargo.lock`. Nobody edits the version by hand. Main retains
  the bootstrap core `0.1.0`; the unmerged release proposal carries the exact upcoming
  prerelease version and `CHANGELOG.md`. The proposal workflow has no tag or publication step.
- The release workflow owns the GitHub Release: it creates a draft, attaches every asset, runs
  report 13's release gate on the tagged commit, and publishes last, so an immutable release
  never locks before its assets exist.
- A tag is never moved, reused or re-run; a failed or missing artifact ships with the next
  tag. A gate check that did not run is a named gap that blocks publishing, not a pass.

## Tags

| Tag | Channel | Cut by | GitHub release |
| --- | --- | --- | --- |
| `vX.Y.Z-alpha.N` | alpha, from the Alpha 1 release until Beta 1 | the dispatched prerelease workflow | pre-release, never latest |
| `vX.Y.Z-beta.N` | beta, from the Beta 1 release on | the dispatched prerelease workflow | pre-release, never latest |

No other tag shape is a release tag. **No stable `vX.Y.Z` tag and no 1.0 until Theo decides**
(#389): no stable release is planned until users have used the betas for a long time, and the
release workflow refuses every `vX.Y.Z` tag with that reason, whoever made it.

**The base** is the greenfield bootstrap commit `d2d8604` (issue #18). A refused `vX.Y.Z` tag
left on `main` is not a release and never the base.

- **A pre-release tags the verified proposal's main source.** `X.Y.Z` remains `0.1.0`;
  the open draft must carry the exact `X.Y.Z-alpha.N` or `X.Y.Z-beta.N` version being tagged.
- **`N` is sequential per core and channel**, starting at 1 and advancing after a tag,
  with at most one pre-release per commit. Failed tagged attempts consume their numbers;
  a conflicting existing tag is refused, never moved. This supersedes #389's commit-count
  numbering ([decision](decisions/2026-11-02-192-exact-prerelease-proposals.md)).
  Nightly `dev.N` builds still count commits since the bootstrap.
- **Nobody tags by hand.** Theo dispatches the prerelease workflow on `main`; it tags the
  commit it was dispatched on, with an app token (a tag pushed with `GITHUB_TOKEN` starts no
  workflow). A dry run prints the tag and creates nothing. The suffix is `-alpha` until the
  Beta 1 checklist (#57) closes, `-beta` after. It refuses:
  - an alpha while the Alpha 1 checklist (#52) is open;
  - a source other than current `main`, or an absent/unverified open proposal;
  - a shallow clone;
  - a commit that already has a pre-release tag;
  - a version other than the workspace's `0.1.0`;
  - a tag not above the newest pre-release, or a commit that does not descend from it.
- **The release workflow checks the tag first.** It refuses a pre-release tag in every case
   above that apply to tagged history, however the tag was made, and a beta tag while the Beta 1 checklist is open.
   Tag checking uses Git history, not a currently open draft, so closing a proposal does not
   invalidate its tag. On a
  pre-release, the workspace version must still be the base's `0.1.0`. A pre-release build
  stamps its own version into `Cargo.toml` and `Cargo.lock` before compiling, and commits
  nothing.

## Channels and what each promises

Milestones are releases (decided 2026-11-01): Alpha 1, Alpha 2, Beta 1, Beta 2: cutover,
Beta 3: designed UI ([CONTRIBUTING](../CONTRIBUTING.md#milestones)). A channel opens when its
first release milestone's checklist issue is closed with its proof linked; no tag of a channel
exists before. A channel's promise is a release blocker, checked by
[report 13's release gate](design/greenfield/13-release-upgrades.md#release-gate) on the tagged
commit. The gate's upgrade check depends on the channel: an alpha may reset data, so only
clean installs block it; a beta must upgrade from the previous beta.

| Channel | Opens after | Promise |
| --- | --- | --- |
| alpha | the Alpha 1 checklist (#52) closes | Alpha 1 installs from archives on Linux and WSL2; each release lists its known gaps; data may be reset between releases |
| beta | the Beta 1 checklist (#57) closes | Everything alpha promises, and upgrades keep state: a beta upgrades in place from the previous beta without losing data |

Theo's [2026-11-02 scope decision](decisions/2026-11-02-568-alpha-platform-scope.md) defers macOS artifacts/installers, Homebrew distribution and macOS acceptance beyond Alpha 1 under #568. No experimental macOS download or restoration milestone is promised; restoration needs evidence and an explicit owner decision. Other packaging (#563/#565) moves to Alpha 2; the Arch AUR recipe (#562) is prepared in Alpha 1 beside the archive, not as its prerequisite ([decision](decisions/2026-11-03-562-arch-packaging-alpha1.md)). Linux/WSL2 archive-install, sandbox and human acceptance still gate Alpha 1; historical macOS results and ordinary repository-required development checks remain separate.

Every later release milestone ships on the beta channel. A stable channel and 1.0 are not
planned: Theo decides them once users have used the betas for a long time.

## Candidate checks

The Alpha1 release plan selects Linux x86_64 and aarch64 archives; WSL2 uses the x86_64
archive with separate gates. macOS distribution and acceptance are deferred under #568;
the retained future macOS jobs do not establish support. The `compatibility` job runs
the archive's daemon through protocol-skew and killed-daemon restart fixtures, plus
linked migration and interrupted-restore tests from the same source
revision. `make release-compatibility` owns those commands.

`make release-responsiveness` is CI-only: five minutes against the verified Linux archive,
with at most two build stand-ins and two database writers. It requires successful load
operations, held-connection p95 ≤ 100 ms and p99 ≤ 250 ms, and every measured response within
one second. The workflow uploads `candidate-responsiveness.md`, including the candidate
revision, even when a target fails. It records phase progress before setup and saves samples
before shutdown. Sampling stops at the window plus one second; incomplete probes fail the
gate. Load draining and build cleanup are bounded. Do not run it as a local stress loop.

`make release-upgrade-fetch` selects the newest published earlier release of the same
channel and downloads its matching archive and checksums with read-only GitHub access.
`make release-upgrade` verifies the checksum, manifest and source revision, then stops the
old daemon before starting the candidate in one private temporary home. Project/task state
must survive, and new writes must work. Tested processes receive no GitHub credentials.
Alpha1 publication requires successful applicable Linux and WSL2 jobs; their presence
is not proof of acceptance. Future macOS gates remain pending a separate verified
restoration decision, alongside the other beta publication requirements.

Alpha keeps its explicit upgrade exemption. Development dry runs have no release channel.
Both run the state fixture twice with the current candidate and print `upgrade=not-run`;
that is orchestration coverage, not proof of a previous-release upgrade. A beta without a
published earlier beta fails by name. Cross-version running-session retention is not yet
covered by this fixture and remains a named beta publication blocker in `GATE_CHECKS`;
same-candidate restart tests are separate evidence. Track these
unmeasured outcomes in [#191](https://github.com/Androlax2/agent-cockpit/issues/191).

## Changelog

[CHANGELOG.md](../CHANGELOG.md) is the only changelog. **git-cliff** generates it from the
squash-merge subjects on `main`; nobody writes it by hand. It needs:

- `main` to take squash merges only: a repository setting that is not on yet (PR #74 was
  still a merge commit), which Theo switches;
- each PR title to be a conventional commit subject. GitHub appends the PR number on merge:

```text
type(scope): description (#N)
```

`type` is one of `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`;
`scope` is the crate or area without its `cockpit-` prefix (`feat(daemon): …`); the issue is
referenced in the PR body, not the title. The first release covers the subjects since the
bootstrap commit `d2d8604`; those from before this policy are not conventional, and #96
decides how git-cliff groups them.

A pre-release's notes compare with the release tag highest in version order below it that is an
ancestor of its commit, or with the base when there is none. Alpha notes carry a `Known gaps`
list; beta notes name any upgrade step.

Two release texts take their version from the release, so nobody edits them per tag:

- `tools/release.py package` writes the bundle's own version into its README's install step, and
  `verify-install` refuses a bundle whose README installs another one. The README on `main` asks
  GitHub for the newest release instead.
- `docs/KNOWN-GAPS.md` names no release. `tools/release.py known-gaps` heads it with the version
  for the notes and refuses a named release, a gap without an issue, and a gap whose issue is not
  open. `prerelease.yml` runs it before it creates an alpha tag; every dry run rehearses it.

## Who does what

- Agent sessions write PRs with conventional titles; they never tag, dispatch the prerelease
  workflow, merge a release PR or publish.
- Theo dispatches pre-releases once the channel's checklist issue is closed (see
  [Channels](#channels-and-what-each-promises)), and decides when a stable release exists.
- The release workflow builds, runs report 13's gate on the tagged commit, signs and
  publishes.

## Archive TUI smoke check

The archive includes `torlek-tui` beside `torlek`. The release install jobs run
`tools/release.py verify-install` on the unpacked archive; it checks the manifest checksums
and runs `torlek tui --version` to execute that sibling and check its compiled version.
This headless check needs no terminal or manager session; interactive TUI coverage remains
in the existing terminal tests.
