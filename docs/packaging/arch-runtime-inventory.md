# Arch runtime inventory

**Status:** inspected input inventory, not a package manifest or a working installer.
Target: Theo's Linux x86_64 Arch setup. Proposed destinations below are not yet implemented.
Source inspected: `f84ba9f5d8b36a982737c0505890f6d00659ef6e`, the final PR 14 head.
Theo merged it as `681effeb7b19d3f64ff29dd92911ab3cb5cfcc26`; this inventory does not claim
to have tested that merge commit. The package work is on a separate documentation branch.

Scope: all **111 tracked `config/` paths**, all **17 Rust binary source entry points**,
component scripts and the additional helper/asset/service boundaries identified below.
No untracked file, live user configuration or external symlink target was imported.
Read together with the [staging design](../superpowers/specs/2026-10-30-arch-binary-package-design.md).

## Destination and activation rules

`P` means proposed `/usr/lib/agent-cockpit`; `I` means proposed
`/usr/share/agent-cockpit/integrations`. These are inventory notation, not environment
variables the application understands. Retain the private tree's relative paths initially.

| Class | Destination rule | Activation requirement |
| --- | --- | --- |
| R | `P/<tracked source path>` for runtime scripts/helpers | Inert files only; explicit public command links are listed below; executing them retains existing host/agent boundaries |
| O | `I/<tracked source path>` for opt-in registrations, presentation and service templates | Separate reviewed user registration/config merge; never copy over the live home or auto-enable services |
| S | No distributable payload selected | Security-sensitive policy/composition review required; no grant changes inferred |
| E | Excluded | Source records user-specific enabled-service intent, not package defaults |
| D | Excluded from runtime | Source development/build material; keep in repository |

Runtime hooks classified R still require reviewed provider registration; classification never
authorizes activation. O files are intended inert examples, not already relocatable units.
Any R row with a blocker is required work, not permission to silently omit that capability.

Public links initially preserve the existing top-level command spellings for R rows under
`config/bin/.local/bin/`, including compatibility aliases. Nested `cockpit-agents` and
`greywall-agents` files stay private support files, not bare `/usr/bin/claude` or
`/usr/bin/opencode` replacements. Their current launcher references need package support.
The clipboard shim is sandbox-only; never shadow the host's `/usr/bin/wl-paste`.

## Rust executable artifacts

These rows identify source entry points, not source files to copy into a runtime package.
Each produces `apps/target/release/<source filename without .rs>`, staged at the same path
under P. Public names use the existing wrapper rows in the next section, not direct links
that would skip wrapper behavior. Cargo library crates are linked into consumers; no `.rlib`,
Cargo manifest, test executable, cache or entire `target/` directory is a runtime payload.

| Tracked source | Consumer / runtime requirement |
| --- | --- |
| `apps/cockpit/src/bin/ask-to-guide.rs` | Agent-facing guided-step request client; authenticated host transport |
| `apps/cockpit/src/bin/ask-to-run.rs` | Agent-facing command request client; host approval remains required |
| `apps/cockpit/src/bin/claude-audit.rs` | Audit reader; user state and gzip for compressed history |
| `apps/cockpit/src/bin/cockpit-codex-hook.rs` | Codex lifecycle reporting; daemon report transport |
| `apps/cockpit/src/bin/cockpit-demo.rs` | Explicit demo command, not a service |
| `apps/cockpit/src/bin/cockpit-design.rs` | Design/session reporting client; not authority to launch a design app |
| `apps/cockpit/src/bin/cockpit-fade.rs` | Managed terminal pane wrapper; tmux/PTY |
| `apps/cockpit/src/bin/cockpit-failsafe.rs` | Resource protection; Linux process/cgroup state and existing host controls |
| `apps/cockpit/src/bin/cockpit-guide.rs` | Host guided-step view; Kitty/terminal and daemon |
| `apps/cockpit/src/bin/cockpit-remote.rs` | Optional phone server; embedded web assets, tmux, existing pairing/state |
| `apps/cockpit/src/bin/cockpit-run-approve.rs` | Host approval terminal; authenticated requests and terminal/PTY |
| `apps/cockpit/src/bin/cockpit-sidebar.rs` | Terminal session manager UI; tmux and shell cockpit command |
| `apps/cockpit/src/bin/cockpit-subscription.rs` | Subscription state/queue helper; provider collectors and user-owned state |
| `apps/cockpit/src/bin/cockpit-team.rs` | Collaboration CLI; daemon transport and user-owned database |
| `apps/cockpit/src/bin/cockpit-todo.rs` | Todo CLI; daemon transport and user-owned database |
| `apps/osd/src/bin/cockpit-idle-prompt.rs` | Optional GTK idle prompt; GTK/layer-shell and external idle CSS |
| `apps/osd/src/bin/osd.rs` | Session registry/desktop daemon; GTK/layer-shell, report socket, external CSS/icons |

