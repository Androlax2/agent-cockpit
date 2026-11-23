# cockpit-github

The GitHub adapter of the forge contract: issues, pull requests as evidence, repository token checks and the hosts and credential schemes GitHub takes, through the `gh` program.

It implements `cockpit-forge-api`'s `Forge` and nothing else, and depends on the contract
alone (`cockpit-workspace-rules`, "forge adapters -> forge-api only"). New GitHub types and
strings belong here (#60); the ones other crates carry from before the contract, such as the
push check's origin test, move behind it as their callers adopt the contract.

## Rules

- **The caller's token, never the owner's sign-in.** `gh` runs with an environment of its
  own: the token in `GH_TOKEN`, `GH_CONFIG_DIR` and `HOME` in the folder Torlek gives the
  adapter, and only `PATH`, the proxy variables and the certificate paths passed through
  from the daemon. It runs in that folder, so no checkout's remote is inferred. The token is
  never an argument and every error masks it.
- **Bodies on standard input.** Issue, comment and pull request bodies go to `gh` on
  standard input (`--body-file=-`), out of the argument list other processes read; every
  value is passed as `--flag=value`, so a title starting with `-` stays a title.
- **Declared hosts (#857).** The API token is added on `github.com` for Git reads (Basic)
  and on `api.github.com` (Bearer); the push token goes to `github.com` for Git pushes
  alone, from the daemon. Nothing outside this crate needs a GitHub host name.
- **Answers are read, never guessed.** A field missing, of another type, or a state GitHub
  does not document is `Malformed`; an answer about another issue or pull request than the
  one asked is too. `gh pr list --head` lists forks' branches of the same name, so a
  branch's pull request is the newest from the repository itself; a full page of forks' is
  an error, not "none". `gh issue` commands take a pull request's number, so a read checks
  the URL and a comment reads the number first. A rollup of 100 checks may be cut short and
  is not read as complete.
- **Writes are read back from the URL `gh` prints**, by number: the owner and name in it
  are GitHub's canonical spelling, which a rename or a difference of case changes. A call
  that timed out says that what it asked may have been done.
- **Scope check.** GitHub shows whether a token is accepted and reaches the repository, and
  its kind by its documented prefix. A classic token (`ghp_`, or 40 hex digits from before
  prefixes) and an OAuth token (`gho_`, what `gh auth login` makes) reach every repository
  and are refused; a fine-grained token's permissions and repository list are not shown, so it
  is reported unverified with what to check on its settings page.
- **Provenance.** The conformance suite runs over answers written by hand to `gh`
  2.104.0's `--json` shape (`tests/fixtures/`), since no test reaches GitHub. They prove the
  adapter reads that shape, not that `gh` prints it, so every capability reports
  `discovered` until answers recorded from a live `gh` replace them.
- **Default branch protection** (#695). The read takes the repository's default branch, the
  active rules every ruleset applies to it (`rules/branches/BRANCH`) and whether GitHub marks
  the branch protected; all three need read access alone. Classic branch protection also
  marks it, and its rules take administration to read, so a marked branch no ruleset rule
  applies to is reported with protection the read cannot see. One with ruleset rules too is
  reported by those rules alone. Protect writes one repository ruleset of Torlek's,
  `torlek-default-branch`, on `~DEFAULT_BRANCH`: deletion and non-fast-forward blocked, a pull
  request with no approval required, the owner's checks required, no bypass actors. It finds
  its own ruleset by name, reads it (`rulesets/ID`), writes nothing when it says exactly that
  (fields GitHub adds are not compared), and replaces it when it says anything else. The body goes on standard input (`--input=-`).
