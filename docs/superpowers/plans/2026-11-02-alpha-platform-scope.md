# Alpha platform scope implementation plan

> **For agentic workers:** Use executing-plans inline for this documentation-only owner task.

**Goal:** Align current Alpha 1 documentation with Theo's Linux/WSL2-only decision.

**Architecture:** README remains the source of the generated public install page. Current acceptance requirements point to a dated decision; historical results retain their original scope.

**Tech Stack:** Markdown, existing Python acceptance fixtures and Node content-staging tests.

**Spec:** [Owner decision and restoration criteria, #568](https://github.com/Androlax2/agent-cockpit/issues/568).

## Global constraints

- macOS delivery, Homebrew distribution and macOS acceptance are deferred beyond Alpha 1, not passed; no restoration milestone assigned.
- Preserve Linux/WSL2, provider, context, two-user isolation and human acceptance requirements.
- Keep historical evidence and the completed #530 checkout; no install, live probe, publication or legacy apply.
- Release and sandbox owners retain technical workflows, code and tests.
- Alpha 1 uses archive installation; #562/#563/#565 move to Alpha 2, #99 to Beta 3 design and #397's further CI efficiency to Hardening.

## Review focus

Check current promises separately from historical evidence, the four-cell onboarding matrix, generated install content, retained non-platform criteria and absence of an implied macOS restoration date.

## Task 1: Reconcile documentation

- [x] Add `docs/decisions/2026-11-02-568-alpha-platform-scope.md` using the existing decision convention.
- [x] Update `README.md` and `docs/RELEASING.md` current platform promises; site content remains generated from README.
- [x] Update `docs/acceptance/m3.md`, `docs/gates/m3-acceptance.md`, `docs/acceptance/98-user-readme.md` and `docs/acceptance/102-documentation-site.md` with explicit deferred scope.
- [x] Add targeted scope annotations to `docs/design/greenfield/16-feature-coverage-map.md`; retain historical tests and design details.

## Task 2: Verify and deliver

- [ ] Run `python3 -m unittest discover -s tools/tests -p '*acceptance*.py'`, `node --test site/tests/content.test.mjs`, explicit Markdown lint, `make docs-ci` and `git diff --check`. Report missing prerequisites rather than installing them.
- [ ] Inspect actual generated content through the existing staging fixture; rely on ready-PR CI for the rendered site build when local dependencies are unavailable.
- [ ] Run the installed OpenCode `code-review medium` command's eight inline angles and deduplication; obtain the separately required independent full-diff review.
- [ ] Commit and push this branch, open a separate PR, mark it ready, inspect exact-head checks and use guarded squash merge only after required checks/reviews pass.
- [ ] Hand off exact evidence and unmeasured acceptance; keep #568, #137 and #52 open.