## Complete tracked configuration inventory

The source column always names an individual tracked path. Destination and activation come
from its class rule above. Runtime prerequisites are observed commands/interfaces, not a
completed Arch `depends` declaration. Existing executable modes must be preserved; a shebang
does not make a 0644 Python helper executable when its caller invokes `python3` explicitly.

### Commands and adjacent support: 54 paths

| Tracked source | Class | Consumer / prerequisites / unresolved behavior |
| --- | --- | --- |
| `config/bin/.local/bin/agent-build` | R | Launcher-installed build shims; Python stdlib, Linux `/proc`, scheduler and game-guard gate; required even when Cockpit itself is prebuilt |
| `config/bin/.local/bin/agent-docker-build` | R | Sandbox Docker helper; Python stdlib, Git, Docker CLI and project-scoped daemon; no global daemon fallback |
| `config/bin/.local/bin/ask-to-guide` | R | Wrapper for same-name Rust executable; POSIX shell and readlink |
| `config/bin/.local/bin/ask-to-run` | R | Wrapper for same-name Rust executable; POSIX shell and readlink |
| `config/bin/.local/bin/claude-audit` | R | Wrapper for same-name Rust executable; POSIX shell and readlink |
| `config/bin/.local/bin/claude-demo` | R | Compatibility alias to adjacent cockpit-demo |
| `config/bin/.local/bin/claude-design` | R | Compatibility alias to adjacent cockpit-design |
| `config/bin/.local/bin/claude-fade` | R | Compatibility alias to adjacent cockpit-fade |
| `config/bin/.local/bin/claude-guide` | R | Compatibility alias to adjacent cockpit-guide |
| `config/bin/.local/bin/claude-health-check` | R | Compatibility alias to cockpit-health-check; source-oriented health blocker |
| `config/bin/.local/bin/claude-idle-prompt` | R | Compatibility alias to cockpit-idle-prompt |
| `config/bin/.local/bin/claude-needs-input` | R | Compatibility alias to cockpit-needs-input |
| `config/bin/.local/bin/claude-remote` | R | Compatibility alias to cockpit-remote |
| `config/bin/.local/bin/claude-run-approve` | R | Compatibility alias to cockpit-run-approve |
| `config/bin/.local/bin/claude-session-label` | R | Compatibility alias to cockpit-session-label |
| `config/bin/.local/bin/claude-sessions` | R | Compatibility alias to adjacent cockpit shell manager |
| `config/bin/.local/bin/claude-todo` | R | Compatibility alias to cockpit-todo |
| `config/bin/.local/bin/claude-update` | R | Explicit provider updater; Bash, timeout and existing trusted Claude binary; no automatic activation |
| `config/bin/.local/bin/clipboard-image-serve` | R | Clipboard bridge service; shell, Wayland and real wl-paste; image-only policy retained |
| `config/bin/.local/bin/cockpit` | R | Shell manager; Bash, jq, tmux, core utilities, provider adapters; desktop commands for Theo profile; path/doctor/reload blockers below |
| `config/bin/.local/bin/cockpit-agents/README.md` | D | Developer adapter contract, not executable runtime input |
| `config/bin/.local/bin/cockpit-agents/claude.sh` | R | Sourced by cockpit adapter discovery; Bash, Claude and sandbox launcher |
| `config/bin/.local/bin/cockpit-agents/codex.sh` | R | Sourced by cockpit adapter discovery; Bash, Codex and sandbox launcher |
| `config/bin/.local/bin/cockpit-agents/opencode.sh` | R | Sourced by cockpit adapter discovery; Bash, OpenCode and sandbox launcher |
| `config/bin/.local/bin/cockpit-codex-hook` | R | Same-name Rust wrapper; Codex lifecycle integration |
| `config/bin/.local/bin/cockpit-demo` | R | Same-name Rust wrapper; explicit demo only |
| `config/bin/.local/bin/cockpit-design` | R | Same-name Rust wrapper |
| `config/bin/.local/bin/cockpit-fade` | R | Same-name Rust wrapper |
| `config/bin/.local/bin/cockpit-failsafe` | R | Same-name Rust wrapper; preserve resource checks |
| `config/bin/.local/bin/cockpit-game-guard` | R | Python/Linux process and scheduler controls; Hyprland window discovery; state outside package |
| `config/bin/.local/bin/cockpit-guide` | R | Same-name Rust wrapper |
| `config/bin/.local/bin/cockpit-health-check` | R | Bash/systemctl/journalctl; currently runs make test in Cockpit and dotfiles and checks a personal backup service; needs package-mode design |
| `config/bin/.local/bin/cockpit-idle-prompt` | R | Same-name GTK wrapper |
| `config/bin/.local/bin/cockpit-left` | R | tmux left-key handler; Bash and tmux; binding is external integration |
| `config/bin/.local/bin/cockpit-needs-input` | R | Bash/jq hook sender; adjacent relative Claude hook, or Codex hook CLI |
| `config/bin/.local/bin/cockpit-remote` | R | Same-name Rust wrapper; preserves boot-time PATH handling |
| `config/bin/.local/bin/cockpit-run-approve` | R | Same-name Rust wrapper; host approval view |
| `config/bin/.local/bin/cockpit-session-doing` | R | Bash/jq hook sender; adjacent relative Claude hook, or Codex hook CLI |
| `config/bin/.local/bin/cockpit-session-label` | R | Bash/jq hook sender; adjacent relative Claude hook, or Codex hook CLI |
| `config/bin/.local/bin/cockpit-sidebar` | R | Same-name Rust wrapper |
| `config/bin/.local/bin/cockpit-subscription` | R | Same-name Rust wrapper; preserve existing implementation/environment contract |
| `config/bin/.local/bin/cockpit-team` | R | Same-name Rust wrapper; collaboration CLI |
| `config/bin/.local/bin/cockpit-todo` | R | Same-name Rust wrapper; Todo CLI |
| `config/bin/.local/bin/craft-open` | R | Optional explicit FigCraft host launch; Bash, hyprctl, private figcraft.sh, existing approved app; no launch during staging |
| `config/bin/.local/bin/greyproxy-prune` | R | Explicit history-retention tool; Python/sqlite; mutable proxy state stays user-owned |
| `config/bin/.local/bin/greywall-agents/claude` | R | Sandboxed Claude launcher; shell, exact Greywall fork, existing Claude binary and optional keyring; registration separately reviewed |
| `config/bin/.local/bin/greywall-agents/codex` | R | Sandboxed Codex launcher; shell, Node bridge, exact Greywall fork; hard-coded bridge checkout path |
| `config/bin/.local/bin/greywall-agents/figcraft.sh` | R | Shared FigCraft reservation helper; Bash/jq/process identity; private code, no app bundled |
| `config/bin/.local/bin/greywall-agents/launcher-common.sh` | R | Shared launcher setup; Git, dig, secret-tool, exact sandbox stack and helpers; copies home-local shims, so relocation is security-sensitive |
| `config/bin/.local/bin/greywall-agents/opencode` | R | Sandboxed OpenCode launcher; shell, exact Greywall fork and trusted existing OpenCode; optional keyring |
| `config/bin/.local/bin/opencode-update` | R | Explicit trusted provider updater; Bash/curl/jq and existing OpenCode; do not auto-run |
| `config/bin/.local/bin/osd` | R | Same-name GTK/registry Rust wrapper |
| `config/bin/.local/bin/osd-show` | R | Host FIFO sender; Bash; host FIFO remains hidden from sandboxes |
| `config/bin/.local/share/agent-shims/bin/wl-paste` | R | Image-only sandbox shim; shell/socat and bridge socket; keep private, never replace host clipboard tool |

