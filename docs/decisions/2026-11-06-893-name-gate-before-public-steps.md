# Only the Nix index and the GitHub name gate the public Torlek steps; clearance waits for v0.1.0

Status: accepted

Issue #893. Decided by Theo on 2026-11-06 (relayed by the coordinator). Settles the "Not established" list of
[the product-name record](2026-11-02-339-product-name.md).

- **Before crate publication, release assets, the first registry package (#562, #563, #565) and
  the repository rename (#570), only two checks must pass:** the official Nix index and the
  GitHub name. Both pass (below).
- **Legal clearance is waived for alpha and beta prereleases and required before v0.1.0
  stable.** This includes the similar-mark and phonetic analysis (Torlen, one letter away, and
  returned marks). The risk Theo accepts: if clearance later blocks Torlek, the prerelease
  publications are redone under the backup name Dorlek.
- **The native-speaker slang check is waived the same way** and required before v0.1.0 stable.
  Both are tracked in [#1043](https://github.com/Androlax2/agent-cockpit/issues/1043).
- **GitHub name: stay on the personal account, no organization.** #570 renames the repository
  to `Androlax2/torlek`. The `torlek` GitHub handle is an unrelated personal account (created
  2013), so URLs, install paths and assets use `Androlax2/torlek`, never `github.com/torlek`.
- **Nix index check, re-verified 2026-11-06:**
  - `gh api repos/NixOS/nixpkgs/contents/pkgs/by-name/to/torlek`: HTTP 404.
  - `gh search code torlek --repo NixOS/nixpkgs`: no results.
  - This is evidence of absence from the `by-name` layout and the code search, not proof for
    every legacy path.
  - The coordinator ran the same two queries earlier the same day with the same results.
- **Rejected:** a GitHub organization, because it adds an owner to maintain for a one-person
  project; and holding all public steps until clearance, because prereleases are cheap to rename.
