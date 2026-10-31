# Releasing

How Cockpit is versioned and shipped (issue #94). The release bundle itself, its packaging
and its gate are [report 13](design/greenfield/13-release-upgrades.md); this file is the
policy a tag must follow. Releases are cut from the greenfield workspace (root
[Cargo.toml](../Cargo.toml)); `apps/` is never tagged.

## Versions

- Cockpit uses [Semantic Versioning](https://semver.org/) and stays on `0.x` until 1.0.
  Under `0.x`, as in Cargo, a breaking change raises the minor number (`0.3.2` → `0.4.0`)
  and anything else raises the patch number (`0.3.2` → `0.3.3`).
- The release version is `[workspace.package] version` in the root `Cargo.toml`; every
  crate inherits it, and a tag names exactly that version. Internal path dependencies also
  state `version = "…"`, and Cargo refuses a requirement that does not match: a release
  changes them together, to the same string (`"0.2.0-alpha.1"` matches only that
  pre-release and later ones of `0.2.0`).
- Tags, all on `main`:

| Channel | Tag | GitHub release |
| --- | --- | --- |
| alpha | `v0.x.y-alpha.n` | pre-release |
| beta | `v0.x.y-beta.n` | pre-release |
| stable | `v0.x.y` (later `vX.Y.Z`) | release |

`n` starts at 1 for each version and channel and only grows: `v0.2.0-alpha.1`,
`v0.2.0-alpha.2`, `v0.2.0-beta.1`, `v0.2.0`. A published tag is never moved or reused; a
broken release is fixed by the next one.

## Channels and what each promises

Each channel opens at a milestone gate of the [refactoring plan](design/greenfield/20-refactoring-plan.md).
No tag of a channel exists before its gate issue is closed with its proof linked. Every tag
also needs [report 13's release gate](design/greenfield/13-release-upgrades.md#release-gate),
whose clean installs check the install promises below.

| Channel | Opens after | Promise |
| --- | --- | --- |
| alpha | M3 gate (providers and onboarding) | Installs on Linux, macOS and WSL2; each release lists its known gaps; data may be reset between releases |
| beta | M5 gate (the cutover, #71) | Everything alpha promises, and upgrades keep state: a beta upgrades in place from the previous beta without losing data |
| stable 0.x | M5 gate (#71), per promoted beta | A beta build whose upgrade-keeps-state promise was verified on Linux, macOS and WSL2 |
| 1.0 | M6 gate and every claim of [report 15](design/greenfield/15-validation-open-questions.md) proven | Report 15's validation list holds for the tagged build; Semantic Versioning's 1.x compatibility applies from then on |

Until the M5 gate, every tag is a pre-release.

A promise is a release blocker, not an aspiration: an alpha that does not install on one of
the three platforms, or a beta that loses state on upgrade, is not tagged.

## Changelog

[CHANGELOG.md](../CHANGELOG.md) is the only changelog. It is kept from pull request
titles: `main` is squash-merged (`gh pr merge --squash`), so each commit subject on `main` is
a PR title followed by its PR number, and a greenfield title already ends in its issue number:
`cockpit-daemon skeleton: … (#26) (#109)` is issue #26, PR #109. A release entry lists those
subjects since the previous tag:

```sh
git log --first-parent --format='- %s' <previous-tag>..HEAD
```

`--first-parent` keeps a stray merge commit to one line instead of its branch's commits;
replace that line with the PR's title. The first release has no previous tag; its range
starts at the greenfield bootstrap commit, `d2d8604` (issue #18, PR #75), included:
`git log --first-parent --format='- %s' d2d8604^..HEAD`.

Entries are newest first, headed `## <tag> — YYYY-MM-DD` (`## v0.2.0-beta.1 — 2027-03-01`).
Alpha entries carry a `Known gaps` list; beta and stable entries name any upgrade step.
[Contributing](../CONTRIBUTING.md) says how to write a PR title for it.

## Cutting a release

1. Check the channel's gate issue is closed.
2. Open a PR that sets the version in the root `Cargo.toml`, in every internal path
   dependency and in `Cargo.lock`, and adds the changelog entry. Merge it once CI is green.
3. Run report 13's release gate on that merge commit, the one to be tagged; a check that did
   not run is a named gap, not a pass. A failure means no tag: fix it in a new PR and run
   the gate again on the new commit.
4. Theo tags the commit the gate passed on and publishes the release from the changelog
   entry in one step: `gh release create <tag> --target <commit> --title <tag> --notes-file
   <entry> --prerelease` (no `--prerelease` for stable).

Promoting a beta to stable follows the same steps; the promotion PR only changes
`0.x.y-beta.n` to `0.x.y`, so `git diff --stat v0.x.y-beta.n HEAD` lists only the manifests,
`Cargo.lock` and `CHANGELOG.md`.

Tagging and publishing are Theo's: agent sessions prepare the PR, they do not tag or publish.
