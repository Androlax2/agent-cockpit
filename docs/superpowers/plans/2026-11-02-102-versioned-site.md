# Versioned documentation site implementation plan

> **For agentic workers:** Execute inline with test-driven development and verification-before-completion. One fresh independent full-delta review follows implementation.

**Goal:** Implement #102's static Astro/Fumadocs documentation from canonical source with generated references, local search and versioned output.

**Architecture:** Stage canonical Markdown and isolated CLI output into ignored site content, render with the approved upstream Fumadocs components, and assemble latest/released-version trees. Build and deploy jobs remain separate; host review owns workflow pushes and Pages changes.

**Tech Stack:** The exact approved site package pins and lockfile; Node built-ins for staging/checking, Astro 7, Fumadocs UI/React, existing Python Playwright for browser verification. No new dependencies.

**Spec:** [Approved r4 handoff](../../design/102-documentation-layout-r4.md), [generator decision](../../decisions/2026-11-02-102-documentation-generator.md), issue #102.

## Constraints and review focus

- Theo approved visual revision `1a19a1c1456` and the exact dependency graph. Record approval and handoff status without overwriting the open r4 document.
- Preserve canonical content, source links and source revision. Escape Markdown-only syntax safely for MDX; code fences and inline code remain literal.
- Generated CLI/configuration references come from the actual binary in a temporary empty home with a cleared environment. Never read live services, state or credentials. Effective defaults are not a schema; #295 stays planned.
- Every internal document/asset/search URL must survive the repository subpath and version prefix. Reject traversal-like version/base inputs.
- Search must support results, no matches and index failure, keyboard dismissal and pointer/touch Close, restoring focus.
- Preserve approved Fumadocs neutral light/dark identity, readable prose, full quickstart and platform/provider limitations. No new name or branded landing page.
- Rebuild the published version set deterministically; a newer latest build must not remove released-version copies. No tags/releases are created.
- Do not modify tools/release.py, release/prerelease workflows or docs/RELEASING.md. A separate docs workflow may listen for completed releases; its push and host Pages actions require final ask-to-run review.

## Task 1 Approval and canonical content

**Files:** approval/handoff records; `site/scripts/content.mjs`, `site/tests/content.test.mjs`, `site/content/` sources for comparison/planned references.

- [x] Record approval; mark only the ten approved frames ready in a separate handoff archive.
- [x] Write failing tests for code-preserving Markdown conversion, canonical link mapping under a version prefix, invalid paths and isolated binary reference generation.
- [x] Implement staging using Node built-ins. Stage README sections, shipped instruction files, design reports and decisions; derive reference content from the binary and identify sources/revision.
- [x] Run focused content tests against temporary fixtures/fake binaries, then stage the real binary's help/defaults offline.

## Task 2 Static UI and real build

**Files:** `site/astro.config.mjs`, `site/src/`, `site/tsconfig.json`, `site/tests/site.test.mjs`, manifest scripts, Makefile site targets.

- [x] Add an entry-point test asserting the built prefix, real canonical content, generated provenance, search index, metadata and valid local links; observe failure before implementation.
- [x] Implement the approved layout with upstream components, version navigation, local search, source edit links and neutral themes. Use a plain planned-schema page until #295 exists.
- [x] Build and check real output, including a release-style prefix. Run fixture and site TypeScript checks with the approved compiler.
- [x] Test light/dark, mobile 320px, long code/tables, keyboard search/Close/focus restoration, no-result and failed-index states with the existing isolated browser tools. Stop temporary servers afterward.

## Task 3 Version assembly and CI proposal

**Files:** `site/scripts/versions.mjs`, matching tests, separate `.github/workflows/docs.yml`, site documentation.

- [x] Write failing fixture tests for assembling latest plus released versions without dropping older trees or accepting unsafe paths.
- [x] Implement deterministic version-tree assembly and rendered local-link checking. Preserve exact per-version source/build provenance.
- [x] Prepare PR build-only and main/released-version build/deploy workflow, using read-only build permissions and a separate deployment job. Validate local workflow/script checks. Do not push workflow changes until host review.
- [x] Document boundaries and coordinate via the PR: release files remain owned by the release lane; docs workflow consumes published-release evidence.

## Task 4 Verification and reviewed delivery

- [ ] Run affected tests, site build/type checks, source/rendered link checks and docs quality gate. Report the existing global JS formatting failure separately; do not hide warnings.
- [x] Run installed Impeccable against actual implemented web source/rendered evidence and record exact coverage. Preserve the earlier FigCraft read failure as historical evidence.
- [x] Update coverage evidence and PR acceptance. Keep volunteer/public-install validation open until measured.
- [ ] Obtain one fresh independent full-delta review. Fix significant findings and verify them.
- [ ] Prepare final host-reviewed workflow push and Pages settings/deployment commands. No public deployment, setting or release without that approval.

## Current review checkpoint

The requested Codex `$code-review` medium workflow ran inline and found six items;
fixes and exact scope are in the [review record](../../acceptance/102-code-review.md).
Final documentation checks, one fresh independent full-delta review and host-reviewed
workflow publication remain. Pages and volunteer acceptance are not complete.
