# Releasing

How Cockpit is versioned and shipped (issue #94). The release bundle itself, its packaging
and its gate are [report 13](design/greenfield/13-release-upgrades.md); this file is the
policy a tag must follow. Releases are cut from the greenfield workspace (root
[Cargo.toml](../Cargo.toml)); `apps/` is never tagged.

## Versions

- Cockpit uses [Semantic Versioning](https://semver.org/) and stays on `0.x` until 1.0.
  Under `0.x`, as in Cargo, a breaking change raises the minor number (`0.3.2` → `0.4.0`)
  and anything else raises the patch number (`0.3.2` → `0.3.3`).
- The version lives in one place: `[workspace.package] version` in the root `Cargo.toml`.
  Every crate inherits it, and a tag names exactly that version.
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
No tag of a channel exists before its gate issue is closed with its proof linked.

| Channel | Opens after | Promise |
| --- | --- | --- |
| alpha | M3 gate (providers and onboarding) | Installs on Linux, macOS and WSL2; each release lists its known gaps; data may be reset between releases |
| beta | M5 gate (the cutover) | Everything alpha promises, and upgrades keep state: a beta upgrades in place from the previous beta without losing data |
| 1.0 | M6 gate and every claim of [report 15](design/greenfield/15-validation-open-questions.md) proven | Report 15's validation list holds for the tagged build; Semantic Versioning's 1.x compatibility applies from then on |

A promise is a release blocker, not an aspiration: an alpha that does not install on one of
the three platforms, or a beta that loses state on upgrade, is not tagged.

## Changelog

[CHANGELOG.md](../CHANGELOG.md) is the only changelog. It is kept from pull request
titles: `main` is squash-merged, so each commit subject on `main` is a PR title ending in
its number, and a release entry lists those subjects since the previous tag:

```sh
git log --format='- %s' <previous-tag>..HEAD
```

The first release has no previous tag; its entry lists the PRs merged since the greenfield
workspace bootstrap (#18). Entries are newest first, headed `## v0.x.y-alpha.n — YYYY-MM-DD`.
Alpha entries carry a `Known gaps` list; beta and stable entries name any upgrade step.
Because titles become the changelog, a PR title says what changed for a user or a
contributor, not how.

## Cutting a release

1. Check the channel's gate issue is closed and the release gate in report 13 passed on the
   candidate commit; a check that did not run is a named gap, not a pass.
2. Open a PR that sets the version in the root `Cargo.toml` (and `Cargo.lock`) and adds the
   changelog entry. Merge it once CI is green.
3. Theo tags the merge commit with the version's tag and creates the GitHub release from the
   changelog entry, marked pre-release for alpha and beta.

Tagging and publishing are Theo's: agent sessions prepare the PR, they do not tag or publish.
