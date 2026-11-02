# Generated changelog regression plan

> **For agentic workers:** Use executing-plans inline and obtain one fresh independent full-delta review.

**Goal:** Fix #192 draft #514's generated Markdown failures at their source.

**Architecture:** Keep git-cliff as the generator; correct its header separation and render angle-bracket subject text literally. Exercise the real pinned generator with a historical-style fixture subject and check the full generated history with the unchanged docs gate.

**Tech stack:** Existing git-cliff 2.14.2, Python fixtures and docs-ci tools.

**Spec:** Chief's #192 follow-up: MD022 at line 6 and MD033 at line 548 of draft #514, run 41381354756.

## Constraints and review focus

- No edits to the draft branch, rewritten history, new dependencies or relaxed lint rules.
- Preserve previous changelog sections and deterministic proposal retries.
- Check real generated output; a source-string assertion is insufficient.
- Keep #192 open until replacement draft CI succeeds. #191 follows this correction.

## Task 1: Reproduce and fix generation

- [x] Extend real-tool conformance with a non-conventional subject containing an angle-bracket placeholder and an inline command.
- [x] Observe both missing separator and raw HTML failures before changing cliff.toml.
- [x] Correct the template and rerun conformance plus the 17 orchestration tests.

## Task 2: Validate and deliver

- [x] Generate CHANGELOG.md from actual repository history in a disposable checkout and run make docs-ci there.
- [ ] Update #192 evidence, run affected formatting/lints, and obtain fresh independent full-delta review.
- [ ] Open a separate feature PR and retain draft #514 until the reviewed main workflow verifies its replacement. Record same-base retry evidence and the replacement's CI result.
