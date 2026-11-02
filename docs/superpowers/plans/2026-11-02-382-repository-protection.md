# Repository protection evidence implementation plan

> **For agentic workers:** Use the executing-plans workflow for inline implementation.

**Goal:** Make #382's observed repository protections, visibility gaps and proposed
host changes reproducible without exercising publishing credentials or changing settings.

**Architecture:** One repository-specific Python metadata collector invokes explicit
GitHub REST GETs through existing `gh`. A dated document interprets the results;
collection success is never an enforcement pass.

**Tech Stack:** Python 3.11 standard library, existing GitHub CLI, unittest fake commands.

**Spec:** [Issue #382](https://github.com/Androlax2/agent-cockpit/issues/382) and the
overnight assignment: documentation and justified verification scripts only; all
external changes remain proposed and unapplied.

## Global constraints

- No settings writes, workflow changes, secret values, App authentication, tags or releases.
- Use the existing isolated clone; retain unfamiliar launcher files.
- No dependencies, installs, manifest edits, load loops or more than two test threads.
- #191/#192 owns release workflows and App identity; #155 owns product Git safety;
  #376 owns plugin security. No crate edges or runtime behavior change here.

## Review focus

- HTTP 403/404 must stay unknown, never be called disabled or absent.
- Failed collection must not silently discard successful observations.
- Pagination must not hide additional rulesets or environments.
- Error output and unrelated variable values must not leak through a host report.
- App creation bypass must never also bypass tag update/deletion protection.

## Task 1: Read-only evidence and operator document

**Files:** Create `tools/repository_security.py`,
`tools/tests/test_repository_security.py`, `docs/security/repository.md`.

**Interface:** `python3 tools/repository_security.py` prints JSON observations and
unknowns for fixed repository `Androlax2/agent-cockpit`; exits 1 for incomplete
collection, 0 for complete collection, never for policy acceptance. No setup mode.

- [ ] Write CLI tests with a fake `gh` on PATH: GET-only repository scope,
  multiple pages, discovered ruleset detail, missing access, redacted errors,
  malformed JSON and absent executable.
- [ ] Run the focused unittest file and observe failure before implementation.
- [ ] Implement the collector using argument arrays, bounded subprocess calls,
  explicit metadata projections and pagination; retain partial evidence on errors.
- [ ] Run focused tests, Ruff and formatting; collect live read-only metadata.
- [ ] Write the dated source/API snapshot, per-setting verification commands,
  source-grounded CodeQL/Scorecard capabilities, token inventory and proposed setup.
- [ ] Run docs checks and independent full-delta review. Record the unavailable
  named `code-review medium` workflow separately; do not substitute or arm merge.
- [ ] Commit/push this issue branch and open a draft PR with exact evidence and
  `Part of #382`; hand off remaining host verification and enforcement acceptance.
