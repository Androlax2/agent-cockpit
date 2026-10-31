# Contributing

Cockpit is being rebuilt from scratch under `crates/` (the greenfield refactor) while the
current programs under `apps/` keep running until the cutover. New work goes into the
greenfield workspace. The working agreement is
[issue #17](https://github.com/Androlax2/agent-cockpit/issues/17); this file is its
contributor-facing summary.

Coding agents also read [AGENTS.md](AGENTS.md) and the repository's
[authorization rules](CLAUDE.md). [Architecture](docs/ARCHITECTURE.md) maps the code that
exists today under `apps/`. Everyone follows the
[Code of Conduct](CODE_OF_CONDUCT.md). Security problems go through
[SECURITY.md](SECURITY.md), never a public issue.

## Before you start

1. Read [the design reports' README](docs/design/greenfield/README.md): its decision
   register and status legend (`decided`, `proposed`, `validate`).
2. Pick an open issue labelled `greenfield` whose **Depends on** issues are closed.
   Milestones run in order: the next one starts only when the previous milestone's `gate`
   issue is closed with its proof linked.
3. Read the reports named on the issue's **Read** line, and nothing more.
4. Read [Development](docs/DEVELOPMENT.md) for prerequisites and the check commands.

Follow the reports according to that status legend. When a change needs a decision the
reports do not make, open an issue labelled `decision-needed` and stop on that point
instead of guessing.

## One issue, one pull request

- Branch `greenfield/<issue-number>-<slug>` from `main`, one branch per issue.
  Dependent issues may stack on the previous issue's branch.
- Never touch `apps/` except for a stabilization fix in its own pull request, which keeps
  the [compatibility boundaries](AGENTS.md#compatibility-boundaries).
- Keep mechanical formatting, lint fixes and structural moves in separate commits from
  behavior changes, so each diff can be reviewed on its own.
- Build in the [order the refactoring plan sets](docs/design/greenfield/20-refactoring-plan.md#order-inside-a-milestone);
  tests drive the CLI and the protocol, not internals. Add restart, stale-result and
  duplicate-delivery fixtures where the issue lists them. Use isolated fixtures and fake
  host commands; never run tests against live user state.
- When the pull request proves a row of the
  [feature coverage map](docs/design/greenfield/16-feature-coverage-map.md), update the row
  with the test name. Record non-obvious decisions as one file each under [docs/decisions/](docs/decisions/README.md).
- Open the pull request from the template. Reference the issue (`Closes #<n>`), list the
  exact commands you ran with their outcomes, and keep `validate` claims open until
  measured. A skipped check is not a pass; never disable a test to make a check pass.
- Title the pull request as a conventional commit subject, `type(scope): description`: it
  becomes a `CHANGELOG.md` entry ([Releasing](docs/RELEASING.md#changelog)). Say what
  changed, not how; the issue goes in the body.
- Never force-push. Rebase onto `main` before the pull request is opened; once it is open,
  merge `main` into the branch (or into a stacked branch) instead of rewriting history.

## Checks

Run focused tests while you work, then the gate for the paths you changed:
[Development](docs/DEVELOPMENT.md#verification-policy) lists which `make` target covers
which paths. The pull request's CI is the full gate; a pull request is merged only when
every check is green.

## Labels

| Label | Meaning |
| --- | --- |
| `greenfield` | Part of the refactor under `crates/` |
| `area:<name>` | The capability an issue belongs to (`build`, `daemon`, `protocol`, `security`, `state`, `sessions`, `tasks`, `teams-chief`, `providers`, `usage`, `clients`, `plugins`, `release`, `ux`) |
| `gate` | Milestone gate: all listed proof must pass before the next milestone starts |
| `validate` | A claim that needs a test or measurement before it counts |
| `decision-needed` | Blocked on a decision by the maintainer |
| `bug`, `enhancement`, `documentation`, `accessibility`, `question` | The kind of change or report |
| `good first issue`, `help wanted` | A good entry point, or extra attention needed |
| `duplicate`, `invalid`, `wontfix` | Closed without a change |

## Milestones

M0 Foundation, M1 Single-session parity, M2 Tasks and scheduling, M3 Providers and
onboarding, M4 Teams and Chief, M5 Full parity and cutover, M6 Designed UI, M7 Plugins.
What each one proves, and its gate, live on GitHub and in the
[refactoring plan](docs/design/greenfield/20-refactoring-plan.md#milestones-and-gates).

## Stale branches

- A branch whose pull request is merged or closed is deleted by the maintainer.
  Contributors and agents never delete branches.
- A pull request with no activity for 30 days gets a comment asking whether to rebase or
  close it; after 14 more days without an answer it may be closed. Its branch stays
  until the maintainer deletes it, so the work can be reopened.
- A branch without a pull request is not reviewed. Open a draft pull request to keep
  work visible.

## Permissions

Permission to edit is not permission to install tools, change dependencies, push workflows
or run host commands. Those need the maintainer's explicit authorization; agents follow
[CLAUDE.md](CLAUDE.md).

## License

Contributions are licensed under the [Apache License 2.0](LICENSE), the project's license,
as section 5 of that license describes.
