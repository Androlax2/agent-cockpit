# Agent Cockpit engineering guide

Cockpit coordinates agent sessions through a terminal UI, an OSD daemon, command-line
clients and a phone interface. The host owns approval and session identity; agent
requests do not grant authority.

## Before editing

Read [CLAUDE.md](CLAUDE.md) for this repository's Git authorization, application workflow
and test entry points. Those permissions are not broadened by this guide or its skills.
Read the [architecture map](docs/ARCHITECTURE.md) and the relevant scoped rule:

| Area | Guidance |
| --- | --- |
| Rust and Cargo | [Rust rules](.agents/rules/rust.md) |
| Shell, Python, JavaScript/TypeScript, Lua | [Script rules](.agents/rules/scripts.md) |
| Requests, session identity, persisted state | [Protocol rules](.agents/rules/protocol.md) |
| Makefile, tests and CI | [Verification rules](.agents/rules/ci.md) |

This index is explicit: clients need not interpret another client's path-scoped rule
metadata. Project guidance supplements the user's global instructions.

## Greenfield refactor

Issues labelled `greenfield` (milestones M0–M7) build the target architecture under
`crates/` following the [design reports](docs/design/greenfield/README.md) and the working
agreement in [issue #17](https://github.com/Androlax2/agent-cockpit/issues/17). Load the
reports' README plus the reports named in your issue, nothing more. The architecture map
describes the code that exists today under `apps/`, which keeps running until the cutover.

## Working loop

1. Identify the owning module, its callers and the real entry-point test that proves
   the change. Read surrounding code before choosing an abstraction.
2. Run the focused test while iterating. Preserve validation and error handling during
   moves; formatting and architectural changes belong in separate reviewable diffs.
3. Run the affected checks on the final source. Use the shared full-test entry point
   when full verification is needed; do not launch duplicate broad runs across clones.
4. Report actual commands and outcomes, including existing failures or missing tools.
   A skipped check is not a pass.

Build and verification instructions live in [Development](docs/DEVELOPMENT.md).
Contribution expectations live in [CONTRIBUTING.md](CONTRIBUTING.md).

## Project workflow skills

| Task | Skill |
| --- | --- |
| Choose checks and verify a change | [verify-cockpit](.agents/skills/verify-cockpit/SKILL.md) |
| Apply completed work through host review | [apply-cockpit](.agents/skills/apply-cockpit/SKILL.md) |

If the client does not discover `.agents/skills/`, read the linked skill explicitly.
These workflows refer to canonical policy; they do not grant additional permissions.

## Compatibility boundaries

- Keep installed command names and `apps/target/release` paths stable.
- Preserve wire fields, validation, state paths and database transaction ownership.
- Keep host-side approval and authenticated sender checks at their existing owners.
- Use isolated fixtures and fake host commands in tests; never exercise a user's live
  services, desktop, credentials or session state as a substitute for a fixture.
- New dependencies, tool installs, host actions and Git operations require the
  authorization specified by the user and [CLAUDE.md](CLAUDE.md).

Record non-obvious architectural decisions in [Decisions](docs/DECISIONS.md). Document
the code that exists; mark proposals explicitly until their implementation is verified.
