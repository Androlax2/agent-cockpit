# gitleaks scans every agent push, is required, and a finding holds the push for the owner

Status: accepted

Issue #694, under #857 (the scan runs inside `torlek push`, over the range from the remote's
tips to the pushed commit, on real Git; not in the proxy or the sandbox). Theo chose the
scanner and what counts as a block on 2026-11-23: option B of the two the implementing session
offered.

- **gitleaks is a required host program.** The daemon runs `gitleaks` from its `PATH`. When it
  is missing, fails or runs past five minutes, an agent's push is refused: never sent
  unscanned. Readiness reports a missing gitleaks (#694, part 3).
- **A finding holds the push for the owner** (`Error::SecretFound`), as a CI change does. The
  refusal names the rule, file, line and commit, never the secret, and the owner's `torlek
  checkout push ID --branch B --commit C` sends it. The owner's own pushes are not scanned.
- **The repository cannot weaken the scan.** gitleaks reads the commits through a private bare
  repository that borrows the checkout's objects. Its rules are its built-in defaults, from a
  configuration file Torlek writes. A `.gitleaks.toml`, a `.gitleaksignore`, Git configuration
  and attributes from the checkout, and `gitleaks:allow` comments do not apply.
- **Rejected:** option A, a ruleset of about 20 token shapes built into `cockpit-projects`
  with the workspace's `regex`, needing no host program. The session recommended it. Also
  rejected: a push sent unscanned when gitleaks is missing.

Evidence: `cockpit-projects` `push`:

- `an_agents_push_adding_a_secret_waits_for_the_owner_naming_where_it_is`
- `the_scan_reads_no_configuration_or_ignore_file_the_checkout_holds`
- `without_gitleaks_an_agents_push_is_refused_and_nothing_is_pushed`
- `a_scan_that_fails_refuses_the_push_and_nothing_is_pushed`
