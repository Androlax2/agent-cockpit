# Releasing

How Cockpit is versioned and shipped (issue #94), modelled on
[AprilNEA/OpenLogi](https://github.com/AprilNEA/OpenLogi)'s release system. The release
bundle itself, its packaging and its gate are [report 13](design/greenfield/13-release-upgrades.md);
this file is the policy every tag follows. Releases are cut from the greenfield workspace
(root [Cargo.toml](../Cargo.toml)); `apps/` is never tagged.

The tools named below (release-plz, git-cliff, nfpm, minisign) are the proposal; issue #96
decides which pieces they provide and which come from `cargo-dist` or an `xtask`. Whatever
it picks, the version source, tag grammar, changelog and channel promises here are the
contract.

## Versions

- Cockpit uses [Semantic Versioning](https://semver.org/) and stays on `0.x` until 1.0.
  Under `0.x`, as in Cargo, a breaking change raises the minor number (`0.3.2` → `0.4.0`)
  and anything else raises the patch number (`0.3.2` → `0.3.3`).
- There is one version: `[workspace.package] version` in the root `Cargo.toml`, inherited by
  every crate (`version.workspace = true`). Nobody edits it by hand: a **release-plz** release
  PR, from a `release-plz/` branch, bumps it, with the regenerated `CHANGELOG.md` in the same
  commit.
- release-plz tags only, and only after its release PR merges; it never creates the GitHub
  Release (`git_release_enable = false`). The tag push starts the release workflow, which owns
  the GitHub Release: it creates a draft, attaches every asset, runs report 13's release gate
  on the tagged commit, and publishes last, so an immutable release never locks before its
  assets exist.
- A published tag is never moved, reused or re-run; a failed or missing artifact ships with
  the next tag.

## Tags

| Tag | Channel | Cut by | GitHub release |
| --- | --- | --- | --- |
| `vX.Y.Z` | stable | release-plz, on merging the release PR | release |
| `vX.Y.Z-beta.N` | beta | the dispatched prerelease workflow | pre-release, never latest |
| `vX.Y.Z-alpha.N` | alpha, until the M5 cutover | the dispatched prerelease workflow | pre-release, never latest |

No other tag shape is a release tag.

**The base** is the previous stable tag; before the first stable, it is the greenfield
bootstrap commit `d2d8604` (issue #18) at version `0.1.0`.

- **A pre-release tags `main` without a version bump.** `X.Y.Z` is the version the open
  release PR prepares; before the first stable, it is the base's `0.1.0`, which must not be
  tagged yet, and no release PR is needed.
- **`N` is the commit count since the base**, `git rev-list --count <base>..<commit>`: at
  least 1 and growing with `main`, with at most one pre-release per commit.
- **Nobody tags by hand.** Theo dispatches the prerelease workflow on `main`; it tags the
  commit it was dispatched on, with an app token (a tag pushed with `GITHUB_TOKEN` starts no
  workflow). A dry run prints the tag and creates nothing. The suffix is `-alpha` until the
  M5 gate issue (#71) closes, `-beta` after. It refuses:
  - a channel whose gate issue is still open (M3 for alpha, #71 for beta);
  - a shallow clone;
  - a commit that is a stable release or already has a pre-release tag;
  - a version not above the base's;
  - a tag not above the newest pre-release, or a commit that does not descend from it.
- **The release workflow checks the tag first.** On a stable tag, the workspace version must
  equal the tag's. On a pre-release, it must still be the base's. A pre-release build stamps
  its own version into `Cargo.toml` and `Cargo.lock` before compiling, and commits nothing.

## Channels and what each promises

Each channel opens at a milestone gate of the [refactoring plan](design/greenfield/20-refactoring-plan.md);
no tag of a channel exists before its gate issue is closed with its proof linked. A channel's
promise is a release blocker, checked by [report 13's release gate](design/greenfield/13-release-upgrades.md#release-gate)
on the tagged commit. The gate's upgrade check depends on the channel: an alpha may reset
data, so only clean installs block it; a beta must upgrade from the previous beta; a stable
from the previous stable.

| Channel | Opens after | Promise |
| --- | --- | --- |
| alpha | M3 gate (providers and onboarding) | Installs on Linux, macOS and WSL2; each release lists its known gaps; data may be reset between releases |
| beta | M5 gate (the cutover, #71) | Everything alpha promises, and upgrades keep state: a beta upgrades in place from the previous beta without losing data |
| stable 0.x | M5 gate (#71), per promoted beta | A beta build whose upgrade-keeps-state promise was verified on Linux, macOS and WSL2 |
| 1.0 | M6 gate and every claim of [report 15](design/greenfield/15-validation-open-questions.md) proven | Report 15's validation list holds for the tagged build; Semantic Versioning's 1.x compatibility applies from then on |

A stable `vX.Y.Z` promotes a beta: Theo merges the release PR only once the newest
`vX.Y.Z-beta.N` had its upgrade promise verified on the three platforms and nothing but the
release PR landed on `main` after it.

## Changelog

[CHANGELOG.md](../CHANGELOG.md) is the only changelog. **git-cliff** generates it from the
squash-merge subjects on `main`; nobody writes it by hand. It needs:

- `main` to take squash merges only (a repository setting);
- each PR title to be a conventional commit subject. GitHub appends the PR number on merge:

```text
type(scope): description (#N)
```

`type` is one of `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`;
`scope` is the crate or area without its `cockpit-` prefix (`feat(daemon): …`); the issue is
referenced in the PR body, not the title. The first release covers the subjects since the
base; older subjects there are not conventional, and #96 decides how git-cliff groups them.

A stable release's notes compare with the previous stable tag, so they include its
pre-releases. A pre-release's notes compare with the highest earlier release tag that is an
ancestor of its commit, or with the base for the first one. Alpha notes carry a `Known gaps`
list; beta and stable notes name any upgrade step.

## Who does what

- Agent sessions write PRs with conventional titles; they never tag, dispatch the prerelease
  workflow, merge a release PR or publish.
- Theo dispatches pre-releases and merges release PRs, under the gates in
  [Channels](#channels-and-what-each-promises).
- The release workflow builds, gates, signs and publishes.
