# Gate M0: WSL2 is proven in CI through a pinned setup action

Issue #27. Decided by Theo, who has no Windows machine: he moved the WSL2 proof from a
hand-recorded run to CI. The `greenfield-wsl2` job runs on `windows-2025`, which ships WSL 2 as
the default; its first green run is on PR #128.

- **`Vampire/setup-wsl`, pinned to v7.2.0's commit**, installs Ubuntu 24.04. A bare
  `wsl --install -d Ubuntu --no-launch` leaves the distribution unregistered until a first
  launch whose account setup is interactive, which a hosted runner cannot answer; the action
  registers it non-interactively and provides the `wsl-bash` step shell. It is a new
  third-party action, which Theo allowed for this job.
- **Fail loud on the wrong kernel or user.** The job stops unless `uname -r` names a WSL 2
  kernel and the steps run as an ordinary user (`wsl-shell-user`), so neither a fallback to
  WSL 1 nor root's permission override can pass as WSL 2 evidence.
- **A clone, not the DrvFs checkout.** The tests run from a `git clone` in the
  distribution's home, which checks files out with Linux git: index modes and LF endings,
  not DrvFs's reported modes or the Windows checkout's line-ending conversion.
- **Downloads are pinned and verified.** rustup-init 1.29.2 and cargo-nextest 0.9.150 are
  fetched by version and checked against SHA-256 sums in the workflow; the Windows-side
  toolchain actions do not reach into WSL. rustup installs no toolchain itself, so
  `rust-toolchain.toml` alone picks it.
