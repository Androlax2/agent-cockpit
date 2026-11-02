# Releasing

How Cockpit is versioned and shipped (issue #94), modelled on
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

The pre-release workflow consumes the repository Actions variable `RELEASE_APP_CLIENT_ID`
and secret `RELEASE_APP_PRIVATE_KEY`. Its token requests contents write only. The private
App also needs pull-request write for the approved #192 release-PR work; that workflow and
its history-preserving draft workflow are described below. No workflow permission is added
by this setup script.

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
PR runs test only with read permissions; the App token is created only in the main-only
proposal job, after generation, with contents/write and pull_requests/write on this repository.

`release-plz update` 0.4.2 calculates the inherited workspace version in git-only mode;
`git-cliff` 2.14.2 writes the unreleased changelog. No matching stable tag means an initial
candidate at `0.1.0`, not an invented bump. A fixture with a previous tag and a conventional
fix proves the later `0.1.0` to `0.1.1` bump. Semver checks and dependency updates are off;
member manifests and external lockfile dependencies may not change. Local generation runs
offline after fetching the existing locked dependencies. Both commands must already exist;
the script never installs tools itself.

The prepared commit uses the base commit's timestamp, so a fresh checkout of the same base
reproduces the same head. It is pushed normally to `release-plz/<full-base-sha>`. A repeat
reuses only the exact branch and open automation-owned draft; a different head, closed PR,
human author or promoted non-draft target causes refusal. An interrupted push or PR creation
can resume without replacing history. A lost response is not proof that the write failed.

Only after fetching and verifying the replacement draft does the script cross-link and
close older drafts owned by `agent-cockpit-release[bot]`, with matching source/head markers
and an ancestor base. It checks again before closing. Edited, promoted or unrelated drafts
are retained. Comments are deduplicated on retries. All old branches and commits remain;
there is no deletion, force push, rebase, merge or automatic approval. If `main` advances,
the stale run refuses instead of superseding current work. The workflow serializes its
runs; operators should leave automation branches untouched and put feedback on the PR.

These are review proposals, not releases. `release-plz release-pr` is never called, since
its update path rewrites history. Tag creation, registry publication and GitHub Releases
are disabled in [release-plz.toml](../release-plz.toml), and there is no merge-triggered
publication workflow. Pre-release tagging continues to follow #389 below; candidate drafts
do not change its bootstrap version rule.

Verification: `make release-pr-check` uses real temporary Git repositories, a bare remote
and fake GitHub; `make release-pr-conformance` additionally runs both pinned tools on a
throwaway unpublished workspace and verifies initial version, bump, changelog, draft
replacement and deterministic retry. It creates tags only in that disposable fixture.
A live draft workflow run must be linked separately before claiming live acceptance.

## Versions

- Cockpit uses [Semantic Versioning](https://semver.org/) and stays on `0.x` until 1.0.
  Under `0.x`, as in Cargo, a breaking change raises the minor number (`0.3.2` → `0.4.0`)
  and anything else raises the patch number (`0.3.2` → `0.3.3`).
- There is one version: `[workspace.package] version` in the root `Cargo.toml`, inherited by
  every crate (`version.workspace = true`). Internal path dependencies have no version pins.
  `tools/release.py stamp VERSION` changes only that root version line; Cargo then updates
  workspace package entries in `Cargo.lock`. Nobody edits the version by hand. It stays the bootstrap's
  `0.1.0` while no stable tag exists: a **release-plz** release PR (#192) may prepare the next
  version and `CHANGELOG.md`. Nobody merges that draft until Theo decides the release policy;
  the proposal workflow has no tag or publication step, even after a merge.
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

- **A pre-release tags `main` without a version bump.** `X.Y.Z` is the workspace's current
  `0.1.0`, and no release PR is needed.
- **`N` is the commit count since the base**, `git rev-list --count <base>..<commit>`: at
  least 1 and growing with `main`, with at most one pre-release per commit.
- **Nobody tags by hand.** Theo dispatches the prerelease workflow on `main`; it tags the
  commit it was dispatched on, with an app token (a tag pushed with `GITHUB_TOKEN` starts no
  workflow). A dry run prints the tag and creates nothing. The suffix is `-alpha` until the
  Beta 1 checklist (#57) closes, `-beta` after. It refuses:
  - an alpha while the Alpha 1 checklist (#52) is open;
  - a shallow clone;
  - a commit that already has a pre-release tag;
  - a version other than the workspace's `0.1.0`;
  - a tag not above the newest pre-release, or a commit that does not descend from it.
- **The release workflow checks the tag first.** It refuses a pre-release tag in every case
  above, however the tag was made, and a beta tag while the Beta 1 checklist is open. On a
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
| alpha | the Alpha 1 checklist (#52) closes | Installs on Linux, macOS and WSL2; each release lists its known gaps; data may be reset between releases |
| beta | the Beta 1 checklist (#57) closes | Everything alpha promises, and upgrades keep state: a beta upgrades in place from the previous beta without losing data |

Every later release milestone ships on the beta channel. A stable channel and 1.0 are not
planned: Theo decides them once users have used the betas for a long time.

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

## Who does what

- Agent sessions write PRs with conventional titles; they never tag, dispatch the prerelease
  workflow, merge a release PR or publish.
- Theo dispatches pre-releases once the channel's checklist issue is closed (see
  [Channels](#channels-and-what-each-promises)), and decides when a stable release exists.
- The release workflow builds, runs report 13's gate on the tagged commit, signs and
  publishes.

## Archive TUI smoke check

The archive includes `cockpit-tui` beside `cockpit`. The release install jobs run
`tools/release.py verify-install` on the unpacked archive; it checks the manifest checksums
and runs `cockpit tui --version` to execute that sibling and check its compiled version.
This headless check needs no terminal or manager session; interactive TUI coverage remains
in the existing terminal tests.