### Provider hooks and integrations: 12 paths

| Tracked source | Class | Consumer / prerequisites / unresolved behavior |
| --- | --- | --- |
| `config/claude/.claude/hooks/agent-environment.sh` | R | Claude SessionStart environment report; Bash/jq/Git; personal projects/repository paths need review |
| `config/claude/.claude/hooks/billing-capture.py` | R | Session hook's adjacent billing recorder; Python stdlib; user billing state outside payload |
| `config/claude/.claude/hooks/session-state.sh` | R | Claude and OpenCode reporting plus metadata CLIs; Bash/jq/socat and daemon transport; no hook settings overwritten |
| `config/claude/.claude/statusline-command.sh` | R | Claude status registration; Bash/jq and user-owned status cache; reviewed registration required |
| `config/codex/.codex/cockpit-bridge.mjs` | R | Codex subscription routing bridge; Node stdlib and existing provider endpoints/state; launcher path blocker |
| `config/codex/.codex/hooks.json` | O | Codex lifecycle registration; merge existing hooks, do not replace user configuration |
| `config/opencode/.config/opencode/lib/subscription-routing.ts` | R | Imported relative module for cockpit plugin; supported OpenCode runtime/API |
| `config/opencode/.config/opencode/plugins/clipboard-image/index.ts` | R | Optional OpenCode clipboard extension registration/module; preserve sibling tui module |
| `config/opencode/.config/opencode/plugins/clipboard-image/tui.ts` | R | Optional OpenCode image-only clipboard client; Node APIs/bridge socket; keep adjacent index |
| `config/opencode/.config/opencode/plugins/cockpit-telemetry/index.ts` | R | Optional OpenCode telemetry extension module; paired tui module |
| `config/opencode/.config/opencode/plugins/cockpit-telemetry/tui.ts` | R | Optional OpenCode UI reporter; child-process API and reporting commands |
| `config/opencode/.config/opencode/plugins/cockpit.ts` | R | OpenCode lifecycle plugin; relative subscription module and fixed home-checkout hook path; package registration/path blocker |

