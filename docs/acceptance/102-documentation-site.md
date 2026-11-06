# Documentation site acceptance

Issue #102; PR #507 merged as **Part of**, with reader and released-version acceptance still open.
The exact design approval is recorded in the [r4 handoff](../design/102-documentation-layout-r4.md).

Alpha 1's public install/quickstart covers **Linux/WSL2 archive installation**, following [Theo's scope decision](../decisions/2026-11-02-568-alpha-platform-scope.md). macOS delivery, Homebrew and macOS acceptance are deferred beyond Alpha 1 under #568, not passed; other packaging (#562/#563/#565) moves to Alpha 2. The site stages this policy from the canonical README. Historical macOS CI results below remain test evidence, not an Alpha 1 support or installation claim. Linux/WSL2 installation and the reader's live session remain required.

## Implemented evidence

| Requirement | Proof |
| --- | --- |
| Canonical guide, install/quickstart, configuration, capabilities and support | `site/scripts/content.mjs` stages README sections without duplicating their prose; `site/tests/site.test.mjs` builds actual HTML and checks the first-session commands |
| Generated CLI/configuration reference | Actual binary help/defaults, empty temporary home and cleared environment; `site/tests/content.test.mjs` checks isolation with a fake executable whose filename contains spaces; page and `build.json` provenance |
| Instruction bundle, design reports, decisions, comparison and schema status | Canonical source staging; explicit #295 planned page; no generated-schema claim |
| Version prefixes and preserved copies | Real Astro builds at latest and a fixture prerelease prefix; `site/tests/versions.test.mjs` covers complete-catalog assembly, missing builds, unsafe names and overlapping directories; browser fixture switches both ways |
| Source and rendered links | Existing `make docs-ci`; `site/scripts/links.mjs` mounts output under its complete prefix and invokes offline lychee; regression fixture proves a missing anchor fails |
| Search and responsive themes | [Browser script](../../site/tests/browser.py), [rendered evidence](../design/evidence/102-site-browser.json) and [captures](../../design-plans/previews/102-web-desktop-light.png); light/dark, 320 px, results/empty/unavailable, keyboard selection, Close/Escape/focus and touch |
| PR build and separate Pages deploy | Merged `.github/workflows/docs.yml`, built-in version assembler, read-only build permissions and separate Pages/OIDC job; successful PR build/assembly and main deployment are recorded below |
| Discoverability | README badge and plain-text TUI help URL to the published latest guide; `cockpit-tui --test keys` opens the real help renderer and asserts the URL |

Exact commands and outcomes are recorded in [validation evidence](../design/evidence/102-site-validation.json).

## Validation boundaries

The prerelease-shaped build is a local fixture from this branch, not a published
tag or a downloaded release archive. The browser is headless Chromium with touch
emulation, not a physical mobile device. The 200% check changes root text size;
real browser zoom and exhaustive assistive-technology behavior remain unmeasured.
No live provider, user's home/state, service installation or credentials were used.

[Impeccable execution evidence](../design/evidence/102-site-impeccable.json)
separates its source scan, incomplete file-HTML scan and failed URL browser launch
from the successful Playwright interactions. The earlier FigCraft detector failure
remains historical evidence; no empty array from a failed scan counts as a pass.

## Measured first publication

