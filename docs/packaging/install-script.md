# Install script: one-line verified install from the release archives

**Status:** [install.sh](../../install.sh) is written and tested (#1047). It is attached to every
release from v0.1.0-alpha.5 on; no published release carries it yet. The README shows it beside
the commands by hand, which stay valid.

```sh
curl -fsSL https://github.com/Androlax2/agent-cockpit/releases/download/v<tag>/install.sh | sh
```

## What it does

1. Refuses, before any download: a tag outside the release grammar (`vX.Y.Z-alpha.N` or
   `vX.Y.Z-beta.N`), a system other than Linux, a machine other than x86_64 or aarch64, root,
   and a missing `curl`, `tar`, `sha256sum`, `awk` or `mktemp`. It never calls sudo.
2. Picks the release: the highest-versioned published alpha in GitHub's release list (by
   version, not by creation date), or the one `--version vTAG` names, which must be a published
   release (GitHub answers 404 for a draft or a tag without a release). A newer beta is named
   with the `--version` to install it, never chosen: the issue asks for the alpha channel, and
   the default moves once betas replace alphas.
3. Downloads only that release's `torlek-vTAG-linux-ARCH.tar.gz` and `SHA256SUMS`.
4. Checks the archive's SHA-256 against its one `SHA256SUMS` entry, then its build provenance
   with `gh attestation verify` when a logged-in `gh` is present, then that every member is a
   plain file or directory under the bundle's one top directory. Any failure stops with nothing
   installed.
5. Unpacks into `~/.local/lib/torlek/torlek-vTAG-linux-ARCH` through a staging directory, and
   links `~/.local/bin/torlek` to its `bin/torlek`. Warns when `~/.local/bin` is not on `PATH`,
   and ends with the next steps: `torlek doctor`, `torlek daemon install`, `torlek init`.

Run again, it changes and downloads nothing when that release is already linked, relinks an
already unpacked release without a download (going back with `--version`), or installs a newer
release beside the old one, which it keeps. It refuses to replace a `~/.local/bin/torlek` that
is not a link into `~/.local/lib/torlek/torlek-v*/`. It never touches the profile's config or
state, never installs, starts or stops the daemon, and never removes a release.

`--check` does steps 1 to 4 and reports what installing would change, changing nothing. Files
given with `--archive` and `--sums` are checked even when that release is already installed.

### Layout

The README's archive layout (#97): `~/.local/lib/torlek/torlek-vTAG-linux-ARCH` and
`~/.local/bin/torlek` pointing into it. `torlek update` recognises exactly this layout. The
issue first named the Scoop/WSL route's `~/.local/lib/torlek/v<tag>` plus `current`; since #97
published the README's layout, both routes now use the README's
([decision](../decisions/2026-11-06-1047-install-script-one-installer.md)).

### One installer

The Windows route runs this same script inside WSL: [install-in-wsl.sh](../../packaging/windows/install-in-wsl.sh)
checks what is particular to WSL (a WSL 2 kernel, x86_64, a non-root default user), then runs
`install.sh --version vTAG --archive FILE --sums FILE` on the files Scoop downloaded and
verified, without the network ([Windows](windows.md)).

## What is verified, and what is not

| Check | Verified by |
| --- | --- |
| The archive matches the release's `SHA256SUMS` | Always, before unpacking |
| The archive was built by this repository's `release.yml` from `refs/tags/vTAG` on a GitHub-hosted runner | `gh attestation verify`, only when `gh` is installed and logged in; otherwise it says it skipped it |
| The archive cannot write outside its bundle | Always: member names and types are checked before unpacking |
| The tag is a published release | Always for downloads; with `--archive`, the caller's files are trusted (Scoop bound them to the release) |
| `install.sh` itself | Not by the script: piping it trusts GitHub's TLS and the release. The read-first route checks it against `SHA256SUMS` first, and `gh attestation verify install.sh --repo Androlax2/agent-cockpit` checks its provenance |
| `SHA256SUMS` itself | Only through provenance: the release attests every file it lists |

Read first, then run:

```sh
cd "$(mktemp -d)"
curl -fLO https://github.com/Androlax2/agent-cockpit/releases/download/v<tag>/install.sh
curl -fLO https://github.com/Androlax2/agent-cockpit/releases/download/v<tag>/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
less install.sh
sh install.sh
```

## Why a versioned URL

GitHub never serves a prerelease as `releases/latest`, and every Torlek release is a
prerelease, so `releases/latest/download/install.sh` is a 404. The README names the newest
release's asset instead (Theo, 2026-11-06), and the script still installs the newest alpha, so
an older URL keeps working. [Releasing](../RELEASING.md#cut-a-release) bumps the README's URL
after each release is published.

## Release and tests

- `release.yml`'s `assemble` job writes `SHA256SUMS` over the archives and `install.sh`, so the
  draft's provenance (`subject-checksums`) covers it, and the draft attaches it.
- `install-script-test` runs [test_install_script.py](../../tools/tests/test_install_script.py)
  and the Windows route's tests. They drive `install.sh` through `sh` with only its tools on
  `PATH`, against [a loopback stand-in for GitHub](../../tools/tests/fake_release_server.py),
  and again under `dash`, which CI requires: fresh install, newest alpha, upgrade, no-op re-run,
  pinned and unpublished tags, checksum mismatch, root, unsupported system and architecture,
  missing tools, `PATH` warning, foreign links, crafted archives, and provenance with a fake
  `gh`.
- `install-script` runs [ci-install.sh](../../packaging/install/ci-install.sh) in clean Ubuntu
  24.04 (`/bin/sh` is dash) and Arch containers: the run's own `install.sh` installs the run's
  archive as an ordinary user, `torlek version --all` and `torlek doctor` run, and a second run
  changes nothing. Neither job is in `publish`'s needs: the one-liner does not gate a tag.
- `TORLEK_INSTALL_ORIGIN=http://127.0.0.1:PORT` is the tests' only override: no other origin is
  accepted, provenance is not checked against it, and only there may a dry run's
  `vX.Y.Z-dev.N` be named.

Not verified: a real download through GitHub's redirects by the tests (a `--check` against
v0.1.0-alpha.4 was run by hand on 2026-11-06, provenance included), BusyBox `sh` and `tar`, and
macOS, for which no archive exists.
