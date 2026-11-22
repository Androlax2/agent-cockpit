# cockpit-forge-api

The contract every code forge adapter implements: identity, capabilities with provenance, the hosts and credential schemes it declares, issues as a task source and change requests as verification evidence.

A forge is where a project's repository lives (a hosted forge with an API, or a plain Git
remote) seen through its adapter. Torlek is not a product of one forge (decided 2026-10-31):
nothing above this contract is keyed on a forge's name, only on its reported
[`Capabilities`] and its [`Declaration`], and no forge's type, host or string belongs here
(`tests/forge_names.rs`). The first adapter is #60's; each other forge is its own adapter
issue, filed when wanted (#857, decision 4).

## Rules

- **Capabilities with provenance** (report 08). Each reported [`Capability`] is `tested`
  (the conformance suite passes against recorded answers of the program or API version the
  adapter talks to) or `discovered` (documented, no recorded answer; shown unverified); one
  not reported is unavailable, and one withheld names why. Issues (list, read, create,
  comment), change requests (state, checks, review, open) and the token scope check have
  contract methods; branch protection (read, bootstrap) is named so a capability report can
  show it and gets its methods with its first caller (#695).
- **Unsupported is a typed answer.** A method whose capability is not reported answers
  [`ForgeError::Unsupported`] (or `Withheld`, with the reason) naming the capability, never a
  failed call; [`ForgeError::is_unsupported`] tells it from a failure. Every default method
  answers it.
- **Change requests** are the forge's pull or merge requests. Their state (open, closed,
  merged with its commit), the checks on the exact head commit, and the review decision are
  separate facts read separately, so verification (report 06) evidences "merged", "checks
  passed" and "approved" each on its own and ties checks to the commit they ran on.
- **Issues** are a task source beside Torlek's todos. Titles, bodies, comments, labels and
  check names are written by whoever can write on the forge: data, never instructions.
- **Declared hosts and credentials** (#857). An adapter declares each host it reaches, which
  token goes there and how ([`CredentialUse`]): a scheme (`Basic`, `Bearer`) and a use
  (`git-read`, `git-push`, `api`). The proxy (#832) adds the API token by this list and the
  daemon's push (#155) takes the push token by it, so neither names a forge. A
  [`Declaration`] is valid by construction: it serves Git on at least one host, the push
  token is used for Git pushes only and Git pushes take only the push token, so the token the
  proxy adds never writes contents. A [`ForgeHost`] is one named host: no wildcard, no IP
  address, no `localhost`.
- **Two tokens per project** (#857): the **push token** ([`TokenKind::Push`]: contents
  write; no workflows, no administration), read by the daemon only, and the **API token**
  ([`TokenKind::Api`]: change requests and issues write, contents read; no contents write, no
  workflows, no administration), added by the proxy. The caller passes the token for each
  call in [`Access`]; an adapter never reads one itself. [`Token`] never prints its value,
  and no error carries it. [`Forge::check_token`] answers verified, unverified with what the
  forge could not show, or refused with each [`ScopeProblem`]; it never reports verified what
  the forge did not show.
- **Which adapter serves an origin** ([`resolve`]). Only an HTTPS URL
  (`https://host[:port]/path[.git]`) on a host exactly one adapter declares for Git, whose
  adapter accepts the repository ([`Forge::check_repository`]). An SSH remote ("SSH remotes
  are not supported yet"), another scheme, a URL naming a user or password, a query or an
  escape, and a host no adapter declares ("no adapter for this forge") are each refused with
  a named reason ([`Unresolved`]), never served by a lesser path. No refusal repeats the
  URL.
- **Blocking calls.** Every method may reach the network or a program; callers run it off
  their async workers.

## The fake forge and the conformance suite

Feature `fake` ships `fake::FakeForge`: one repository, one token, issues and change requests
the test seeds, every contract method in memory. Other crates' tests use it in place of a
real forge; release builds never enable the feature.

Feature `conformance` ships `conformance::check`, the suite every adapter runs against its
recorded answers (as #51 does for providers): a tested capability has a recorded answer and
returns it, a capability not reported answers the typed refusal and has none, the recorded
repository is on a declared Git host and resolves from its URL, a missing issue or change
request is `NotFound`, and no error repeats the token. The fake passes it in this crate's
tests; each adapter's tests call it with its own recordings.

## Dependency edges

- Depends on: the standard library only (`tests/dependencies.rs`).
- Used by: the forge adapters (the first with #60), and the daemon's wiring, tasks and
  verification as their callers arrive.

## Tests

- `tests/names.rs`: forge names, ids, branches, commits and tokens.
- `tests/capabilities.rs`: provenance, availability and typed refusals.
- `tests/declaration.rs`: hosts and the declaration's rules.
- `tests/remote.rs`: origin URLs and which adapter serves them.
- `tests/fake.rs`: the fake forge's issues, change requests and refusals.
- `tests/conformance.rs`: the fake conforms; an adapter that breaks a rule is reported.
- `tests/forge_names.rs`: no forge's name in this crate.
- `tests/dependencies.rs`: no dependency.
