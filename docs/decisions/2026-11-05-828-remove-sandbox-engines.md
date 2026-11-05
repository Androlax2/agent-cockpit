# Greywall and Greyproxy are removed; agents run unsandboxed by default and `required` refuses

Issue #828, stage 1 of the [#546 migration plan](../research/2026-11-05-546-own-sandbox-and-traffic-control.md#migration-plan).
Removal, the `off` default and the refusal decided by Theo (2026-11-05); the doctor flag by
Theo (Option A on #828); the rest by the implementing session.

- **No engine code, pins or verbs.** `greywall.rs`, `install.rs`, `proxy/`, `seccomp.rs`,
  `engine-pins.json`, `torlek sandbox install`/`status`/`restart`/`stop`, their tests and the
  engine CI tools are gone. The profile, `Refusal`, `ProviderReach` and the evidence stay in
  `cockpit-sandbox`: they are backend-neutral, and the native backend plugs in behind
  `Sandbox::prepare`.
- **The product default of `agents.sandbox` is `off`.** Nothing used the engines, so Theo
  accepts unsandboxed agents until the native sandbox is live (stage 5 returns the default to
  `required`). The daemon logs the risk at its start and at every launch; doctor, init and each
  launch's evidence say "no sandbox"; a reload that changes the setting logs it again.
  `cockpit_sessions::Settings::default()` follows the
  product default, which `cockpit-daemon` `sessions::a_clean_owner_configuration_keeps_the_sessions_product_defaults` checks.
- **An explicit `required` refuses every launch, naming the missing backend.** It never runs
  an agent unsandboxed: `Sandbox::prepare` returns a `Refusal` with
  `cockpit_config::NO_SANDBOX_BACKEND`, and doctor reports it as a missing prerequisite.
  Regression tests: `cockpit-sandbox` `launch::a_required_sandbox_refuses_every_launch_naming_the_missing_backend`,
  `cockpit-daemon` `sandbox::an_explicit_required_setting_refuses_the_launch_naming_the_missing_backend`.
  `torlek init` keeps its sandbox step open under `required`, which an earlier release's init
  advised as consent, and readiness blocks every project under it.
- **Old engine state goes only on the owner's word.** `torlek doctor` lists the
  `torlek-greyproxy-<profile>` unit and the `greyproxy`, `engines` and `sandbox` state folders
  without touching them; `torlek doctor --remove-old-sandbox` stops and disables the unit,
  deletes it and the folders, and stops at the first step that fails, naming the manual way
  past it. Only a unit carrying the marker line the old release wrote is Torlek's; a folder
  that is not a directory is left for the owner to inspect. Doctor warns that a session an
  earlier release launched in Greywall still needs them until it ends.
- **Rows recorded as `sandboxed` by an earlier release still read back.** The evidence schema is
  unchanged; this release records no such row, so owner observations (#433) are refused on
  every launch it makes.
- **Rejected:** keeping the engines installed but unused, because their install, service and
  pins would still need upkeep and review; falling back to unsandboxed under `required`,
  because the owner chose `required` to refuse exactly that; an interactive prompt in
  `doctor`, because doctor runs in scripts and agent sessions, so the confirmation is a named
  flag.

Supersedes the engine parts of `2026-11-01-61-sandbox-backend.md`,
`2026-11-01-476-release-engine-pins.md`, `2026-11-02-470-managed-engine-install.md` and
`2026-11-02-495-managed-greyproxy.md`.
