# install.sh is the one archive installer, in the README's layout, served from a versioned release URL

Issue #1047. The URL decided by Theo (option A, 2026-11-06); the layout and the shared
installer by the worker, following the issue's rule that #97's layout wins.

- **One installer.** [install.sh](../../install.sh) holds every check and the unpack;
  [install-in-wsl.sh](../../packaging/windows/install-in-wsl.sh) adds only the WSL checks and runs
  it on Scoop's files with `--archive`/`--sums`, and the Scoop manifest downloads it as a sixth
  file. Rejected: a second copy of the unpack logic, which the issue forbids.
- **The README's layout:** `~/.local/lib/torlek/torlek-vTAG-linux-ARCH` and
  `~/.local/bin/torlek`, which #97 published and `torlek update` recognises. The WSL route moved
  to it from `v<tag>` plus `current`, which no release had shipped. Rejected: keeping two layouts.
- **Versioned URL.** `releases/download/vTAG/install.sh`, bumped in the README after each
  release (docs/RELEASING.md). GitHub never marks a prerelease `latest`, so
  `releases/latest/download/install.sh` is a 404. Rejected: publishing alphas as full releases
  marked latest, and a docs-site copy.
- **Provenance when `gh` can check it.** A logged-in `gh` must verify the archive; without `gh`,
  or logged out, the script says it skipped it and relies on the checksum. Rejected: failing
  without `gh`, which would make the one-liner need a GitHub login.
- **Not a gate.** The `install-script` jobs stay out of `publish`'s needs, like the AUR route.
