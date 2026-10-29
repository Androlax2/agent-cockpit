# Architecture

This map includes the headless daemon and shared protocol extractions from engineering alignment.
Planned extractions are listed separately.

## Source owners

| Path | Responsibility |
| --- | --- |
| `apps/cockpit/src/app*` | Terminal application, refresh and screen composition |
| `apps/cockpit/src/bin/` | CLI and terminal executable entry points |
| `apps/cockpit/src/teams*`, `chief*` | Compatibility exports, client calls, SQLite stores and presentation |
| `apps/crates/cockpit-protocol/src/` | Team/Chief wire contracts, pure validators and agent identity catalog |
| `apps/cockpit/src/remote*`, `assets/remote/` | Phone server support and browser assets |
| `apps/cockpit/src/approval.rs` | Approval request helpers and terminal support |
| `apps/crates/cockpit-daemon/src/registry.rs` | Session registry, request queues and host-operation coordination |
| `apps/crates/cockpit-daemon/src/{teams,chief}.rs` | Daemon-side collaboration handling |
| `apps/crates/cockpit-daemon/src/fifo.rs` | FIFO/socket helpers, peer identity and bounded reads |
| `apps/osd/src/lib.rs` | Compatibility exports of the headless daemon modules |
| `apps/osd/src/bin/` | GTK OSD and idle-prompt executables |
| `config/bin/.local/bin/` | Installed wrappers, launchers and scripts |
| `config/claude/`, `config/codex/`, `config/opencode/` | Agent hooks and integrations |
| `tools/`, `packages/`, `system/` | Host maintenance, pinned sandbox builds and system setup |

## Dependency direction today

`osd` depends on `cockpit-daemon` and `cockpit`; `cockpit-daemon` depends on `cockpit`.
`cockpit` depends on `cockpit-protocol`, whose only external dependencies are serde,
serde_json and regex. Its Cargo-graph test rejects GTK, terminal UI and SQLite dependencies.
The daemon crate has no GTK runtime dependency, enforced by the Cargo-graph test in
`tools/tests/test_crate_boundaries.py`. The `cockpit` library still combines terminal UI,
state access, wire types and other shared behavior, so this is not yet a UI-free dependency
tree. GTK dependencies and executable ownership remain in `osd`.

Team and Chief types reference each other within the protocol crate. Existing
`cockpit::teams`, `cockpit::chief` and `cockpit::agents` paths re-export the contracts.
Chief storage still uses the team store's SQLite connection; these are not independent
databases. Explicit SQL-error conversions use `teams::store::storage_error`, preserving
the wire `Storage` code and SQLite error text without giving the protocol a database dependency.
Background modules still call the application refresh notifier.

## Runtime and trust boundaries

Installed wrappers resolve executables in `apps/target/release`. Stow links files from
`config/` into the user's home; `~/agent-cockpit` is the host's running copy, not an
agent development checkout.

Agent reports and requests enter daemon-controlled paths. The report socket's peer
identity comes from the kernel; request payloads cannot establish their own authority.
Host commands and user approvals remain distinct from agent requests. Preserve those
owners and their checks during extraction.

Tests use scratch home/runtime/state directories and fake external commands. Rust
integration tests live with their executable package; daemon tests inject host
operations through the registry's `System` boundary. Daemon fixtures resolve the checkout
from the three-level `apps/crates/cockpit-daemon` layout; other package fixtures retain
their two-level paths. Native roundtrips use the existing release `cockpit-team` binary,
which must be built before running the daemon tests on a clean checkout.

## Deferred UI boundary

A separate UI crate is not justified by the current ownership graph: refresh notification,
draft persistence and terminal presentation still share runtime state in `cockpit`.
The protocol extraction provides the useful dependency boundary without moving those
responsibilities or adding a reverse dependency. Reassess a UI crate only when that
state ownership can be separated for a concrete consumer.

Retain the `apps/` workspace and existing binary packages during these extractions.
See the [engineering plan](superpowers/plans/2026-10-28-engineering-workflow.md).