### Desktop profiles and security policy: 9 paths

| Tracked source | Class | Consumer / prerequisites / unresolved behavior |
| --- | --- | --- |
| `config/fish/.config/fish/functions/claude.fish` | O | Optional Fish session entry point; existing provider/launcher and cockpit; merge without replacing user's function |
| `config/fish/.config/fish/functions/opencode.fish` | O | Optional Fish session entry point; existing provider/launcher and cockpit |
| `config/greywall/.config/greywall/greywall.json` | S | Default-deny policy names checkout binaries, `/run/user/1000` and private paths; cannot install unchanged as generic policy |
| `config/greywall/.config/greywall/learned/claude.json` | S | Existing reviewed user grants; excluded from universal payload, not regenerated or broadened |
| `config/greywall/.config/greywall/learned/codex.json` | S | Existing reviewed user grants and Codex hook paths; excluded from universal payload |
| `config/greywall/.config/greywall/learned/opencode.json` | S | Existing reviewed user grants; excluded from universal payload |
| `config/hyprland/.config/hypr/apps/craft.lua` | O | Optional FigCraft window/workspace rules; Hyprland Lua integration and existing compositor setup |
| `config/hyprland/.config/hypr/figcraft-review.lua` | O | Optional approved design-review float; Hyprland Lua integration; no window action during install |
| `config/kitty/.config/kitty/cockpit.conf` | O | Optional Kitty overlay, not a complete Kitty config; cockpit also assumes external user kitty.conf/tmux bindings |

### User service templates and enablement intent: 27 paths

O means an inert template under I, not a ready unit installed into systemd's search path.
All service paths/compatibility unit names need package-mode review before activation.

