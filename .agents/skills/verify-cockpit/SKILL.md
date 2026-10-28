---
name: verify-cockpit
description: Select and run evidence-based verification for Agent Cockpit changes. Use when changing its Rust daemon or clients, shell/Python launchers, phone assets, shared protocols, or when deciding which checks establish that a Cockpit task is finished.
---

# Verify a Cockpit change

Read the repository's `AGENTS.md`, `CLAUDE.md`, `.agents/rules/ci.md` and
`docs/DEVELOPMENT.md`. They own permissions and executable verification policy;
this workflow does not grant build, install, host or Git authorization.

1. Inspect the final diff and its callers. Name the observable failure the change
   prevents and find the real entry-point test exercising it.
2. Select the narrowest useful proof during iteration:
   - Python tool: its matching file under `tools/tests/`.
   - Launcher or CLI: its integration test under `apps/cockpit/tests/`, using the
     existing scratch-home/fake-command fixtures.
   - Registry or collaboration state: the relevant daemon tests and affected
     client/protocol tests, including invalid and stale inputs.
   - Phone assets: the existing Node tests plus browser checks when interaction or
     layout needs verification. A Node-only result does not prove browser behavior.
3. Run only authorized commands. Check the current Makefile for available targets;
   proposed targets in old plans are not executable documentation. Do not install
   missing tools automatically, bypass a sandbox block or alter test assertions to
   conceal a baseline failure.
4. Once the source settles, run affected formatting/lint checks and consumer checks.
   For a full run, use `make test` so its resource controls, shared lock and OSD build
   prerequisite remain in effect. Avoid repeating unchanged broad checks.
5. Report the exact command, exit status and result. Distinguish passed, failed,
   not run and missing prerequisite. Record pre-existing failures separately from
   regressions, with evidence rather than assumptions.

For crate moves, additionally check installed executable paths, package-local binary
test discovery, repository-relative fixtures, dependency direction and wire compatibility.
Keep test isolation explicit: a fake host launch must not inherit sandbox markers that
select a different execution path.

End with the verification evidence and any unresolved gap. Passing tests are not
authorization to commit, push, install or apply changes.
