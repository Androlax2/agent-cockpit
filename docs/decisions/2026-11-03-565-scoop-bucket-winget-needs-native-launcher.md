# Windows packaging is a Scoop bucket of WSL launchers; WinGet waits for a native-launcher decision

Issue #565. Decided by the worker on 2026-11-03, after Theo moved #565 into Alpha 1 that day,
from the evidence in [windows.md](../packaging/windows.md#dispositions). WinGet's route needs
Theo's scope decision and is stopped, not passed.

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
  The options for Theo:
  - **A (recommended for Alpha 1): defer WinGet** to a later milestone, record the deferral on
    #97, and ship only the Scoop bucket.
  - **B: build a native `torlek.exe` launcher** that only runs `wsl.exe` (a new Windows
    target, release asset, signing and SmartScreen reputation), then submit a `zip` + `portable`
    manifest. It is a launcher, not native Torlek support.
  - **C: no Windows package manager at all**; document the archive inside WSL only.
- **Nothing here is verified on Windows.** The checklist in
  [windows.md](../packaging/windows.md#publication-checklist) lists the clean Windows + WSL 2
  run, `torlek doctor` and the #97 onboarding that remain.
