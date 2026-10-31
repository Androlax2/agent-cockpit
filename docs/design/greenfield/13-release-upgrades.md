# 13. Release, upgrades and recovery

Status: proposed

## Decision

Each release is **one verified bundle**: binaries, helpers, workflow presets, agent
instruction bundle, with a manifest naming its contents, source revision and compatibility
requirements. "Merged" is not "installed"; "installed" is not "running".

## Packaging

- `dist` (cargo-dist) for archives, checksums and release manifests; platform packaging
  (Arch first, Homebrew for macOS) wraps the same artifacts. Windows receives the Linux
  package plus WSL-specific setup and validation.
- Install tests check that helpers and agent guidance are present, not only that the binary
  starts.
- Build provenance (attestation) and verification results are recorded separately: where an
  artifact came from, and what was tested against it.

## Running sessions during an upgrade

- Each session records the helper and instruction bundle version it started with; referenced
  bundles are retained until no session needs them.
- A compatible daemon update reconciles executions (report 03) before dispatching new work;
  an incompatible change requires a controlled restart with a clear message.
- Grants are live: preserving an instruction version never preserves a revoked grant.

## Database

- Capabilities own their migrations; startup runs one coordinated, ordered sequence and
  refuses to run on a newer schema than it knows.
- Backups through the engine's supported online mechanism (SQLite backup API semantics);
  verify Turso's equivalent before adoption is final (report 04).
- Restoring a backup is a reconciliation, not a resume: pending actions are re-checked
  against real process and provider state, and old approvals cannot become reusable.

## Release gate

- Rust checks, Clippy, dependency and license checks, and the crate-graph rules (report 02).
- Real CLI, terminal and provider-contract tests; version-skew and interrupted-action fixtures.
- Clean install and upgrade on Linux, macOS and WSL2.
- Sustained responsiveness run (report 03) against the exact candidate.
- Named not-run outcomes after a fail-fast; retrying a broad suite until green is not a fix.

## Visible versions

`cockpit version --all` shows installed release, running daemon, each session's helper and
instruction bundle, and database schema compatibility, so "update installed but these
sessions still use the previous bundle" is a normal, explained state.

## Sources

- dist: <https://github.com/axodotdev/cargo-dist>, archives
  <https://axodotdev.github.io/cargo-dist/book/artifacts/archives.html>, attestations
  <https://axodotdev.github.io/cargo-dist/book/supplychain-security/attestations/github.html>
- SQLite online backup: <https://www.sqlite.org/backup.html>
- Delivery-chain findings: `docs/audits/audit-2026-10-30.md` F1/F9
