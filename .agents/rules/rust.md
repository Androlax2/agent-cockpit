# Rust changes

- The current programs' workspace is `apps/Cargo.toml`; retain its target directory and
  executable names. It takes only stabilization fixes until the M5 cutover.
- The greenfield workspace is the root `Cargo.toml` with `crates/`; members inherit its
  edition, `rust-version` and `[lints]` (`workspace = true`).
- Prefer cohesive modules and explicit calls over generic managers or one-call wrappers.
  Extract a crate only when its dependency boundary or independent testability pays off.
- Libraries return errors to their callers. Validate at input boundaries; do not turn
  malformed state into empty success values to satisfy a lint.
- Unsafe operations need a local explanation of pointer, lifetime and ownership safety.
  Refactors preserve those proofs; a broad suppression is not a proof.
- Keep a shared lint policy in the workspace when introduced, inherited by every member.
  A justified exception belongs at the smallest applicable source item with a reason.
- Keep wire representations and persistent schemas unchanged in a structural move.
- Preserve real entry-point tests. Moving an integration test changes
  `CARGO_MANIFEST_DIR` and may remove access to package-local `CARGO_BIN_EXE_*` values.

See [Development](../../docs/DEVELOPMENT.md) for focused and full verification.
