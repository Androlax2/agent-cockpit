# Agent Cockpit

Engineering guidance: [AGENTS.md](AGENTS.md). Read its scoped rules before editing.

## Git authorization

Claude may create feature branches, stage and commit completed, requested agent-cockpit
changes without asking again in chat. Never commit directly to the default branch.

With the per-project `GH_TOKEN` the launcher provides (decided 2026-10-31), Claude may also,
for the issue it was given: push its own `greenfield/<issue>-<slug>` branch to
`https://github.com/Androlax2/agent-cockpit.git` (`--force-with-lease` on that branch only,
after a rebase), open the pull request to `main`, and arm `gh pr merge <n> --auto --squash`
once one `/code-review` at `medium` has run on the branch delta (a second pass only when its
fixes are large) and the focused tests pass. `/simplify` does not run on pull requests; it runs
per crate before the first stable release (#152). Decided 2026-11-01. The `main` ruleset
(pull request + green CI, no bypass) is the merge gate. Never push to `main` or to another
branch, never force-push without lease, never merge by hand, never delete branches or tags.
Theo's local running copy is applied separately and is never a push target.

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
