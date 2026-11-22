# The daemon reads a project's forge with its API token, for reads only

Status: accepted

Issue #60, amending #857's token split. Decided by Theo on 2026-11-22: option A of the two the
implementing session offered.

- **The API token is no longer the proxy's alone.** #857 had the API token
  (`github-api-tokens/<project>`) added only by the session proxy. The daemon now reads with it
  too: it lists the project's open issues beside its todos (`torlek todo issues PROJECT`, the
  owner's verb alone) and, from #60's next part, reads pull request state as task evidence. It
  never writes with it.
- **No API token, no read.** A project without one has its issues refused, and the refusal
  names `torlek project token set PROJECT --api`. The daemon never falls back to the project's
  other token or to the owner's `gh` sign-in: the adapter's `gh` keeps its configuration in the
  state directory's `forges/github`.
- **Rejected:** a third, read-only token per project (option B). It would add a token the owner
  creates and rotates for each project, for permissions the API token already holds.

Evidence:

- `cockpit-daemon` `issues`:
  - `issues_are_read_with_the_projects_api_token_and_never_its_other_token`;
  - `a_project_without_an_api_token_has_no_issues_read`.
- `cockpit-daemon` `sandbox`: `TaskIssues` is refused on the agent socket.
