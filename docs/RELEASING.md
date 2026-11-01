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

## Versions

- Cockpit uses [Semantic Versioning](https://semver.org/) and stays on `0.x` until 1.0.
  Under `0.x`, as in Cargo, a breaking change raises the minor number (`0.3.2` → `0.4.0`)
  and anything else raises the patch number (`0.3.2` → `0.3.3`).
- There is one version: `[workspace.package] version` in the root `Cargo.toml`, inherited by
  every crate (`version.workspace = true`). Nobody edits it by hand. It stays the bootstrap's
  `0.1.0` while no stable tag exists: a **release-plz** release PR (#192) may prepare the next
  version and `CHANGELOG.md`, but merging it tags a stable `vX.Y.Z`, so nobody merges it until
  Theo decides on a stable release.
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
