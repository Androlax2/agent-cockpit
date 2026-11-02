# Documentation dependency and build validation plan

> **For agentic workers:** Execute this approved validation scope inline; preserve the separate design and host-action approvals.

**Goal:** Resolve Theo's approved pins and prove Astro can build Fumadocs content statically before implementing the documentation layout.

**Architecture:** Keep all Node dependencies and their lockfile under `site/`. A small isolated validation fixture exercises Astro, Fumadocs MDX/React and CSS compilation without becoming the product documentation surface.

**Tech Stack:** Exact candidate versions from decision commit `fd9194c`.

**Spec:** [Dependency and generator decision](../../decisions/2026-11-02-102-documentation-generator.md).

## Global constraints

- Theo approved the exact dependency proposal, site-only manifest/lockfile, ignore-scripts sandbox install and build validation.
- Report incompatible pins before changing them. Do not use legacy-peer-deps, force a dependency resolution or install another framework.
- Proposed frames remain unapproved; do not implement the product layout.
- Workflow pushes, Pages settings and deployment retain separate host approval. No release file, host service or running Cockpit changes.
- Use the existing isolated clone and branch `greenfield/102-versioned-documentation`. Preserve unrelated untracked files.

## Review focus

- Every direct version must remain the approved one; use npm's actual installed dependency report, not only manifest text.
- Validate the real Astro build, not an import-only mock.
- Static output must contain fixture content and generated CSS without needing a server.
- The React/Fumadocs integration must participate in compilation; a plain Astro page alone is insufficient evidence.
- No installation script or host service may be enabled to conceal a build failure.

## Task 1 Dependency resolution

**Files:** `site/package.json`, `site/package-lock.json`, `site/.gitignore`, dependency decision record.

- [x] Record Theo's exact approval and create the private site-only manifest with approved direct pins.
- [x] Run `npm --prefix site install --ignore-scripts --no-audit --no-fund`; a sandbox-local npm cache is allowed when the default cache is read-only.
- [x] Inspect resolution errors before proceeding. Report incompatible pins rather than changing them.
- [x] Verify the actual graph with `npm --prefix site ls --depth=0` and check that no direct pin changed.

## Task 2 Build probe

**Files:** `site/validation/astro.config.mjs`, `site/validation/src/` fixture, `site/tests/build.test.mjs`, and a manifest validation script.

- [x] Write the entry-point test first: run the actual Astro CLI against the fixture and assert the exported HTML contains the MDX heading and content and links generated assets. Assert a hydrated React fixture is present.
- [x] Run it before implementing the fixture and confirm the static output assertion fails.
- [x] Add the smallest Astro/Fumadocs integration fixture using standard upstream MDX components and CSS imports. Use plain fixture text rather than the proposed product layout.
- [x] Run the test again and inspect actual output and diagnostics. Validate type-check configuration only with the approved toolchain; do not install an additional checker silently.

## Task 3 Evidence and handoff

**Files:** `site/README.md`, dependency decision record and PR #507 description.

- [x] Document exact commands, exit statuses, resolution/build failures and any remaining integration gap.
- [x] Keep the design preview and exact-frame approval visibly pending. Do not claim site or browser acceptance from the probe.
- [x] Run affected JS style and documentation checks, inspect the staged delta, then commit and push only this feature branch.
- [x] Keep PR #507 in draft. Its one final independent full-delta review waits for the complete implementation, not this dependency checkpoint.

## Execution record

The first install failed at the default read-only cache (exit 1); the approved local-cache retry succeeded. All approved pins remained unchanged, and npm reported a valid full dependency graph. The entry-point test was observed failing for missing static output, then for the missing static search file, before each fixture was implemented. The final probe and TypeScript checks pass. Two upstream CSS warnings remain visible. The [validation record](../../../site/README.md) records fixture corrections and the limits of this proof.

Design review, full site implementation and the final independent PR review are outside this approved build-validation checkpoint and remain pending.

Repository-wide `make lint-js` exits 2 on unchanged OpenCode-plugin formatting (one error), with 46 existing warnings and two informational diagnostics. All six new JS/TS files pass their focused Biome check. This baseline failure is reported, not fixed or suppressed here.
