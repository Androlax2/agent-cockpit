# Documentation site acceptance

Issue #102; PR #507 is **Part of**, pending publication and reader acceptance.
The exact design approval is recorded in the [r4 handoff](../design/102-documentation-layout-r4.md).

## Implemented evidence

| Requirement | Proof |
| --- | --- |
| Canonical guide, install/quickstart, configuration, capabilities and support | `site/scripts/content.mjs` stages README sections without duplicating their prose; `site/tests/site.test.mjs` builds actual HTML and checks the first-session commands |
| Generated CLI/configuration reference | Actual binary help/defaults, empty temporary home and cleared environment; `site/tests/content.test.mjs` checks isolation with a fake executable whose filename contains spaces; page and `build.json` provenance |
| Instruction bundle, design reports, decisions, comparison and schema status | Canonical source staging; explicit #295 planned page; no generated-schema claim |
| Version prefixes and preserved copies | Real Astro builds at latest and a fixture prerelease prefix; `site/tests/versions.test.mjs` covers complete-catalog assembly, missing builds, unsafe names and overlapping directories; browser fixture switches both ways |
| Source and rendered links | Existing `make docs-ci`; `site/scripts/links.mjs` mounts output under its complete prefix and invokes offline lychee; regression fixture proves a missing anchor fails |
| Search and responsive themes | [Browser script](../../site/tests/browser.py), [rendered evidence](../design/evidence/102-site-browser.json) and [captures](../../design-plans/previews/102-web-desktop-light.png); light/dark, 320 px, results/empty/unavailable, keyboard selection, Close/Escape/focus and touch |
| PR build and separate Pages deploy | Proposed `.github/workflows/docs.yml`, built-in version assembler, read-only build permissions and separate Pages/OIDC job; YAML parsed locally using the already installed transitive `yaml` package |
| Discoverability | README badge and plain-text TUI help URL, both labeled publication pending; `cockpit-tui --test keys` opens the real help renderer and asserts the URL |

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

## Open release checkpoints

- Pages enablement/environment policy, deployment and repository website field;
  no custom domain before #339. Host workflow-push approvals are recorded in the
  validation evidence; successful hosted checks remain the PR merge gate.
- Final public installation instructions following the merged installer/release
  implementation. Main's engine-install work #470 has progressed independently;
  it is not a measured public onboarding result for this branch.
- A reader installs and starts a session from the published site alone, coordinated
  with Chief. No volunteer has completed this acceptance walk yet.
- Keep #98's pending screenshot/public-install/volunteer evidence open. Local site
  screenshots do not constitute a greenfield product screenshot or a live session.

The [build guide](../../site/README.md) owns executable commands and tooling limits.
This record does not close either issue's live-validation criteria.

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
