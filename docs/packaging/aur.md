# Arch Linux: the `torlek-bin` AUR recipe

**Status:** recipe template, renderer and fixture build are tested (#562). No AUR package is
published, and no release archive exists yet to bind it to. Do not document `yay -S torlek-bin`
as an install route until the [publication checklist](#publication-checklist) has passed.

This is an [AUR](https://wiki.archlinux.org/title/Arch_User_Repository) recipe that repackages
the published release archives. It is not an official Arch repository package, and it is not
the legacy desktop staging recipe in [packaging/arch](../../packaging/arch/PKGBUILD)
(`agent-cockpit-bin`, see [arch-package-staging.md](arch-package-staging.md)). The two share
no installed path, so both can stay installed.

## What exists

| File | Role |
| --- | --- |
| [packaging/aur/torlek-bin/PKGBUILD.in](../../packaging/aur/torlek-bin/PKGBUILD.in) | The reviewed recipe, with the tag and checksums as `@TOKEN@`s |
| [packaging/aur/render.py](../../packaging/aur/render.py) | `render` fills it from one release's `SHA256SUMS` and writes `.SRCINFO` with `makepkg --printsrcinfo`; `check-srcinfo` refuses a stale `.SRCINFO` |
| [tools/tests/test_aur_package.py](../../tools/tests/test_aur_package.py) | Tag and checksum refusals, `vercmp` order, `.SRCINFO`, and a `makepkg` build of a `tools/release.py` bundle of stand-in programs |

The rendered `PKGBUILD` and `.SRCINFO` are what the AUR repository holds; the template stays here.
A template rather than a committed recipe, because the recipe must name a real release's
checksums: there is no placeholder or `SKIP` checksum, and no `latest` URL.

## Package

- **Name:** `torlek-bin`, the AUR convention for a prebuilt package. It `provides` and
  `conflicts` with `torlek`, the name a later source-built recipe would take. Neither name was
  taken on the AUR on 2026-11-03 (RPC `info` and `search` returned nothing).
- **Architectures:** `x86_64` and `aarch64`, the two Linux archives of the Alpha 1 release plan
  (`ALPHA1_PLATFORMS` in [tools/release.py](../../tools/release.py)), each with its own
  `source_<arch>` and `sha256sums_<arch>`.
- **Layout:** the whole bundle goes to `/usr/lib/torlek` unchanged, with `/usr/bin/torlek` as a
  symlink into it. `torlek` finds `torlek-tui`, the daemon its helpers, and both the release
  manifest (`share/torlek/manifest.json`) through their own resolved path, so the bundle cannot
  be split across `/usr/bin` and `/usr/share`. The unchanged tree still passes the manifest
  check. The license is linked to `/usr/share/licenses/torlek-bin/LICENSE`.
- **Dependencies:** `glibc` and `gcc-libs` for the Rust programs, plus the
  [README](../../README.md#platforms-and-prerequisites)'s prerequisites: `git`, `curl` and `tmux>=3.6`. `systemd`
  is optional and needed only for `torlek daemon install` and `torlek sandbox install`.
  Provider CLIs are left out, because their AUR names are not owned here.
- **License:** Apache-2.0, the archive's `LICENSE`.

### Version order

pacman does not allow a hyphen in `pkgver`, so the renderer drops the tag's only hyphen:
`0.1.0-alpha.1` becomes `0.1.0alpha.1`. `vercmp` then orders `0.1.0alpha.1 < 0.1.0alpha.2 <
0.1.0alpha.10 < 0.1.0beta.1 < 0.1.0`, which the tests check. `0.1.0_alpha.1` or `0.1.0.alpha.1`
would sort after `0.1.0`, and a later stable package would then look like a downgrade. The
renderer accepts only `X.Y.Z-alpha.N` and `X.Y.Z-beta.N` tags, the release tags that exist
([RELEASING](../RELEASING.md#tags)).

### User state

The package owns files under `/usr` only. A profile's configuration, state and user services
(`torlek.service`, `torlek-greyproxy-*.service`) live in the user's home and are written by
`torlek daemon install` and `torlek sandbox install`, not by pacman, so `pacman -R` keeps them.
Their units still point at `/usr/lib/torlek/bin`: stop them with `systemctl --user disable --now`
before removing the package. An upgrade replaces the programs in place, and a running daemon
keeps the old one until it restarts.

## Publication checklist

Each step that downloads, installs or publishes needs its own approval; this list grants none.
Run it on an Arch host, for each published `vX.Y.Z-alpha.N` tag.

1. Download the release's checksums and both Linux archives:
   `gh release download "v$TAG" -R Androlax2/agent-cockpit -p SHA256SUMS -p "torlek-v$TAG-linux-*.tar.gz" -D "$WORK"`
2. In `$WORK`, check the archives against the checksums:
   `sha256sum -c --ignore-missing SHA256SUMS`, which must list both archives as `OK`.
3. Check the build provenance of each archive:
   `gh attestation verify "torlek-v$TAG-linux-x86_64.tar.gz" --repo Androlax2/agent-cockpit`,
   then the same for `linux-aarch64`.
4. Render the recipe:
   `python3 packaging/aur/render.py render --tag "$TAG" --sums "$WORK/SHA256SUMS" --out "$WORK/torlek-bin"`
5. Build it in a clean chroot (`extra-x86_64-build` from `devtools`) and run `namcap` on the
   `PKGBUILD` and the built package. Explain or fix every finding.
6. On a clean Arch fixture, install the built package with `pacman -U`, then run
   `torlek doctor` and the #97 onboarding scenario. Remove the package and confirm that the
   profile's state directories are still there. When an earlier `torlek-bin` was published,
   also upgrade from it. Record the commands and their output on #562.
7. Publish, only with Theo's approval: clone `ssh://aur@aur.archlinux.org/torlek-bin.git`, copy
   in the rendered `PKGBUILD` and `.SRCINFO`, run `render.py check-srcinfo` on the clone,
   then commit and push. This needs Theo's AUR account and its registered SSH key; its name
   replaces the template's `Maintainer` line.
8. Check the published route on a new fixture: `git clone https://aur.archlinux.org/torlek-bin.git`,
   then `makepkg -si` and `torlek doctor`. Link the evidence from #97.

**Updates:** a new tag goes through the whole checklist with `pkgrel=1`. A fix to the recipe
alone changes the template in this repository, raises `pkgrel` in the rendered recipe, and runs
`check-srcinfo` again before pushing.

## Not yet verified

- No published release exists, so neither the checksums of real archives nor the installed
  programs have run on Arch. The tests build the package from stand-in programs.
- No clean-chroot build, `namcap` run, clean-fixture install, upgrade or uninstall has run.
  This sandbox has no Docker for an Arch fixture.
- `aarch64` is rendered and tested only as recipe text. The fixture build runs on the host's
  architecture.
- On a host without `makepkg` (the Ubuntu CI runners), the `.SRCINFO`, `vercmp` and fixture
  tests report themselves as skipped. They are not passes.
