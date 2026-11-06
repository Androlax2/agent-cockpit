# Windows packaging is a Scoop bucket of WSL launchers; WinGet is deferred until a native launcher exists

Issue #565. Decided by the worker on 2026-11-03, after Theo moved #565 into Alpha 1 that day,
from the evidence in [windows.md](../packaging/windows.md#dispositions). Theo deferred WinGet
on 2026-11-03 (option A below); it is deferred, not passed.

- **Scoop is prepared as an own bucket.** A bucket is any Git repository, and a manifest may
  shim `.ps1` scripts. `main` and `extras` require popularity and a stable release, which Torlek
  does not have yet. The bucket's repository and name are Theo's to choose, and it is published
  only with his approval.
- **The package ships launchers and the verified Linux archive, and changes nothing in WSL on
  its own.** `scoop install` only downloads; `torlek-wsl install` unpacks into the distribution
  after a `[y/N]` prompt. WSL, Windows features, distributions, elevation, services and
  environment variables are never changed; a missing one stops with the command to run.
  Rejected: a Scoop `installer`/`post_install` script that writes into WSL, because it would
  change a distribution without asking, and an `uninstaller`, because removal must never reach
  user distributions or data.
- **The manifest is rendered per release and binds five checksums.** The archive and
  `SHA256SUMS` come from the GitHub release; the three scripts come from the `v<tag>` git tag and
  are hashed as that tag holds them. Rejected: `checkver`/`autoupdate`, because a bot bump would
  publish hashes nobody reviewed, and committing a manifest with placeholder checksums.
- **The in-WSL layout `~/.local/lib/torlek/v<tag>` plus `current` is this route's own.** #97
  has not published the archive's install layout. If it chooses another one, the scripts change to
  match before the bucket is published.
- **The distribution is named, never guessed.** `torlek` reads `TORLEK_WSL_DISTRIBUTION` and
  stops when it is unset. Rejected: falling back to WSL's default distribution, which may be a
  different system from the one Torlek was installed in.
- **WinGet needs a native Windows executable.** Its community repository forbids script
  installers, and the client accepts only `.exe` as a portable file. Rejected: describing the
  Linux archive or a `.cmd` as an `exe` or `zip` installer, which misrepresents the payload.
  **Chosen (Theo, 2026-11-03): defer WinGet** to a later milestone, record the deferral on #97,
  and ship only the Scoop bucket. Rejected for now: a native `torlek.exe` launcher that only
  runs `wsl.exe` (a new Windows target, release asset, signing and SmartScreen reputation), and
  no Windows package manager at all.
- **Nothing here is verified on Windows.** The checklist in
  [windows.md](../packaging/windows.md#publication-checklist) lists the clean Windows + WSL 2
  run, `torlek doctor` and the #97 onboarding that remain.

## Revisited 2026-11-06 (#565, validated in Windows CI)

Decided by Theo on 2026-11-06 (route A: the Scoop own bucket with Windows CI validation;
WinGet stays deferred, as above). The choices below are the worker's. The Scoop route is
validated by a release-workflow job on a Windows runner, against a loopback rehearsal.
[windows.md](../packaging/windows.md) states the job's steps.

- **The `install-scoop` job lives in `release.yml`, beside `install-arch` and `install-wsl2`,
  and is not in `publish`'s needs.** It tests the candidate archive the release builds, so it
  belongs where the archive exists. No bucket is published, so a failure of it must not stop a
  tag; the publication checklist requires its success for the release it renders.
- **A dry run and a tag's run are rendered with `render.py rehearse`.** A dry run has no
  `v<tag>` git tag, and a tag's GitHub Release is not published while its run tests it, so the
  manifest is rendered for the candidate's version (`-dev.N` or the tag's), the launchers are hashed
  as the candidate commit holds them, and the downloads come from a loopback `http.server`.
  Scoop's own downloader, hash check and shims run unchanged. The rendered manifest refuses any
  non-loopback URL, so a rehearsal cannot be published by mistake.
- **Scoop is pinned by commit and run from a checkout.** Its installer downloads `master`, so a
  Scoop change could turn the job red or green without any change here. Rejected: the installer,
  and Scoop's `main` bucket as a dependency (the manifest has none).
- **PSScriptAnalyzer 1.24.0 runs under PowerShell 7 and Windows PowerShell 5.1**, the shells
  Scoop's shims start. `PSAvoidUsingWriteHost` is the single named exclusion: the launchers
  write to the user's console on purpose. The module comes from the PowerShell Gallery by exact
  version; the Gallery signs packages, and nothing else in the job fetches unpinned code.
- **`install-in-wsl.sh` accepts a dry run's `X.Y.Z-dev.N`,** because the job must install the
  archive it rendered; the bucket only ever carries alpha and beta tags, since
  `render.py render` refuses everything else. **`ci-install.ps1` refuses to run outside GitHub
  Actions:** it installs Scoop's app directory and shims and starts a server.
- **Rejected:** `file://` URLs in the rehearsed manifest (Scoop's downloader may accept them, but
  real users download over HTTPS, and a loopback HTTP server exercises the same path), and a
  self-hosted Windows machine (the owner has none; hosted runners ship WSL 2).
