# Architecture

This map includes the headless core, daemon and shared protocol boundaries.

## Source owners

| Path | Responsibility |
| --- | --- |
| `apps/cockpit/src/app*` | Terminal application, private panel/input/render modules and refresh scheduling |
| `apps/cockpit/src/bin/` | CLI and terminal executable entry points |
| `apps/cockpit/src/teams*`, `chief*` | Compatibility exports and terminal presentation |
| `apps/crates/cockpit-core/src/` | Headless state/services, client calls, SQLite stores, draft persistence and shared formatting |
| `apps/crates/cockpit-protocol/src/` | Team/Chief wire contracts, pure validators and agent identity catalog |
| `apps/crates/cockpit-core/src/remote*`, `apps/cockpit/assets/remote/` | Phone server support and browser assets |
| `apps/crates/cockpit-core/src/approval.rs`, `apps/cockpit/src/approval/terminal.rs` | Shared approval request helpers and separate terminal/PTY adapters |
| `apps/crates/cockpit-daemon/src/registry.rs` | Session registry, request queues and host-operation coordination |
| `apps/crates/cockpit-daemon/src/{teams,chief}.rs` | Daemon-side collaboration handling |
| `apps/crates/cockpit-daemon/src/fifo.rs` | FIFO/socket helpers, peer identity and bounded reads |
| `apps/osd/src/lib.rs` | Compatibility exports of the headless daemon modules |
| `apps/osd/src/bin/` | GTK OSD and idle-prompt executables |
| `config/bin/.local/bin/` | Installed wrappers, launchers and scripts |
| `config/claude/`, `config/codex/`, `config/opencode/` | Agent hooks and integrations |
| `tools/`, `packages/`, `system/` | Host maintenance, pinned sandbox builds and system setup |

## Dependency direction today

`osd` depends on `cockpit-daemon` and `cockpit`; daemon production dependencies use
`cockpit-core`, not `cockpit`. The terminal package depends on the same core and keeps
its existing executable ownership. Core consumes `cockpit-protocol`, whose only external
dependencies are serde, serde_json and regex. Cargo-graph tests reject GTK/Ratatui/Crossterm
and the terminal package from core/daemon production graphs, and also reject SQLite from
protocol. GTK dependencies and GTK executable ownership remain in `osd`.

Daemon tests retain a dev-only `cockpit` dependency for existing cross-layer App coverage.
Core's default-off `test-support` feature exposes only existing cache fixture methods
used by the terminal package's dev-dependency. A real normal-dependency feature-tree test
checks that production application dependencies do not enable it. Transaction barrier
hooks remain strictly `cfg(test)` within core; their tests moved with them, not into the
production feature. No test helper grants host authority.

Team and Chief types reference each other within the protocol crate. Existing
`cockpit::teams`, `cockpit::chief` and `cockpit::agents` paths re-export the same core/protocol definitions.
Chief storage still uses the team store's SQLite connection; these are not independent
databases. Explicit SQL-error conversions use `teams::store::storage_error`, preserving
the wire `Storage` code and SQLite error text without giving the protocol a database dependency.
Background services call the neutral core notification module; old `app::refresh`
notification paths re-export the same singleton. Draft text formatting is owned beside
draft persistence, not by App. Shared Chief digest/phone status does not instantiate a view;
view-local pending/error state remains in its wrapper.

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

## Module and crate ownership

The existing `cockpit` package is the UI/binary owner rather than adding a redundant
`cockpit-tui` package. Explicit compatibility exports keep its public import paths stable.
Private App children own feature methods; App retains shared state and visible dispatch
ordering. Method relocation does not imply independent state or pure redraws.

Core retains shared Team/Chief connection and transaction ownership. Headless inline unit
tests and transaction barrier regressions live with core; binary integration tests remain
with `cockpit` so `CARGO_BIN_EXE_*` and installed-path fixture resolution keep their owners.
The shared test-only link fixture is referenced through a verified relative path; it is
not a production dependency or a new fixture crate.

Retain the `apps/` workspace and existing binary packages during these extractions.
See the [module/core extraction plan](superpowers/plans/2026-10-29-rust-module-and-crate-boundaries.md).
