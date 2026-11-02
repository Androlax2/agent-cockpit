# Documentation site build validation

Issue #102. The Astro/Fumadocs dependency graph is approved and installed; the product site layout remains pending exact-frame approval.
The proposed layout is in the [design handoff](../docs/design/102-documentation-layout.md).

## Dependency approval

Theo approved the exact candidate versions recorded at decision commit `fd9194c`, the site-only manifest and lockfile, sandbox installation with scripts disabled, and build validation.
The [decision record](../docs/decisions/2026-11-02-102-documentation-generator.md) preserves the package list and boundaries.
No root Cargo manifest, global generator, host service, workflow or Pages setting changes here.

From the repository root, install using a writable sandbox-local cache:

```sh
npm_config_cache="$PWD/site/.npm-cache" npm --prefix site install --ignore-scripts --no-audit --no-fund
npm --prefix site ls --depth=0
npm --prefix site test
npm --prefix site run check:probe
```

`npm_config_cache="$PWD/site/.npm-cache" npm --prefix site ci --ignore-scripts --no-audit --no-fund` reproduced the lockfile successfully (exit 0; 373 packages).
The default npm cache in this sandbox is read-only: the first install failed with `EROFS` at `/home/theo/.npm/_cacache/tmp/***` (exit 1).
The local-cache retry succeeded and installed 373 packages (exit 0); all 16 approved direct pins remained unchanged.
`npm --prefix site ls --all --json` also exited 0.

## Isolated integration probe

`validation/` is a fixture, not the published documentation surface. It contains:

- Astro static configuration with the official Fumadocs integration and Tailwind plugin.
- A Fumadocs MDX collection and upstream Markdown components.
- A minimal React island, with ordinary fixture text and no product layout.
- A Fumadocs search endpoint exported as static JSON, built from the fixture collection.

`npm --prefix site test` drives the real Astro CLI and checks the emitted HTML, React island/assets, generated CSS and static search index. The initial test failed because no document was exported; the search assertion separately failed before the endpoint existed. The completed test passes (one test, zero skipped).
`check:probe` type-checks the fixture's TypeScript and TSX files with the approved TypeScript package; Astro compilation checks its page syntax. This is not a full Astro language-service check or browser test.

During iteration the fixture incorrectly called `page.data.load()` on an Astro collection entry. That helper belongs to Fumadocs' converted source object; using Astro's `render(entry)` corrected the fixture without a dependency change.
An initial test command also used an obsolete Astro executable path; the installed package declares `bin/astro.mjs`, which the test now uses.

## Remaining evidence

The CSS compiler reports two warnings about `::highlight(fd-search)` in upstream generated search styles. The test forwards build diagnostics; no warning or test was disabled. Browser search highlighting remains to be checked when the real layout is implemented.

The integration probe does not prove reader onboarding, keyboard/search interactions, responsive layout or versioned Pages deployment. Canonical content staging, generated references, the real site build, its CI wiring and publication remain work for #102.
The final independent full-delta review will run when that implementation is ready; this dependency checkpoint is not a completed site.

## Quality checks

`make docs-ci` passes on the staged source. Biome passes on all six new JS/TS files.
The broader `make lint-js` command exits 2: it reports one formatting error in the existing `crates/cockpit-opencode/plugin/cockpit-opencode-plugin.js`, plus 46 warnings and two informational diagnostics in existing source. That file has no delta from this branch's `origin/main` base; its focused Biome check reproduces the formatting failure. It remains outside this documentation change.
