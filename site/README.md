# Versioned documentation site

Issue #102. This static Astro/Fumadocs site renders canonical repository content,
with the [approved r4 layout](../docs/design/102-documentation-layout-r4.md).
Pages publication and reader onboarding acceptance remain pending.

## Build and check

The [generator decision](../docs/decisions/2026-11-02-102-documentation-generator.md)
records Theo's approval of the exact site-only package graph and ignore-scripts
installation. No product Cargo dependency or root package manifest is added.

From the repository root, with Node 26, the pinned Rust toolchain and the existing
lychee checker available:

```sh
npm_config_cache="$PWD/site/.npm-cache" npm --prefix site ci --ignore-scripts --no-audit --no-fund
make site-ci
```

`site-ci` builds only the CLI used for references (`--locked -j 2`), runs the Node
Node tests, checks site/probe TypeScript and checks rendered HTML with the existing
offline lychee command. It does not install missing tools. The Astro compiler
checks `.astro` syntax; the full Astro language-service checker is not installed.
For a build without tests, use `make site-build` after building the CLI.

The approved lockfile was reproduced with 373 packages and all direct pins
unchanged. The default npm cache failed with `EROFS`; the command above uses the
writable sandbox-local cache. Lifecycle scripts remain disabled.

## Content and provenance

`site/scripts/content.mjs` stages canonical Markdown into ignored `content/docs/`:
README guide sections, development/contribution guidance, the shipped agent
instruction bundle, greenfield reports and dated decisions. It preserves code
fences, rewrites mapped links to the selected version and leaves other source
links on GitHub. `site/content/` contains the comparison and explicitly planned
protocol reference. No generated protocol schema is claimed before #295.

CLI help and effective configuration defaults come from the actual built binary.
The generator clears its environment and uses a temporary empty home, explicit
`cockpit` profile, no daemon and no provider calls. It normalizes the temporary
home to `~`; defaults are not a complete configuration schema. Each page retains
its owning source path, revision and source hash. `build.json` records these plus
the binary hash, version, base path and version catalog. References from a release
tag use a source build of that tag, not an assertion about downloaded archives.

The default path is `/agent-cockpit/latest/`. Release builds set
`COCKPIT_DOCS_VERSION`, `COCKPIT_DOCS_BASE` and `COCKPIT_DOCS_VERSIONS`; the CLI path
can be selected with `COCKPIT_DOCS_BINARY`. Version and base inputs are validated.
Search is a static, version-local index; no search service or Node server runs on
Pages. Edit links target current source; revision links identify the viewed copy.

## Publishing proposal

The separate `.github/workflows/docs.yml` builds every PR without deployment.
Main builds `latest`; a successful tag-triggered Release run or a published-release
event rebuilds latest plus every published stable/alpha/beta version. Each version
uses its own source and lockfile. This does not change the release lane's scripts
or workflows and creates no tag or release. The successful `workflow_run` trigger
also handles releases published with `GITHUB_TOKEN`.

Build jobs have read-only repository permissions. A separate deployment job has
Pages/OIDC write permissions and uses the `github-pages` environment. The version
assembler validates the complete catalog before replacing its output; a missing
or incompatible historical build fails publication instead of removing that
version. There are currently no published releases. Rebuilding the whole catalog
is the initial preservation strategy; revisit storage when its measured cost
warrants it. A release predating this site cannot be silently skipped.

Workflow pushes, Pages enablement, environment policy, deployment and repository
website settings require their separate host review. No custom domain is set
while #339 remains open. README and TUI links say publication is pending.

## Browser verification

The existing Python Playwright environment runs `site/tests/browser.py` against an
isolated static server. Mount `site/dist/` at its complete base path under a
temporary server root, then run the script with `--url` pointing to
`get-started/first-session/`. `--output` selects its screenshot/evidence directory.
The script requires Playwright and an available Chromium; it installs neither.

The local run covers desktop and 320 px light/dark, client navigation and Back,
search results/no matches/index failure, keyboard result selection, Close/Escape
and focus restoration, touch Close and 200% root text scaling. Text scaling is not
a desktop-browser zoom test. A separate two-build fixture verified switching
between latest and a prerelease-shaped prefix; it is not a real release.
See [acceptance and evidence](../docs/acceptance/102-documentation-site.md).

## Known validation limits

Two upstream CSS optimization warnings report `::highlight(fd-search)` as an
unrecognized pseudo-element; diagnostics remain visible. Search interactions pass
in the tested Chromium, without a claim about every browser's highlighting.
Codex's `$code-review` workflow is installed at
`/home/theo/.codex/skills/code-review/SKILL.md`. The
[review record](../docs/acceptance/102-code-review.md) distinguishes its inline
medium review from the independent final PR review.

Earlier `make lint-js` failed on the unchanged OpenCode plugin's formatting,
with existing warnings/information. Focused new-source checks are recorded in the
acceptance evidence. No unrelated static-analysis failure is hidden or suppressed.
Public installation, a volunteer site-only session walkthrough, deployed Pages,
physical-device checks and exhaustive accessibility/performance conformance are
not established by the local build.
