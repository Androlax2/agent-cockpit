# Frontends and authorization are checked by allow-list; nothing below the daemon reaches scheduling

Issue #119. Decided by the worker, reading report 02's rules as written.

- **Allow-lists, not forbidden edges, for "imports X only".** `cockpit-tui` and `cockpit-web`
  may reach only `cockpit-client` and `cockpit-protocol`; `cockpit-cli` also the #77 list
  (`cockpit-config`); `cockpit-authorization` only `cockpit-turso`, as its own
  `crates/cockpit-authorization/tests/crate_graph.rs` allowed; that test is folded in and
  deleted, like the CLI's. A crate added later is
  forbidden until a decision names it, which a forbidden-edge list would silently allow.
- **Frontends left the `clients ->` rules.** Their allow-list is stricter, so one edge yields
  one violation. `CLIENTS` keeps `cockpit-client` and `cockpit-notify`.
- **"Lower capabilities never call scheduling" covers every workspace member** except the
  daemon, scheduling itself, the clients and the allow-listed crates (their own rules cover
  it), the chief included because it proposes and never places. It names its exemptions, not
  its sources, so a crate added later is checked. It replaces `workflows -> scheduling` and the
  scheduling half of the projects rule. Loosening it for one crate is a decision.
- **The CLI's `Cargo.lock` walk is gone.** `cargo tree` (the 2026-10-31 decision for #19) does
  not follow an intermediate crate's dev-dependencies, which Cargo never builds; the old test
  did, and checked a list that was not tied to report 02's names. The #77 entry in
  `docs/DECISIONS.md` still points at `dependency_edges`; the list now lives in `ALLOW_LISTS`.
- **Rejected:** listing every product crate as a forbidden target of authorization, because a
  new capability would pass until someone remembered to add it.
