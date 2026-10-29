# Contributing

Start with [Development](docs/DEVELOPMENT.md) for prerequisites and verification, and
[Architecture](docs/ARCHITECTURE.md) for code ownership. Coding agents also read
[AGENTS.md](AGENTS.md) and the repository's [authorization rules](CLAUDE.md).

- Keep changes scoped to the requested task. Separate mechanical formatting, lint fixes
  and structural moves so each diff can be reviewed independently.
- Preserve behavior and compatibility in refactors, especially command paths, request
  validation, persistent state and host approval boundaries.
- Exercise real public entry points when wiring matters. Use existing isolated fixtures
  and fake host commands; never run destructive tests against live user state.
- Report the commands run and their actual outcomes, including pre-existing failures,
  skipped checks and missing tools. Do not disable tests to make a check pass.
- Update the owning documentation with behavior or workflow changes. Proposals belong
  in clearly labeled plans, not in instructions presented as current functionality.

The project uses feature branches and reviewed host application. Permission to edit is
not permission to install tools, change dependencies, push workflows or run host commands;
follow the current user's explicit authorization and `CLAUDE.md`.
