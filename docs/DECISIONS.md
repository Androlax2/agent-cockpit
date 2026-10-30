# Engineering decisions

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