Host-approved request `f24491c5f806` configured Pages in Actions mode without a
custom domain, set the repository website and squash-merged PR #507. The
[first main deployment](https://github.com/Androlax2/agent-cockpit/actions/runs/41388685000)
succeeded; Linux, macOS, WSL2 and documentation checks passed. A separate
host-approved anonymous HTTP check (`20389aa6abf8`) returned 200 for the build
metadata, quickstart and static search index, and verified source revision
`ea5458970720cf24dd807c21a1cdb808085ed5a4`.

[Publication evidence](../design/evidence/102-pages-publication.json) retains the
exact command, outcomes and unavailable sandbox checks. Local TLS/browser requests
timed out and web retrieval was unavailable; no network permission changed.
Interactive browser results come from the isolated build, not a public browser
pass. No install/session walkthrough or live release-tag deployment is claimed.

## Open release checkpoints

- A released version publishes its tagged copy without losing older copies:
  measured below once releases existed (see
  [released-version publication](#released-version-publication)).
- Public installation instructions on the site itself. The install page stages the
  README's archive steps (checksum, unpack, `torlek daemon install`, `torlek init`)
  from their canonical source, so a reader no longer has to leave for GitHub.
  Their use by a reader remains unmeasured.
- A reader installs and starts a session from the published site alone, coordinated
  with Chief. No volunteer has completed this acceptance walk yet.
- #98's greenfield fixture image is complete in PR #536; its
  [capture evidence](98-demo-screenshot.json) identifies the actual TUI renderer and
  fictional data. Public installation and volunteer acceptance stay open. Neither
  that fixture image nor local site screenshots prove a live session.

The [build guide](../../site/README.md) owns executable commands and tooling limits.
This record does not close either issue's live-validation criteria.

## Released-version publication

Measured 2026-11-06 against the live site and main workflow run
[42251269940](https://github.com/Androlax2/agent-cockpit/actions/runs/42251269940),
built from main revision `5174ca1f6870fe60b039027d5fa48610f9790ed0`. Anonymous
HTTP only; no credentials, provider or user state.

| Copy | Live | `build.json` revision | Pages | Build artifact |
| --- | --- | --- | --- | --- |
| latest | 200 | `5174ca1f6870` (main) | 201 | 7.6 MB |
| v0.1.0-alpha.4 | 200 | `87274175a5d5` (its tag) | 199 | 7.5 MB |
| v0.1.0-alpha.3 | 200 | `e1132851b779` (its tag) | 187 | 6.8 MB |
| v0.1.0-alpha.2 | 200 | `fb683e93a779` (its tag) | 173 | 5.9 MB |
| v0.1.0-alpha.1 | 200 | `2f620da3f8a7` (its tag) | 170 | 5.7 MB |

Reproduce with `curl https://androlax2.github.io/agent-cockpit/<copy>/build.json`
and `gh api repos/Androlax2/agent-cockpit/actions/runs/42251269940/artifacts`.
The figures are a dated snapshot and move with each release.

Every copy lists all five versions in its switcher. The assembled Pages artifact
was 33.2 MB. Each build job took about one minute (serial, `max-parallel: 1`),
assembly 21 seconds and deployment 1 min 45 s. Rebuilding every copy per publish
therefore costs about five minutes and grows by roughly 6 to 8 MB per release;
the preservation strategy needs no change at this size.

## Install steps on the site

The live install page linked to the README on GitHub and carried none of its
commands (no `sha256sum` or `torlek init` on `get-started/install`). The site now
stages the README's "Install with your AI agent" and "Install from the command
line" sections as `get-started/install-release` ("Install Torlek"), and the install status page links
there. `site/tests/site.test.mjs` asserts the built page carries the checksum,
daemon-install and init steps and that the status page links to it.

Still owner or volunteer work, not claimed here: a reader walking install and a
first session from the published site alone, and a custom domain (#339).

## Hosted CI bootstrap correction

The first site run [41385306721](https://github.com/Androlax2/agent-cockpit/actions/runs/41385306721)
failed before the Node install/build: the pinned install-action did not support
lychee 0.24.2 with fallback disabled. The correction reuses the existing `ci.yml`
checksum-verified download of that exact lychee release. The shared bootstrap
regression now covers both jobs (five tests pass); no site package pin, compiler
fallback or local installation changed. Hosted success was not observed in that checkpoint; consult PR #507 checks for the final outcome.

The corrected bootstrap passed in [run 41385864682](https://github.com/Androlax2/agent-cockpit/actions/runs/41385864682),
and the real static site export passed. One isolation fixture failed because its
`/usr/bin/env node` shebang could not find setup-node's tool-cache executable in
the deliberately cleared PATH. The fixture now names the running Node executable
with `process.execPath`; production environment isolation remains unchanged. All
eight focused content tests pass locally; the subsequent hosted outcome is recorded in PR #507 checks.

The complete site gate passed in [run 41386339863](https://github.com/Androlax2/agent-cockpit/actions/runs/41386339863).
Assembly exposed the pinned download action's flat layout for a single artifact,
confirmed in its [exact source](https://github.com/actions/download-artifact/blob/9000827ccba6bdab643e8b6fd33ac0654aef8333/src/download-artifact.ts).
The assembler now accepts that layout and the named directories used for multiple
artifacts. A real CLI fixture with spaces in its paths passes. Downloading this
run's actual artifact, assembling it locally and checking its complete repository
prefix also passed: 15,186 links, 901 unique, zero errors. No publication occurred.
