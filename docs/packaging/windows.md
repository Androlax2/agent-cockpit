# Windows: Scoop and WinGet for Torlek inside WSL 2

**Status:** the Scoop manifest template, its renderer, the two Windows launcher scripts and the
script they run inside WSL are written and tested on Linux, and the release workflow's
`install-scoop` job runs them on a hosted Windows runner with a WSL 2 distribution
([below](#validation-in-ci)) (#565). No bucket exists, nothing is published, and no release
exists yet to bind the manifest to. Do not document `scoop install` as an install route until
the [publication checklist](#publication-checklist) has passed.

Torlek is a Linux program. On Windows it runs **inside a WSL 2 distribution**, from the
`linux-x86_64` release archive ([user guide](../USER-GUIDE.md#platforms-and-prerequisites)). There
is no native Windows daemon, sandbox or provider support, and neither package manager route
below adds any.

## Dispositions

| Manager | Disposition | Reason |
| --- | --- | --- |
| Scoop, own bucket | **Supported route, prepared here** | A bucket is any Git repository of JSON manifests; `bin` may be a `.ps1` script |
| Scoop `main` | Not eligible now | Needs 500 stars or 150 forks and the latest *stable* version; Torlek has 0 and 0, and only prereleases |
| Scoop `extras` | Not eligible now | Needs 100 stars or 50 forks and the latest *stable* version |
| WinGet community repository | **Unsupported without a native payload: deferred** | Scripts are disallowed as installers; a portable installer must be an `.exe` |

### WinGet: what the policy and the client require

All read on 2026-11-03.

- The community repository's
  [installer types](https://github.com/microsoft/winget-pkgs/blob/89b39feb49ce424c3a1aa1d144d32515c0deac09/doc/Policies.md#installer-types)
  are MSIX, APPX, MSI, exe-based installers and fonts, possibly nested in a `.zip`, and
  "**Scripts are expressly disallowed as installers.** Examples include: Batch files (`.bat`),
  PowerShell scripts (`.ps1`)".
- The manifest schema 1.28.0
  ([installer schema](https://github.com/microsoft/winget-cli/blob/dafe5e7795c6b36e2a8cea86aa9fddec898df795/schemas/JSON/manifests/v1.28.0/manifest.installer.1.28.0.json))
  allows `InstallerType` `msix, msi, appx, exe, zip, inno, nullsoft, wix, burn, pwa, portable,
  font`, and no script type.
- A zip's nested `portable` file must be an `.exe`: `s_AllowedPortableFiletypes = { L".exe" }`
  in [ManifestValidation.cpp](https://github.com/microsoft/winget-cli/blob/dfad88cdd908941dfc8077bb981c5d4b6052a168/src/AppInstallerCommonCore/Manifest/ManifestValidation.cpp#L18-L20),
  checked under full validation, which `winget validate` and the pipeline run. The client also
  appends `.exe` to every portable command alias
  ([PortableFlow.cpp](https://github.com/microsoft/winget-cli/blob/b042b3b681e1ba4b70309c2ada7dc18b6b6b8bcf/src/AppInstallerCLICore/Workflows/PortableFlow.cpp#L245)),
  so a `.cmd` or `.ps1` launcher cannot even be linked as a command.
- The [validation pipeline](https://github.com/microsoft/winget-pkgs/blob/89b39feb49ce424c3a1aa1d144d32515c0deac09/doc/Validation.md)
  installs the package silently as a standard user, checks that "the main application
  executable can be located", and scans the installer and installed files with Defender. A
  `.tar.gz` of Linux ELF programs is none of the allowed types; labelling it `exe` or `zip`
  would misdescribe it, which the issue forbids.
- The [Windows Package Manager policies](https://learn.microsoft.com/en-us/windows/package-manager/package/windows-package-manager-policies)
  (updated 2026-08-30) require accurate metadata (1.1), allow depending on non-integrated
  software (1.2.3), and ask that a product on an incompatible device "detect that at launch and
  display a message to the customer detailing the requirements" (1.4.1). A native launcher could
  meet them; a script cannot be submitted at all.

So a WinGet entry needs a native Windows executable, for example a small `torlek.exe` that only
runs `wsl.exe`. None exists, and building one is a new Windows build target, release asset and
signing question. Theo deferred WinGet to a later milestone on 2026-11-03
([decision](../decisions/2026-11-03-565-scoop-bucket-winget-needs-native-launcher.md)). A
private WinGet REST source does not avoid it: it uses the same manifest types and client.

### Scoop: what the format and the buckets allow

All read on 2026-11-03; Scoop sources at
[`e6aa3b36`](https://github.com/ScoopInstaller/Scoop/tree/e6aa3b366bdee8ed138c1e0f7b85192ebdd35d0f), its `master`.

- [App manifests](https://github.com/ScoopInstaller/Scoop/wiki/App-Manifests): `bin` lists
  "programs (executables or scripts)"; `hash` holds one SHA-256 per `url`; a `#/name` URL
  fragment sets the local file name. Scoop writes a `.ps1` and a `.cmd` shim for a `.ps1` program
  ([core.ps1](https://github.com/ScoopInstaller/Scoop/blob/e6aa3b366bdee8ed138c1e0f7b85192ebdd35d0f/lib/core.ps1#L1012-L1043)).
- [Buckets](https://github.com/ScoopInstaller/Scoop/wiki/Buckets): "Anyone can set up their own
  bucket", added with `scoop bucket add <name> <git url>`. Its own example ships a `.ps1` as the
  app.
- [Main criteria](https://github.com/ScoopInstaller/Scoop/wiki/Criteria-for-including-apps-in-the-main-bucket)
  and the [Main](https://github.com/ScoopInstaller/Main/blob/8b0efb09e9cdeaa0c58e5c353558673ee33f10c2/.github/ISSUE_TEMPLATE/package-request.yml)
  and [Extras](https://github.com/ScoopInstaller/Extras/blob/bb57b25354439811249d3f818d700600a58ed97c/.github/ISSUE_TEMPLATE/package-request.yml)
  request templates require popularity (500/150 and 100/50 stars/forks) and the latest stable
  version. On 2026-11-03 `Androlax2/agent-cockpit` had 0 stars and 0 forks and no stable
  release, so only an own bucket is open. Revisit after a stable release.
- Scoop unpacks any download whose local name matches `\.(…|gz|…|tar|t[abgpx]z2?|…)(\.[^\d.]+)?$`
  ([depends.ps1](https://github.com/ScoopInstaller/Scoop/blob/e6aa3b366bdee8ed138c1e0f7b85192ebdd35d0f/lib/depends.ps1#L134-L146)).
  Unpacking on NTFS loses the bundle's Unix modes, so the manifest saves the archive as
  `torlek-v<tag>-linux-x86_64.archive`, which the tests check against that pattern.
- On Windows 11 arm64, a manifest without an `arm64` entry installs its `64bit` one
  ([manifest.ps1](https://github.com/ScoopInstaller/Scoop/blob/e6aa3b366bdee8ed138c1e0f7b85192ebdd35d0f/lib/manifest.ps1#L165-L181)),
  so the WSL-side check refuses an aarch64 distribution with an actionable message.
- `Compare-Version` orders `alpha` and `beta` before the release of the same core and compares
  numbers numerically ([versions.ps1](https://github.com/ScoopInstaller/Scoop/blob/e6aa3b366bdee8ed138c1e0f7b85192ebdd35d0f/lib/versions.ps1#L110-L185)),
  so the manifest's `version` is the tag itself, `0.1.0-alpha.1`. Not run: `install-scoop` installs a
  dry run's `X.Y.Z-dev.N`, not an alpha tag.

### WSL commands the launchers use

From the `wsl.exe` usage text in [Resources.resw](https://github.com/microsoft/WSL/blob/fe15c863b0537e705159e0cec45e0dfc4b8d1f36/localization/strings/en-US/Resources.resw)
and [Basic commands for WSL](https://learn.microsoft.com/en-us/windows/wsl/basic-commands)
(updated 2025-12-01), read 2026-11-03: `--distribution`, `--exec` ("without using the default
Linux shell"), `--cd` (an absolute Windows path is accepted) and `--list --quiet`. `wsl.exe`
writes UTF-8 instead of UTF-16 when `WSL_UTF8=1`
([WslClient.cpp](https://github.com/microsoft/WSL/blob/fe15c863b0537e705159e0cec45e0dfc4b8d1f36/src/windows/common/WslClient.cpp#L1883-L1886)).
The [documented](https://learn.microsoft.com/en-us/windows/wsl/basic-commands) `wsl --unregister`
permanently deletes a distribution's data; nothing here calls it.

## What exists

| File | Role |
| --- | --- |
| [packaging/windows/scoop/torlek.json.in](../../packaging/windows/scoop/torlek.json.in) | The reviewed Scoop manifest, with the tag and five checksums as `@TOKEN@`s |
| [packaging/windows/render.py](../../packaging/windows/render.py) | `render` fills it from one release's `SHA256SUMS` and the `v<tag>` git tag |
| [packaging/windows/torlek.ps1](../../packaging/windows/torlek.ps1) | The `torlek` command on Windows: forwards its arguments to the Linux `torlek` in WSL |
| [packaging/windows/torlek-wsl.ps1](../../packaging/windows/torlek-wsl.ps1) | `torlek-wsl check` and `torlek-wsl install`: Windows-side checks and the consent prompt |
| [packaging/windows/install-in-wsl.sh](../../packaging/windows/install-in-wsl.sh) | Runs inside the distribution: WSL 2 kernel, architecture, user, checksum, then the unpack |
| [packaging/windows/ci-analyze.ps1](../../packaging/windows/ci-analyze.ps1) | Parses the two launchers and runs PSScriptAnalyzer on them; the workflow runs it under PowerShell 7 and Windows PowerShell 5.1 |
| [packaging/windows/ci-install.ps1](../../packaging/windows/ci-install.ps1) | The `install-scoop` job's scenario on a Windows runner; never for a host |
| [tools/tests/test_windows_package.py](../../tools/tests/test_windows_package.py) | Rendering, checksum binding, tag handling, the in-WSL refusals, and source checks of the PowerShell |

The manifest downloads five files, each with its own SHA-256: the release's `linux-x86_64`
archive and its `SHA256SUMS` from the GitHub release, and the three scripts from
`raw.githubusercontent.com` at the `v<tag>` git tag. The renderer hashes the scripts as that git
tag holds them (`git show v<tag>:<path>`), not the working tree, and the checksum file as given.
There is no placeholder checksum, no `latest` URL, no `checkver` or `autoupdate`: a new release is
rendered and reviewed, not bumped by a bot.

## How it works

1. `scoop install torlek` downloads and verifies the five files into the app directory and
   shims `torlek` and `torlek-wsl`. It runs no install script and changes nothing in WSL.
2. `torlek-wsl check -Distribution <name>` reports, without changing anything: WSL missing,
   no distribution registered, the named one not registered, then inside it a non-WSL 2
   kernel, an architecture other than `x86_64`, root as the default user, missing `sha256sum`,
   `tar` or `awk`, and an archive whose SHA-256 differs from `SHA256SUMS`.
3. `torlek-wsl install -Distribution <name>` repeats the check, asks
   `Install Torlek <tag> into WSL distribution '<name>' at ~/.local/lib/torlek/v<tag> …? [y/N]`
   (or takes `-Yes`), refuses archive members outside the bundle directory and links, unpacks into
   `~/.local/lib/torlek/v<tag>` and points `~/.local/lib/torlek/current` at it. It never
   replaces an installed version.
4. `torlek …` on Windows runs `$HOME/.local/lib/torlek/current/bin/torlek …` in the
   distribution named by `TORLEK_WSL_DISTRIBUTION`. There is no default distribution: an unset
   variable stops with the command that sets it, which the user runs.

The whole bundle stays in one directory, as in the [AUR recipe](aur.md#package), because `torlek`
finds its other programs and the release manifest from its own resolved path.

### Consent and prerequisites

Nothing here installs or enables WSL, enables a Windows feature, adds, converts or removes a
distribution, elevates, registers a service or sets an environment variable. Each missing
prerequisite stops with the command for the user to run, for example
`wsl.exe --install --no-distribution` (elevated, enables Windows features, may restart) or
`wsl.exe --install --distribution Ubuntu-24.04`. The source checks in the tests hold the
scripts to `wsl.exe --distribution/--exec/--cd/--list --quiet`.

Inside WSL, Torlek's own [prerequisites](../USER-GUIDE.md#platforms-and-prerequisites) still
apply: tmux 3.6 or newer (Ubuntu 24.04 ships 3.4), Git, curl and a systemd user manager for
`torlek daemon install`. `torlek doctor` names what is missing; the package does not install it.

## Update and uninstall

- **Update:** `scoop update torlek` replaces the Windows files with the next rendered release.
  The Linux side changes only when the user runs `torlek-wsl install` again, which adds
  `v<new tag>` beside the old one and moves `current`. User services written by
  `torlek daemon install` point at the resolved old path: run `torlek daemon install` again after
  moving `current`, as with any archive upgrade.
- **Uninstall:** `scoop uninstall torlek` removes only Scoop's app directory and shims. The
  manifest has no uninstaller, so it never touches a WSL distribution, `~/.local/lib/torlek`, a
  profile's state, or user services inside WSL. Removing them is the user's choice: inside WSL,
  stop the services, then delete `~/.local/lib/torlek/v<tag>`. A distribution is never
  unregistered by this package.
- **Going back:** an older installed version is selected with
  `ln -sfn v<tag> ~/.local/lib/torlek/current` inside WSL; the refusal message prints that command.

## Validation in CI

The release workflow's `install-scoop` job (`windows-2025`, with an Ubuntu 24.04 WSL 2
distribution from the same pinned action as `install-wsl2`) runs on every dry run, a nightly,
a dispatch and a tag, and on pull requests that change these files. It is not in `publish`'s
needs: the route is unpublished and does not gate a tag
([decision](../decisions/2026-11-06-565-scoop-validated-in-windows-ci.md)). Nothing runs on a
maintainer's machine.

1. `ci-analyze.ps1` parses both launchers and runs PSScriptAnalyzer 1.24.0 (errors and warnings
   fail; `PSAvoidUsingWriteHost` is the one named exclusion, because the launchers talk to the
   console) under PowerShell 7 and Windows PowerShell 5.1.
2. `render.py rehearse` renders the manifest for the candidate's `-dev.N` version: the launchers
   are hashed as the candidate commit holds them, and every download points at a loopback
   `http.server` that serves the candidate archive, its `SHA256SUMS` and those launchers.
   `rehearse` refuses any URL that is not loopback, so its manifest cannot be published.
3. A pinned Scoop (v0.6.0, run from a checkout of commit `e6aa3b36`, not from its installer,
   which follows `master`) installs that manifest. The job checks the five downloaded files and
   the two shims, and that WSL is unchanged.
4. Under both shells: `torlek-wsl check` succeeds, and the refusals fire: unknown distribution,
   no distribution named, `TORLEK_WSL_DISTRIBUTION` unset, Torlek not yet installed in WSL, and
   no `wsl.exe` on `PATH`. `torlek-wsl check` also runs through Scoop's `.cmd` shim, as a
   `cmd.exe` user would. If the distribution starts as root (setup-wsl's `wsl-shell-user` applies
   to its own shell wrapper), the job first makes `cockpit` the distribution's default user.
5. Declining the consent prompt installs nothing. A tampered archive is refused by `check` and by
   `install -Yes`, and installs nothing.
6. `torlek-wsl install -Yes` installs; a second install keeps the first; `release.py
   verify-install` accepts the installed bundle against the candidate revision; `torlek version
   --all` and `torlek doctor` run through the Windows launcher.
7. `scoop uninstall torlek` removes the app directory and shims, and the job checks that the
   distribution, the Linux install, its `current` link and a seeded file in the user's home are
   still there.

Not covered by this job: a WSL 1 distribution and an arm64 machine (the in-WSL script's refusals
for both are tested with a faked `uname`), a machine without WSL installed at all (only a `PATH`
without `wsl.exe` is simulated), the real bucket, and the #97 onboarding.

## Publication checklist

Each step that downloads, creates a repository, installs or publishes needs its own approval;
this list grants none. For each published `vX.Y.Z-alpha.N` tag:

1. Download the release's checksums and archive, then verify them and their provenance as in the
   [AUR checklist](aur.md#publication-checklist), steps 1 to 3, for `linux-x86_64`.
2. In a checkout that has the `v$TAG` tag, render the manifest:
   `python3 packaging/windows/render.py render --tag "$TAG" --sums "$WORK/SHA256SUMS" --out "$WORK/bucket"`
3. Check that the tag's release run passed `install-scoop` (the same scenario on the release's
   own archive, rendered for its `-dev.N` rehearsal). Record its run URL on #565.
4. On a clean Windows 11 x64 machine with WSL 2 and a fresh Ubuntu 24.04 distribution with tmux
   3.6 or newer: `scoop install "$WORK/bucket/torlek.json"`, then `torlek-wsl check`,
   `torlek-wsl install`, `torlek doctor` and the #97 onboarding scenario from Windows Terminal.
   Then check a WSL 1 distribution and an arm64 machine, which CI does not. Record the commands
   and output on #565.
5. Publish, only with Theo's approval: create the bucket repository he names, add the rendered
   `torlek.json` under `bucket/`, and push it.
6. Check the published route on a new machine: `scoop bucket add <name> <url>`,
   `scoop install <name>/torlek`, `torlek-wsl install`, `torlek doctor`. Link the evidence from #97.

## Not yet verified

- `install-scoop` first went green on run 42282361877 (#1007, version `0.1.0-dev.316`): every
  step of "Validation in CI" passed on a Windows runner, including the Windows PowerShell 5.1
  and cmd.exe paths. The assumptions that run confirmed: `wsl.exe --exec` passes each argument
  intact; `--cd` accepts Scoop's `current` junction; `@args` carries named parameters through
  Scoop's `.ps1` shim; `wsl.exe --list --quiet` output under `WSL_UTF8=1`; a runner's `python`
  serves loopback. One hosted run, not a soak: a flaky download (the PowerShell Gallery has
  timed out once) can still fail it, and the job retries that download.
- The run also found three things worth knowing: Scoop 0.6.0 writes `scoop-manifest.json` and
  `scoop-install.json` next to the five bound files; Scoop's `.ps1` shim turns piped input into
  pipeline input for the script, so a scripted answer to the consent prompt goes to the
  installed script, not the shim; and `torlek-wsl install` once treated a missing answer as yes
  (fixed, with a regression test).
- `install-in-wsl.sh` also ran on an Arch host with faked `uname` and `id`, not in a WSL kernel
  (`install-scoop` runs it in one).
- No release exists, so no real archive was rendered, downloaded or installed; the job uses a
  dry run's `-dev.N` archive.
- A published bucket, a WSL 1 distribution, an arm64 machine and the #97 onboarding are unrun.
- The in-WSL layout `~/.local/lib/torlek/v<tag>` and `current` is this route's own; #97 has not
  yet published the archive route's install instructions. If #97 picks another layout, change
  the scripts to match it before publishing.