| Tracked source | Class | Consumer / prerequisites / unresolved behavior |
| --- | --- | --- |
| `config/systemd/.config/systemd/user/claude-health-check.service` | O | Compatibility health service; home-local command and source health blocker |
| `config/systemd/.config/systemd/user/claude-health-check.timer` | O | Weekly legacy timer; avoid duplicate activation with canonical health timer |
| `config/systemd/.config/systemd/user/claude-remote.service` | O | Compatibility phone service; home-local alias and existing pairing |
| `config/systemd/.config/systemd/user/claude-update.service` | O | Explicit provider updater service; executes both provider update scripts; opt-in only |
| `config/systemd/.config/systemd/user/claude-update.timer` | O | Periodic provider update policy; not enabled on package installation |
| `config/systemd/.config/systemd/user/clipboard-image-bridge.service` | O | socat image-only socket service; private runtime directory and home-local helper |
| `config/systemd/.config/systemd/user/cockpit-failsafe.service` | O | Failsafe check; home-local binary and existing user/resource policy |
| `config/systemd/.config/systemd/user/cockpit-failsafe.timer` | O | Existing resource-check cadence; preserve intent only at reviewed activation |
| `config/systemd/.config/systemd/user/cockpit-game-guard.service` | O | Game guard run/restore; Linux/Hyprland and home-local script; do not silently disable on migration |
| `config/systemd/.config/systemd/user/cockpit-health-check.service` | O | Canonical health service; source health blocker |
| `config/systemd/.config/systemd/user/cockpit-health-check.timer` | O | Canonical weekly health timer; enabled intent is user-owned |
| `config/systemd/.config/systemd/user/cockpit-remote.service` | O | Canonical phone server; home-local command, no new pairing or network publication |
| `config/systemd/.config/systemd/user/cockpit.service` | O | Manager prepare service; home-local shell command and tmux |
| `config/systemd/.config/systemd/user/default.target.wants/clipboard-image-bridge.service` | E | Symlink `../clipboard-image-bridge.service`; preserve user intent, no package-created enablement |
| `config/systemd/.config/systemd/user/default.target.wants/cockpit.service` | E | Symlink `../cockpit.service`; no automatic manager start |
| `config/systemd/.config/systemd/user/default.target.wants/greyproxy.service` | E | Symlink `../greyproxy.service`; no automatic proxy start |
| `config/systemd/.config/systemd/user/default.target.wants/osd.service` | E | Symlink `../osd.service`; no automatic daemon restart |
| `config/systemd/.config/systemd/user/greyproxy-prune.service` | O | Existing proxy retention policy; Python helper/home path and live database; never run during staging |
| `config/systemd/.config/systemd/user/greyproxy-prune.timer` | O | Hourly user retention schedule; opt-in activation |
| `config/systemd/.config/systemd/user/greyproxy.service` | O | Proxy template has absolute `/home/theo/.local/bin/greyproxy`; must not activate unchanged |
| `config/systemd/.config/systemd/user/osd-report.socket` | O | Authenticated report socket; `%t/cockpit-report/osd-report.sock`, mode 0600; preserve lifetime/ownership |
| `config/systemd/.config/systemd/user/osd.service` | O | GTK/registry service; home-local binary and inherited report socket |
| `config/systemd/.config/systemd/user/sockets.target.wants/osd-report.socket` | E | Symlink `../osd-report.socket`; socket activation intent stays user-owned |
| `config/systemd/.config/systemd/user/timers.target.wants/claude-health-check.timer` | E | Symlink `../claude-health-check.timer`; no duplicate timer enablement |
| `config/systemd/.config/systemd/user/timers.target.wants/claude-update.timer` | E | Symlink `../claude-update.timer`; no silent provider update policy |
| `config/systemd/.config/systemd/user/timers.target.wants/cockpit-failsafe.timer` | E | Symlink `../cockpit-failsafe.timer`; do not discard existing protection when migrating |
| `config/systemd/.config/systemd/user/timers.target.wants/greyproxy-prune.timer` | E | Symlink `../greyproxy-prune.timer`; no new retention policy |

### Collectors versus Waybar presentation: 9 paths

| Tracked source | Class | Consumer / prerequisites / unresolved behavior |
| --- | --- | --- |
| `config/waybar/.config/waybar/scripts/claude-usage.sh` | R | Core usage collector, also Waybar output; Bash/curl/jq and existing OAuth credentials; core paths.rs names home Waybar path |
| `config/waybar/.config/waybar/scripts/cockpit.sh` | O | Waybar session chip; shell/jq and daemon snapshot; not the terminal manager executable |
| `config/waybar/.config/waybar/scripts/codex-quota.py` | R | Shared Codex protocol/credential helper; Python stdlib and trusted Codex; sibling imports require co-location |
| `config/waybar/.config/waybar/scripts/codex-session.py` | R | Core session history helper; Python/runpy and adjacent codex-quota.py; paths.rs names home Waybar path |
| `config/waybar/.config/waybar/scripts/codex-usage.sh` | R | Core Codex quota collection, also presentation; Bash/jq/Python and adjacent codex-quota.py |
| `config/waybar/.config/waybar/scripts/native-subscription.py` | R | Core native-provider auth/refresh helper; Python/curl, adjacent codex-quota.py and existing providers; auth state stays outside package |
| `config/waybar/.config/waybar/scripts/on-claude-workspace.sh` | O | Desktop profile and cockpit workspace checks; Bash/hyprctl/jq; cockpit currently names home Waybar path |
| `config/waybar/.config/waybar/scripts/waybar-visibility.sh` | O | Desktop profile bar visibility; Bash, preceding helper and existing Waybar; manager currently names home path |
| `config/waybar/.config/waybar/scripts/workspace-listener.sh` | O | Desktop profile compositor listener; shell/socat and preceding visibility helper; no listener autostart during staging |

