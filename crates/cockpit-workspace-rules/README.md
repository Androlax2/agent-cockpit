# cockpit-workspace-rules

Tests that every member of the greenfield workspace follows its shared rules.

It is not a capability crate and nothing depends on it. Its tests fail, naming the crate,
when a member under `crates/`:

- does not inherit the workspace's edition, `rust-version`, `publish` setting or lint
  policy from the root [Cargo.toml](../../Cargo.toml);
- does not state its ownership as a one-sentence `description` that its README repeats;
- depends, directly or through other crates, on a crate one of the forbidden edges of
  [report 02](../../docs/design/greenfield/02-crate-architecture.md) and issue #19 rules
  out. The failure names the rule, for example `clients -> persistence`;
- is a frontend, `cockpit-authorization` or `cockpit-messaging` and reaches a `cockpit-*`
  crate outside its allow-list (report 02, #77, #119, #41), for example
  `frontends -> client/protocol only`;
- is depended on by another member while being `cockpit-daemon`, which wires capabilities
  together and so sits above all of them (rule `nothing -> daemon`).

The forbidden edges and allow-lists are tables in `tests/crate_graph.rs`, keyed by crate
name. A test fails when a table names a crate report 02 no longer lists, so a rename updates
both.
