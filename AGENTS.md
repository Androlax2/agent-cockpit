# Agent Cockpit engineering guide

Cockpit coordinates agent sessions through a terminal UI, an OSD daemon, command-line
clients and a phone interface. The host owns approval and session identity; agent
requests do not grant authority.

## Before editing

Read [CLAUDE.md](CLAUDE.md) for this repository's Git authorization.
Those permissions are not broadened by this guide or its skills.
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

Issues labelled `greenfield` build the target architecture under `crates/` following the
[design reports](docs/design/greenfield/README.md) and the working agreement in
[issue #17](https://github.com/Androlax2/agent-cockpit/issues/17). Milestones are releases
(Alpha 1, Alpha 2, Beta 1, Beta 2: cutover, Beta 3: designed UI; `Maintenance: hardening`
is the intake for follow-ups no release needs); [CONTRIBUTING](CONTRIBUTING.md#milestones) says what a release milestone
means. Load the
reports' README plus the reports named in your issue, nothing more. The architecture map
describes the code that exists today under `apps/`, which keeps running until the cutover.

## Working loop

1. Identify the owning module, its callers and the real entry-point test that proves
   the change. Read surrounding code before choosing an abstraction.
2. Run the focused test while iterating. Preserve validation and error handling during
   moves; formatting and architectural changes belong in separate reviewable diffs.
3. Run the affected checks on the final source with the cargo commands in Tests below;
   do not launch duplicate broad runs across clones. The `make` targets are not
   for agents.
4. Report actual commands and outcomes, including existing failures or missing tools.
   A skipped check is not a pass.

Build and verification instructions live in [Development](docs/DEVELOPMENT.md); its
`make` commands are for humans and `apps/`, not for agents (see Tests below).
Contribution expectations live in [CONTRIBUTING.md](CONTRIBUTING.md).

## Tests

Never use the old cockpit's tooling until the cutover (#66) removes it: no `make test`,
`make crates-test`, `make greenfield-ci` or other Make target that runs through `agent-build`
(game guard, admission gate), and no `cockpit-*` host commands. `apps/` is the old cockpit;
leave it alone unless your issue says otherwise. Run the checks with cargo
directly from the repository root, with `CARGO_BUILD_JOBS=2` because parallel clones share the host:

- while iterating: `cargo nextest run --locked -p <crate>` (add `--test <name>` for one file)
- before pushing: `cargo nextest run --locked --workspace`, so crates that depend on yours build too
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items`

CI's `greenfield` job runs the full gate (deny, typos, sandbox smoke too) once the pull request
is ready for review.

## Project workflow skills

| Task | Skill |
| --- | --- |
| Coordinate and deliver repository work | [repo-workflow](.agents/skills/repo-workflow/SKILL.md) |
| Coordinate Torlek workers: start, watch, close out | [coordinator](.agents/skills/coordinator/SKILL.md) |

If the client does not discover `.agents/skills/`, read the linked skill explicitly.
These workflows refer to canonical policy; they do not grant additional permissions.

### Skill invocation by client

Project Markdown may name a skill workflow using Claude Code's `/name` syntax.
Use `/name` in Claude Code and `$name` in Codex: `/code-review` means
`$code-review` for a Codex worker. This translation applies to skill invocations,
not file paths, shell commands or client-specific built-in commands.

Codex workers must invoke `$code-review` for the required review and record what
actually ran. Check Codex's available skills before declaring it unavailable;
the absence of `/code-review` alone is not evidence. If the named skill is missing,
report that exact limitation rather than silently substituting another skill or
claiming an independent-agent fallback was the requested workflow. Existing
authorization, review-scope and task-specific restrictions still apply.

## Compatibility boundaries

- Keep installed command names and `apps/target/release` paths stable.
- Preserve wire fields, validation, state paths and database transaction ownership.
- Keep host-side approval and authenticated sender checks at their existing owners.
- Use isolated fixtures and fake host commands in tests; never exercise a user's live
  services, desktop, credentials or session state as a substitute for a fixture.
- New dependencies, tool installs, host actions and Git operations require the
  authorization specified by the user and [CLAUDE.md](CLAUDE.md).

Record non-obvious architectural decisions as one file each under [docs/decisions/](docs/decisions/README.md). Document
the code that exists; mark proposals explicitly until their implementation is verified.