## Additional runtime edges outside config

| Tracked source / artifact | Classification / proposed destination | Consumer / gate |
| --- | --- | --- |
| `apps/cockpit/assets/remote/page.html` | Embedded at Rust compile time; no independent runtime copy | cockpit-remote includes it; verify exact release artifact |
| `apps/cockpit/assets/remote/page.js` | Embedded at Rust compile time; no independent runtime copy | cockpit-remote includes it |
| `apps/cockpit/assets/remote/sw.js` | Embedded at Rust compile time; no independent runtime copy | cockpit-remote service worker |
| `apps/cockpit/assets/remote/manifest.webmanifest` | Embedded at Rust compile time; no independent runtime copy | cockpit-remote manifest |
| `design/tokens.cockpit-ui.css` | Embedded at Rust compile time; no independent runtime copy | cockpit-remote includes CSS tokens |
| `design/tokens.cockpit-ui.json` | D; no runtime copy identified | Maintained design source, not a reason to delete it |
| `design/tokens.css` | D; no independent runtime copy identified | Maintained design source |
| `design/tokens.json` | D; no runtime copy identified | Maintained design source |
| `assets/claude-cockpit.png` | D; no runtime copy | Documentation screenshot |
| `assets/claude-cockpit-todo.png` | D; no runtime copy | Documentation screenshot |
| `assets/claude-guide.png` | D; no runtime copy | Documentation screenshot |
| `tools/migrate-cockpit-names.py` | Required compatibility edge; destination unresolved | cockpit reload currently resolves it relative to checkout; separate activation/migration review, never execute at staging |
| `tools/agent-docker.sh` | Optional host management capability; destination unresolved | Existing make agent-docker commands; rootless daemon setup/grants/units require host approval, not automatic install hooks |
| `tools/agent-forward.sh` | Optional host management capability; destination unresolved | Existing make agent-forward interface; no network-policy changes during packaging |
| `tools/agent-github.sh` | Optional host management capability; destination unresolved | Existing make GitHub-token/policy interface; credentials/repository settings are not package data |
| `tools/tailnet-dashboard-grant.sh` | Optional host management capability; destination unresolved | Existing dashboard/tailnet grant interface; no service publication implied |
| `tools/handoff-install.sh` | Source-oriented host maintenance; destination unresolved | Existing make handoff-install; host review required; not a package activation substitute |
| `tools/system-install.sh` | Source-oriented host maintenance; exclude from runtime | Source system-tree installer; not a package lifecycle hook |
| `tools/capture-claude.sh` | D; exclude from runtime | Development/manual capture utility, not a runtime caller identified here |
| `tools/full-test-lock.py` | D; exclude from runtime | Source-suite serialization; keep local development locking unchanged |
| `tools/make-help.awk` | D; exclude from runtime | Make help renderer; no runtime dependency identified |
| `tools/quality.py` | D; exclude from runtime | Development quality gate |
| `system/etc/systemd/system/agent-docker@.service` | S; no default activation | Rootless project daemon provisioning; privileged setup and user/path templates require separate review |
| `system/etc/systemd/system/agent-docker-relay@.service` | S; no default activation | Project socket relay; do not broaden sandbox access |
| `system/etc/systemd/system/agent-docker-clones-relay@.service` | S; no default activation | Parallel-clone relay; preserve project identity |

`tools/tests/**`, `apps/**/tests/**`, Rust inline test modules, all remaining Rust library
source, manifests, lockfiles, build scripts, CI files and design-plan archives/previews are
development/documentation inputs, not installed payloads. They remain maintained in Git.
No development directory is copied to satisfy a runtime dependency by accident.

## Component composition: preserve exact identities

