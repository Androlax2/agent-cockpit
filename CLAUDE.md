# Agent Cockpit

## Git authorization

Claude may create feature branches, stage and commit completed, requested agent-cockpit
changes without asking again in chat. Never commit directly to the default branch.
Pushing and opening pull requests still require an explicit request.

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
PATH (apps/cockpit/tests), the daemon's registry (apps/osd), and the Python tools (tools/tests).

## Todo trailers

End a commit message with one `Todo: <id>` line per todo item the commit fixes (8 hex
characters, as `claude-todo list` shows). `cockpit apply` marks those items done once Theo
approved the commits.
