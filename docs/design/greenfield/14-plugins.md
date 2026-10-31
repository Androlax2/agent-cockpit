# 14. Plugins

Status: decided (plugin-ready now; plugin system later)

## Decision

Design the refactor so a plugin ecosystem is possible, but do not build the plugin runtime,
manifest format or registry until a real plugin need exists. Chosen path: external-process
plugins over the protocol first, WASM in-process plugins later (option B).

## Plugin-ready now (part of the refactor)

- `cockpit-protocol` is **versioned from the first release** with capability negotiation, so
  external tools built later do not break on upgrade. Whatever external tools use becomes
  de facto stable; treat it as such from day one.
- **Extension points are named traits** at existing boundaries, used by the built-in
  implementations too: provider, workflow step, verification check, evidence source, notifier.
- The **change feed is subscribable** by any authenticated local client, not only the TUI.
- **Permissions are grants** (read state, change state, run commands, read pane contents,
  write clipboard…), so a future plugin manifest maps onto `cockpit-authorization` instead
  of a new system.
- Plugin failures must be isolatable and visible in diagnostics (crash, timeout, dropped
  events), never a silent task stall.

## Later (not in the refactor)

- WASM runtime inside daemon/TUI behind a capability API (Zed/Zellij model), an API crate,
  manifest with declared permissions and owner prompts, dev-install flow, API versioning duty.
- Estimated cost when started: external-process layer about 2–3 weeks on top of the
  design; WASM layer roughly 4–8 more weeks.

## Rules

- A plugin can never widen authority; it requests, Cockpit decides.
- Incompatible plugin versions are refused with a clear message, not loaded.
- Scheduling and admission policy hooks come last, if ever.

## Rejected

- Native dynamic libraries: no isolation, fragile ABI, one bug takes the daemon down.
- WASM from the start: freezes the extension API before real usage shows what it should be.

## Sources

- Zellij plugin permissions: <https://zellij.dev/documentation/plugin-api-permissions>
- Zed extensions (WASM, `extension.toml`, dev install): <https://zed.dev/docs/extensions/developing-extensions>