| Tracked source | Inventory decision | Existing identity / consumer |
| --- | --- | --- |
| `packages/greywall/build.sh` | D; never run on workstation package install | Fork source `7c06d51afd26cbc90a32c82375a393570c22ffc7`; source archive SHA256 `4fb512bcd02515af39d273953aabcf60c704827afb491a6f987ea6b8170067a1` |
| `packages/greywall/check.sh` | Required runtime compatibility logic; port from build-script parsing | Reads greywall --version; no approved package-mode metadata yet |
| `packages/greyproxy/build.sh` | D; never run on workstation package install | Fork source `cdd5cde76dce1262c0fd8d6b1ab2bd9953df5e71`; source archive SHA256 `6e1932582a097cc665917b3177ff33dfe6f6eaef2f32047a05655082f63fd448`; advertised version 0.4.5 |
| `packages/greyproxy/check.sh` | Required runtime compatibility logic; port from build-script parsing | Reads greyproxy --version; no upstream replacement permitted |
| `packages/greyproxy/restart.sh` | Host activation utility, no staging execution | Live proxy restart/session compatibility needs review |
| `packages/greyproxy/settings.sh` | Mutating host activation utility; destination unresolved | Reads settings, then sends PUT to disable conversation tracking when enabled; execution requires reviewed activation, never a staging/preflight check |

Greywall's build also pins tun2socks **2.5.2**, Linux amd64 ZIP SHA256
`4aa7737009a9f06b9f4957c4fc12932ae0cd2039471d2ae4e5d4666ffeb40a2e`, embedded during its
build. Source/archive identities are not hashes of future executable release artifacts.
No approved binary download location, binary checksum, component package name or
redistribution license inventory was established here. Issue 6 must settle composition.

## Runtime dependencies versus build dependencies

Observed runtime commands/interfaces, not invented package names:

| Capability | Runtime requirements observed in source | Boundary |
| --- | --- | --- |
| Terminal/session management | Bash/POSIX shell, tmux, jq, Linux core/process utilities, Python 3 | No Rust/Go compiler required to run prebuilt Cockpit |
| Launching development sessions | Git, dig, exact Greywall/Greyproxy forks, Linux namespaces/process identity, runtime shims | Shared launcher always runs dig for proxy DNS readiness; its absence causes errors and a roughly 30-second startup wait; Git is not exclusively a build dependency |
| Provider integration | Selected trusted Claude/Codex/OpenCode installation; Node for Codex bridge and supported plugin runtime; Python collectors; curl/jq | Credentials and provider accounts not shipped; optional integrations must remain visibly optional |
| Host approval/guides | Kitty/kitten, tmux, existing daemon/report transport | Do not replace authenticated host-side approval with a package script |
| Theo desktop | Hyprland/hyprctl, GTK4/layer-shell, Wayland, Waybar profile helpers and user terminal/tmux setup | This is a desktop profile, not generic Linux portability |
| Clipboard images | Host wl-paste, socat, restricted image bridge and sandbox shim | Never grant clipboard text to sandboxes |
| Optional repository actions | gh, Git and lazygit where that command is selected | Package operation itself must not require a source checkout or Git to determine installed identity |
| Optional project Docker integration | Docker CLI, project daemon/service provisioning, agent-docker-build | Never use the user's main Docker daemon as a fallback |
| Other explicit host features | systemctl/journalctl/systemd-cat, secret-tool, sqlite3, gzip, notify-send where called | Map feature-specific dependencies before declaring mandatory Arch dependencies |
| Source development / package production only | Cargo/Rust, Go, nextest, lint tools, pkg-config, compiler headers; make/Stow for current source install | Do not invoke source install or rebuild components from a package hook |

The earlier roadmap's blanket “no Git installation requirement” conflicts with existing
runtime Git calls. This inventory does not remove those calls or silently rewrite that
acceptance criterion. Distinguish **no checkout/compiler for installing Cockpit** from
**Git available when launching/managing Git development projects**. Settle the intended
feature contract before promising a Git-free full runtime.

Read-only `readelf -d` on the local existing binaries observed:

