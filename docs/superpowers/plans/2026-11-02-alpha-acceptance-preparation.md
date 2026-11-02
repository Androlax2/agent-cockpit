# Alpha acceptance preparation implementation plan

> **For agentic workers:** Use the executing-plans workflow inline; the overnight assignment prohibits additional implementation workers.

**Goal:** Prepare #137 without claiming Theo's acceptance, and inventory #52 evidence.

**Architecture:** Keep the historical M1 runner for its recorded walk. Add an offline rehearsal that invokes only the selected candidate's CLI in a new scratch home, with an allowlisted environment. The Alpha guide records live prerequisites and observations separately.

**Tech Stack:** Python standard library, unittest subprocess fixtures, ask-to-guide Markdown.

**Spec:** Issues #137 and #52, their named design reports, and the overnight lane assignment.

## Global constraints

- No live desktop, credentials, services, provider probes, installs, publishing or guides overnight.
- Profile `next`; scratch HOME, XDG directories, tmux and temporary files.
- No manifests, workflows, release-owner or README-owner file changes.
- One issue per PR; #52 receives an evidence comment, not a second implementation.
- Independent review is separate from the unavailable named code-review medium gate.

## Review focus

- Inherited credentials, desktop bus, agent socket and profile must not reach the candidate.
- Missing executable and unexpected exit must fail, rather than appear as acceptance.
- Existing homes must never be reused or deleted.
- Paths containing spaces must retain argument boundaries.
- Offline doctor refusal must remain observable; it is not host readiness.

## Task 1: Offline rehearsal and guide

- [x] Add `tools/tests/test_alpha_acceptance.py`: subprocess fixtures for environment isolation, private fresh homes, exact candidate invocation, refusal and exit preservation. Run it red.
- [x] Add `tools/acceptance/alpha1.py`: `--bin-dir DIR --parent DIR`; create a private unique directory, run version/config/init/doctor with explicit `--profile next`, save raw bounded-time command results and candidate SHA-256. Never start a daemon or install anything.
- [x] Run `python3 -m unittest discover -s tools/tests -p 'test_alpha_acceptance.py'` green.
- [x] Add `docs/acceptance/m3.md` in guide format and `docs/gates/m3-acceptance.md` with preparation status, dependencies, source evidence and an empty live-run record.
- [x] Run focused lint, guide-format checks and existing acceptance-runner regressions.
- [ ] Obtain a bounded independent full-delta review, address findings, commit and open a preparation PR. Do not arm merge without the named review gate.

## Task 2: Gate evidence handoff

- [x] Inventory source, merged commits, CI/native receipts and remaining acceptance gaps on #52.
- [x] Cross-link #97 release/docs coordination and prepare one morning readiness question with an honest time estimate.
