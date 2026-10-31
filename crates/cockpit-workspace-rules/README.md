# cockpit-workspace-rules

Tests that every member of the greenfield workspace follows its shared rules.

It is not a capability crate and nothing depends on it. Its tests fail, naming the crate,
when a member under `crates/`:

- does not inherit the workspace's edition, `rust-version`, `publish` setting or lint
  policy from the root [Cargo.toml](../../Cargo.toml);
- does not state its ownership as a one-sentence `description` that its README repeats;
- depends, directly or through other crates, on a crate that
  [report 02](../../docs/design/greenfield/02-crate-architecture.md) forbids. The failure
  names the rule, for example `clients -> persistence`.

The forbidden edges are a table in `tests/crate_graph.rs`, keyed by crate name: renaming a
crate means renaming it there.
