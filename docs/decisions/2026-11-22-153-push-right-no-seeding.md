# The daemon pushes only to a host whose grant carries the owner's push right, and the upgrade grants none

Status: accepted

Issue #153, under #857's decision 3 (one record per project and host, with the rights reach,
credential and push). The no-seeding rule was decided by Theo on 2026-11-22: option B of the two the implementing
session offered.

- **The push right lives on the project's grant of the host** (`sandbox_network_grants`,
  migration 9: `push_granted_at`, `push_granted_through`). A request's "allow always" never sets
  it. Only the owner's `torlek network grant PROJECT HOST --push` sets it. It is refused for a
  project that holds no grant of that host, so push never stands without reach. Revoking the
  grant ends the push right too.
- **The upgrade seeds nothing.** No existing grant gets the right, github.com's included, so
  `torlek checkout push` is refused until the owner binds a host per project. The `push`
  readiness line names the one command that binds it (`needs-authorization`, not blocking).
  Before this change, any project with a GitHub origin pushed.
- **A push goes only to an `https` origin on a host a forge adapter declares for Git pushes**
  and the project's grant carries the right on. An SSH origin is refused naming SSH (#857's
  decision 4), and so is an origin that names a credential. The host list comes from the
  adapters' declarations (`cockpit-daemon` `forges.rs`: GitHub alone), never from a GitHub
  string in `cockpit-projects`.
- **Rejected:** a migration granting push to every project whose origin is on GitHub (option A),
  so pushes would keep working with no step from the owner. #857 says push comes only after the
  owner binds it. Also rejected: a `--push` that grants reach when it is missing, because binding
  push would then silently widen what the project's agents may reach (#857: binding push to a
  host without reach is refused).

Evidence:

- `cockpit-sandbox` `network`:
  - `grants_made_before_the_push_right_carry_none`;
  - `a_push_right_on_a_host_the_project_does_not_reach_is_refused`;
  - `revoking_the_hosts_grant_ends_its_push_right`.
- `cockpit-projects` `push`:
  - `a_host_without_the_push_right_is_refused_naming_the_command_that_binds_it`;
  - `an_ssh_origin_is_refused_naming_ssh_even_on_a_host_the_project_may_push_to`.
- `cockpit-readiness` `report`: `a_project_reaching_its_forge_without_the_push_right_names_the_command_that_binds_it`.
