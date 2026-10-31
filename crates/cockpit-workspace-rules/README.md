# cockpit-workspace-rules

Tests that every member of the greenfield workspace follows its shared rules.

It is not a capability crate and nothing depends on it. Its tests read each manifest under
`crates/` and fail with the crate's name when a member does not inherit the workspace's
edition, `rust-version` or lint policy from the root [Cargo.toml](../../Cargo.toml).
