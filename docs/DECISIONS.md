# Engineering decisions

## 2026-11-01 — Prepare legacy Codex trust on the host

`cockpit-agents/codex-trust.py` inherits an exact trusted source's folder decision
only during fresh non-PR clone creation. It never reconstructs provenance from a
clone's name, Git remote or project-owned files. Explicit destination decisions stay intact.
All native Cockpit Codex launch modes prepare hook trust before starting tmux.
Only the seven reviewed user-level `cockpit-codex-hook` declarations qualify;
changed declarations and TOML hook definitions require host review. Project hooks
receive no trust entries, and disabled user hooks remain disabled.

The format is sourced from Codex's `rust-v0.161.0` tag:
[hook keys](https://github.com/openai/codex/blob/rust-v0.161.0/codex-rs/hooks/src/lib.rs),
[normalization](https://github.com/openai/codex/blob/rust-v0.161.0/codex-rs/hooks/src/engine/discovery.rs),
[hashing](https://github.com/openai/codex/blob/rust-v0.161.0/codex-rs/config/src/fingerprint.rs)
and [TUI persistence](https://github.com/openai/codex/blob/rust-v0.161.0/codex-rs/tui/src/hooks_rpc.rs).
Folder trust is `projects."<absolute path>".trust_level = "trusted"`;
hook trust is `hooks.state."<hooks.json path>:<snake_case event>:0:0".trusted_hash`,
containing `sha256:` followed by the normalized identity's canonical JSON hash.

The helper parses both the original and proposed TOML before appending missing tables.
It never truncates or replaces the inode that existing sandboxes bind-mount.
Existing entries needing edits fail closed rather than guessing how to rewrite TOML.
An advisory lock serializes Cockpit writers, with content/inode checks around the write.
Codex's own writer uses atomic replacement without this lock: simultaneous native
host config edits are not supported; detected replacements stop preparation.
The sandbox's config and hook write restrictions remain the security boundary.
Tests use scratch homes, fake launch commands and the manager's clone entry point.

## 2026-10-30 — Keep Chief coordination separate from observed todo ownership

An observed in-progress todo remains a work reservation, but does not by itself mean
the Chief is executing a native turn. Wake readiness permits only the exact active,
admitted Chief to pass this sole blocker when every observed reservation belongs to
its own project. The reservations and generic worker readiness remain unchanged.

Real legacy reservations, pending or uncertain launches (including orphaned launch
records), product-worker membership and all native, identity, focus and gaming holds
still prevent delivery. Regression tests exercise the authenticated Receive/Receipt
and Context routes, including a subsequent automatic wake; fixtures are not live
admission or draft-safety evidence.

## 2026-10-29 — Keep database error conversion at the store boundary

Team and Chief wire contracts share `cockpit-protocol`, which has no SQLite or UI
dependency. Moving `TeamError` prevents `cockpit` from implementing the foreign
`From<rusqlite::Error>` trait for it. SQL callers therefore explicitly map errors with
`teams::store::storage_error`, retaining `ErrorCode::Storage` and `error.to_string()`.
The existing stores retain their queries, connections and transaction ownership.
A public approval-operation regression checks the serialized SQL error and rollback
of membership when the final proposal write fails.

## 2026-10-28 — Reuse Make as the verification entry point

Cockpit already owns build/test resource controls in Make and its helper scripts.
Local checks and future CI will share those recipes rather than introducing an `xtask`
crate solely to wrap commands. OpenKeychron demonstrates the smaller Make-based approach.

## 2026-10-28 — Extract responsibilities without relocating the workspace

Keep `apps/Cargo.toml` and `apps/target/release`. Installed wrappers and integration
tests depend on that layout. New libraries may live under `apps/crates/`; binary owners
stay in their existing packages initially. A root-level `crates/` move has no demonstrated
benefit worth its operational churn.

## 2026-10-28 — Start from the current applied source

The initial parallel clone inherited broker work later reverted on main. Theo approved
a fresh `refactor/5779dac9-engineering` branch at verified `origin/main` (`652d6e2`),
preserving the old branch and uncommitted work. Only reviewed audit/planning documents
carry forward; reverted broker changes are not part of engineering alignment.
