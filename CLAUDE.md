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

## Applying completed work

`~/agent-cockpit` is the running copy the host runs with Theo's rights; no session works in it.
Sessions work in `~/IdeaProjects/agent-cockpit` or its parallel clones. Finishing requested
changes includes applying them, without asking again in chat: export the commits with
`git format-patch origin/main -o .outgoing`, then use `ask-to-run` to run
`cockpit apply <clone-root>`. Keep the host-side review and approval; never bypass it. The
apply runs `make after-apply` (relink, rebuild, `cockpit reload`); then run
`make -C ~/agent-cockpit test` through `ask-to-run`.

## Tests

`make test` from the repository root: the scripts, hooks and programs through fake commands on
PATH (apps/cockpit/tests), the daemon's registry (apps/crates/cockpit-daemon), and the Python tools (tools/tests).
The greenfield workspace in crates/ has its own gate, `make greenfield-ci` (tests: `make crates-test`).
The Rust tests run under `cargo nextest` (Arch package `cargo-nextest`), one process per test, all
in parallel; a single file still runs with `cargo nextest run --release --manifest-path apps/Cargo.toml --test <name>`,
or `cargo nextest run --manifest-path Cargo.toml --test <name>` for a greenfield crate.

## Todo trailers

End a commit message with one `Todo: <id>` line per todo item the commit fixes (8 hex
characters, as `claude-todo list` shows). `cockpit apply` marks those items done once Theo
approved the commits.
