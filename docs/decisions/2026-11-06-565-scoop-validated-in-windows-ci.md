# The Scoop route is validated by a release-workflow job on a Windows runner, against a loopback rehearsal

Issue #565. Decided by Theo on 2026-11-06 (route A: the Scoop own bucket with Windows CI
validation; WinGet stays deferred, as on 2026-11-03). The choices below are the worker's.

- **The `install-scoop` job lives in `release.yml`, beside `install-arch` and `install-wsl2`,
  and is not in `publish`'s needs.** It tests the candidate archive the release builds, so it
  belongs where the archive exists. No bucket is published, so a failure of it must not stop a
  tag; the publication checklist requires its success for the release it renders.
- **A dry run is rendered with `render.py rehearse`.** A dry run has no `v<tag>` git tag and no
  GitHub Release, so the manifest is rendered for its `-dev.N` version, the launchers are hashed
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
- **`install-in-wsl.sh` accepts a dry run's `X.Y.Z-dev.N`.** The job must install the archive it
  rendered; the bucket only ever carries alpha and beta tags, because `render.py render`
  refuses everything else.
- **`ci-install.ps1` refuses to run outside GitHub Actions.** It installs Scoop's app directory
  and shims and starts a server.
- **Rejected:** `file://` URLs in the rehearsed manifest (Scoop's downloader may accept them, but
  real users download over HTTPS, and a loopback HTTP server exercises the same path), and a
  self-hosted Windows machine (the owner has none; hosted runners ship WSL 2).
