# A provider that is not usable is listed with how to install it, and blocks `cockpit doctor` only when none is usable

Issue #411 (wiring Codex and OpenCode beside Claude Code), decided by Theo on 2026-11-01,
within #50's readiness rules (`2026-11-01-50-readiness-report.md`).

- **Listed, never hidden.** Every provider the daemon wires gets a `provider <name>` line.
  One it cannot use (not on `PATH`, or failing its version) reads `not usable, optional:
  <reason>`, with how to install it as its next action.
- **Not blocking on its own.** It is `available` with advice, as an unsandboxed owner is (#50).
  No new status and no `optional` flag, so `Status::blocks` stays the one rule `cockpit doctor`
  exits on.
- **Blocking when nothing works.** `providers` is `missing-prerequisite` when no provider is
  usable, naming every supported one.
- **Required by the project through its preset.** A project needs what its preset's steps
  need (#50: capabilities come from the preset, not from a new key). `workflow preset` blocks
  when no usable provider reports them. Cockpit has no configuration key naming one provider
  as required; adding one is out of scope here.
- **Rejected:** `missing-prerequisite` for each one that is not usable (#50's first rule). With
  three providers wired, a host with only Claude Code would fail `doctor` for ever. Also
  rejected: leaving uninstalled providers out of the report.

Evidence: `cockpit-readiness` `report::a_provider_that_is_not_installed_is_listed_with_how_to_install_it_without_blocking`,
`report::a_host_where_no_provider_is_usable_blocks_naming_every_supported_one`;
`cockpit-daemon` `readiness::a_provider_that_does_not_run_is_listed_as_optional_and_exits_0`.
