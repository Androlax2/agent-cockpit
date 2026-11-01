# Every agent launches through `cockpit-sandbox`, Greywall its first backend, unsandboxed only by the owner's setting

Issue #61 (option C, decided). The points below were decided by the implementing session,
within reports 11 and 17 and decision register item 15.

- **The daemon launches through the sandbox; no launcher script.** `cockpit-sessions` calls
  `cockpit_sandbox::Sandbox::prepare` for every launch, and the evidence is recorded in the
  launch's own transaction (`sandbox_launches`). That way a launch the database names always
  says how its agent ran. Only `cockpit-sandbox` builds a greywall command line.
- **Unsandboxed only by `agents.sandbox = "off"` in the owner's file.** `[agents]` is
  owner-only, so a project file that sets it is refused. The default is `required`. Each
  unsandboxed launch logs a warning and records `unsandboxed` evidence with the reason. A
  host without a usable engine refuses launches; it never falls back. Tests that drive
  something other than the sandbox use this one opt-out (`common::unsandboxed_settings`, the
  TUI e2e owner file). Daemon binary tests use a stand-in greywall instead, which proves
  launches go through it.
- **A fresh greyproxy client per launch: `cockpit-<session>-<12 hex>`.** Greyproxy keeps one
  client-secret registration per name, and greywall registers a per-run secret. Two launches
  under one name replace each other's secret, and the earlier sandbox loses its network
  (stack proposal 00f3a41c). A random suffix from `/dev/urandom` keeps names apart across
  restarts of one session.
- **The provider's own needs come from greywall's reviewed engine profile**
  (`--profile claude|codex|opencode`, `--auto-profile`). Cockpit adds its baseline denials,
  the checkout (read-write), the agent socket folder and the bridge programs (read-only), and
  its own files (denied). A provider without an engine profile is refused. Moving those needs
  into `cockpit-provider-api` is a later step.
- **Refuse, never weaken.** A rule the host cannot enforce refuses the launch and names the
  rule and the reason. Examples: a missing Linux capability, an allowed folder inside a denied
  one (greywall applies denials last), a checkout on a WSL2 Windows drive, desktop variables
  in a launch that withholds the desktop session, or an unsupported platform. WSL2 also denies
  the Windows drives and `/run/WSL` and drops `WSL_INTEROP`.
- **The launch token passes through (`--ignore-secret COCKPIT_LAUNCH_TOKEN`).** The agent
  presents it on the local agent socket, never in an HTTP header, so a greyproxy placeholder
  would break it.
- **Pins live in `cockpit-sandbox` and must equal `packages/*/build.sh`.** A test compares
  them. Another build is refused at daemon start, naming both commits.
- **Evidence is read with `cockpit session sandbox SESSION`, not `cockpit session show`.**
  `session show` is the manager's viewer action: it needs tmux and wakes the session. The dry
  run is `cockpit sandbox show DIR [--agent NAME]`. Both requests are owner-only.
  `cockpit doctor` does not exist yet (#50). Until it does, the daemon log at start and
  `sandbox show` report the engine's readiness.
- **Rejected:** keeping the greywall launcher scripts in front of providers. The daemon could
  not record what each launch got, and the client-secret race stays.
- **Rejected:** a silent unsandboxed fallback when greywall is missing. That hides the
  missing protection from the owner.