- `osd`: `libglib-2.0.so.0`, `libgtk4-layer-shell.so.0`, `libgtk-4.so.1`,
  `libgio-2.0.so.0`, `libgobject-2.0.so.0`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`,
  `ld-linux-x86-64.so.2`.
- `cockpit-sidebar`: `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`, `ld-linux-x86-64.so.2`.

These observations are not an ABI floor or verification of a release binary. Inspect every
final ELF, required symbol versions and transitive native dependencies in the actual Arch
build environment. Ubuntu-built CI executables are not automatically suitable for Arch.
Cargo's bundled SQLite dependency does not eliminate the separately observed sqlite3 CLI.

Arch package ownership lookup was attempted, not verified: `pacman -Qo /usr/bin/bash`
exited **255**, “failed to initialize alpm library” with `/var/lib/pacman/` unreadable or
absent in this sandbox. Reported as Cockpit blocker `f1f58829`; no access workaround.
Exact Arch package names and supported dependency versions remain a native-environment gate.

## Missing external resources and personal assumptions

| Consumer | Existing resource/reference | Required package work |
| --- | --- | --- |
| `apps/osd/src/bin/osd.rs` | `~/.config/osd/style.css`, `~/.config/waybar/icons` | Not tracked in this checkout; obtain reviewed redistributable assets or diagnose absence; no copying from another project's home |
| `apps/osd/src/bin/cockpit-idle-prompt.rs` | `~/.config/hypr/scripts/claude-idle.css` | Same external-resource gate; GTK startup alone is not visual compatibility |
| `config/bin/.local/bin/cockpit` | user Kitty config, cockpit Kitty overlay, tmux options/bindings/fingerprint | Keep optional user files; distinguish required manager behavior from profile styling |
| OpenCode cockpit plugin | `~/agent-cockpit/config/claude/.claude/hooks/session-state.sh` | Use reviewed installed hook resolution and preserve reporting identity |
| Sandbox policy | checkout paths, user id 1000 runtime paths, learned grants | Dedicated security-sensitive path migration; do not grant all of P by default |
| Launcher common | home-local command/helper copies, per-project Git, keyring and Docker paths | Preserve identity and fail-closed checks while resolving installed resources |
| Health/failsafe/source actions | source tests/backup checks, `IdeaProjects/agent-cockpit`, make/apply tooling | Separate runtime health from developer maintenance; do not fake a Git checkout in the package |

## Package-layout blockers and required entry-point regressions

| Blocker / owner | Regression the next implementation must prove |
| --- | --- |
| Public symlinks / POSIX wrappers | Stage a path containing spaces; invoke public wrapper through symlink with a fake same-name binary; arguments and wrapper behavior survive; no checkout fallback |
| `cockpit` reload/doctor and core helper paths | In isolated home with no `~/agent-cockpit` or home Waybar scripts, resolve package helpers without make/Git for installed-version identity; existing checkout mode remains unchanged |
| Optional provider/desktop integration | With no provider/profile registration, report unavailable capability; never replace user hook settings or create enabled-unit links implicitly |
| Collectors under Waybar | Run real collector entry points with fake providers/network and no Waybar installation; keep codex-quota.py siblings together and preserve credential boundaries |
| Policy/service templates | Stage only; verify no learned grants, credentials or wants links copied; approved path migration preserves denial/authentication and protects host-only commands |
| Component checks | Correct fork/revision accepted; wrong/missing/unidentified component rejected without download/build/install/restart; expected identity comes from reviewed artifact composition |
| Native Arch package | Fresh isolated native-layout startup with no source checkout/compiler and no access to live services/user state; inspect contents and all native dependencies |

Current CI is also a release gate: PR-head run **41047226643** reached nextest but ended
with **156 passed, 3 failed, 0 skipped, 3,110 not run** due to fail-fast. The failures were
the model-preference tests reporting an empty OpenCode catalog; root cause is not established
here. Successful installer and admission setup are not a successful quality gate.

## Smallest next implementation slice

Start with non-security resource resolution for the five required collectors and their
real core callers (`paths.rs`, `usage.rs`, `subscription_fallback/native.rs`). Preserve the
existing checkout/user override behavior; do not add a general service/config abstraction.
Test the installed layout using fake commands and isolated state before changing a launcher
or policy. That is a bounded follow-up plan, not authorization to edit those Rust files now.

After that, separately settle launcher/policy migration, component composition and service
activation before assembling a complete package. License choice, exact-source green CI,
native smoke tests and approved artifact identities remain publication gates. AUR package
name `agent-cockpit-bin` is still a proposal, not a published install command.
