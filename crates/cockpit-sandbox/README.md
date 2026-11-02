# cockpit-sandbox

Owns the sandbox every launched agent runs in: the profile Cockpit requests, the platform
capabilities that decide whether it can be enforced, the Greywall/greyproxy backend that
enforces it and the evidence of what each launch got.

Decision (#61, option C): Cockpit owns the policy, the engine enforces it.

1. `Profile::build` turns what a launch asks for (working directory, the session's own
   directory, read-only mounts the daemon needs the agent to reach, Cockpit's own state it
   must never reach, extra hosts, environment, secrets, the owner's desktop session choice)
   into the effective profile, with Cockpit's baseline rules added. Every agent writes its
   session's directory (#437); a Codex agent finds a folder of the session's own, kept in
   the daemon's mount sources outside anything an agent writes, at
   `/tmp/codex-daemon-<uid>` when `greywall --help` lists `--mount`, and the plan's `reach`
   says otherwise why its sockets are unreachable, never sharing the host's folder (#446); a shared desktop session
   reaches exactly the sockets and file its variables name, and the session bus only
   through greywall's filtered proxy (#435). A rule the platform cannot enforce is a
   `Refusal` naming the rule and the reason; nothing is weakened silently.
2. `Greywall::detect_owned` checks the installed engine: the fork commits the release pins
   (the tags, commits, per-target URLs and SHA-256 values in `engine-pins.json`, with
   `PINNED_GREYWALL_COMMIT` and `PINNED_GREYPROXY_COMMIT` checked against it by a test) and the host capabilities Greywall
   reports. A missing or other build leaves the engine unready, with the reason.
3. `Greywall::plan` turns a profile into the command that runs the agent inside Greywall and
   the settings file it reads. Each launch gets its own greyproxy client identity
   (`ClientIdentity::for_launch`), so back-to-back launches never share the registration a
   per-run client secret is stored under.
4. `Evidence` says what one launch got: enforced or unsandboxed (only by the owner's
   `agents.sandbox = "off"`), the engine and platform, every rule with what enforced it, and
   the host capabilities observed. `record` stores it in the launch's transaction;
   `latest_for_session` reads it back for `cockpit session sandbox`.
   `record_observations` appends one bounded owner-measured batch for that exact launch;
   output labels it `owner probe` to distinguish an attestation from engine capabilities.

Edges: `cockpit-config` (the owner's `DesktopSession` and `AgentSandbox` values) and
`cockpit-turso` (the evidence table, behind the default `persistence` feature).
`cockpit-sessions` calls it for every launch; the CLI's offline installer uses it without
persistence. Nothing else invokes Greywall or greyproxy.

The first fork releases are greywall `v0.3.7-cockpit.1` (with `--mount`) and greyproxy
`v0.4.5-cockpit.1`, copied from the coordinator's release record on #470. The pin file
supersedes `packages/*/build.sh` for greenfield; the legacy packages remain untouched until
cutover.

## Managed installation (#470)

Run `cockpit sandbox install` before starting the daemon, selecting the same profile with
`cockpit --profile NAME sandbox install` when needed. The command needs curl for HTTPS;
it never compiles or installs a system package. It downloads the two host assets from the
embedded pin file, verifies both SHA-256 values before extraction, validates their exact
commits, and publishes the pair with one directory rename. A checksum, archive, commit or
unsupported-target failure is typed and leaves no partial installation.

The profile's state directory holds `engines/<greywall asset hash>-<greyproxy asset hash>`.
This shares the daemon's existing private, persistent layout on Linux, macOS and WSL2.
The greywall archive retains its versioned directory, LICENSE, NOTICE, third-party licence
and `greywatch -> greywall` alias; greyproxy retains its flat binary and LICENSE. Entries
are streamed with size/count bounds; other links, special entries, duplicates and paths
outside the known layout are refused. A rerun compares an existing installation with the
verified release, refusing altered files. Older pairs remain for running sessions.

Restart a daemon that was already running when engines were installed. Its launches resolve
only its own release's paths. `doctor` and `init` name the install command when absent;
`doctor` and `session sandbox` flag running sessions whose recorded engines differ, without
restarting them. Greywall receives its owned engine directories first on PATH and has its
upstream installer disabled. Installation readiness alone does not prove enforcement (#433).

## Managed proxy (#495)

`cockpit sandbox install` also configures and starts this profile's Greyproxy. Use
`--engines-only` for an offline binary installation. Linux and WSL2 need the user's
systemd manager; macOS needs the user's login launchd domain. Commands never install these
prerequisites or repair the host automatically. Health uses curl and process ownership
metadata (`readlink`, `cat`, `ls` on Linux/WSL2; macOS's `lsof`). Missing capabilities fail
closed with a reason and a next action.

Configuration, the API token and proxy data stay in `<profile-state>/greyproxy`. The
service is `cockpit-greyproxy-<profile>.service` in the user's systemd configuration or
`io.github.androlax2.cockpit.greyproxy.<profile>.plist` in `~/Library/LaunchAgents`.
Authentication and launch secrets are required; anonymous clients and exemptions are
disabled. Managed setup disables conversation capture, notifications and Docker discovery.
It generates a profile-local CA pair using the verified engine's bounded `cert generate`
command, stages both files with the configuration, and preserves an existing pair. It
never runs `cert install` or changes the OS trust store. The private key remains unreadable
inside agent sandboxes.

The pinned Greywall uses fixed management port 43080. One profile can own the loopback
ports at a time (43080 API, 43051 HTTP, 43052 SOCKS, 43053 TCP/UDP DNS). An occupied endpoint
is refused without adopting or stopping its listener. This limits simultaneous Cockpit
profiles, not the number of sessions in a profile.

`cockpit --profile NAME sandbox status` checks current health without a daemon;
`sandbox restart` restarts only the owned service; `sandbox stop` stops it. Restart or stop
interrupts that profile's agent networking. Reinstall preserves an already healthy service. When pins change, it reports an explicit
restart requirement; `sandbox restart` upgrades the previous owned service to the new pair.
`doctor` reports matching engine builds separately from proxy health and refreshes the
health on every request. Required-sandbox launches check it before execution reservation;
dry runs still show their planned profile. The launch denies reads/writes to proxy state
and denies writes to the service directory, including through a writable ancestor.
The published macOS Greywall cannot expose only the public CA through that private-state
denial. The proxy service may run, but doctor and required launches return
`proxy-engine-capability` until a verified engine reports `public CA isolation`.
[Issue #527](https://github.com/Androlax2/agent-cockpit/issues/527) tracks the upstream fix
and separately approved release. This capability is absent from today's pin; fixture
engines advertise it only to test later wiring. No private-state grant is relaxed.

Temporary-home tests replace service managers, process metadata and HTTP commands. They
prove the wiring and refusal paths; actual systemd/launchd startup and a real network probe
remain open acceptance on #495, to be measured with #433's isolated platform CI.

To test an unreleased local fork, a **debug build in profile `dev` only** accepts both
`COCKPIT_DEV_GREYWALL` and `COCKPIT_DEV_GREYPROXY` as absolute paths. This explicitly bypasses
commit pins and logs a warning; release builds and other profiles refuse the override.

`make sandbox-install-smoke` verifies the real published assets through installation and
doctor in a temporary home, with fake providers and service managers. The existing Linux
`greenfield-ci` gate includes it. It does not start a proxy service or prove live sandbox
enforcement. Greyproxy `v0.4.5-cockpit.1` lacks fork PR #8; concurrent-session acceptance
requires the separately approved corrected release before #433 can claim it.

## Measured enforcement evidence (#433)

The owner can record a measured batch with
`cockpit session sandbox-record SESSION --client CLIENT --observed TEXT...`, using the
latest launch's proxy client identity from `session sandbox`. The daemon refuses agent
callers, stale identities, unsandboxed launches and conflicting replacement reports. An
identical retry is idempotent. A batch contains 1–16 unique, nonblank observations of at
most 512 bytes each, with no control characters. Reports do not change sandbox policy,
capabilities or grants. They persist with the launch in its existing transaction domain.

`make sandbox-enforcement-fixtures` exercises the isolated runner with fake commands and
temporary Unix sockets; systemctl endpoint selection never contacts a real user manager.
The separate `make sandbox-enforcement-ci` requires explicit disposable hosted CI: it
installs the verified published engines into a temporary home, starts only that profile's
managed service and daemon, and runs one fake-provider session through four sequential
launches. Allowed checkout/network controls accompany private-path and destination denials.
The host reads only its fixture proxy's request decisions through the authenticated API;
exact launch/client/destination matching prevents a DNS/TLS failure or timeout alone from
counting as enforcement. Saved artifacts omit tokens and private file contents.

Live platform results remain unmeasured until the workflow is approved and executed. The
current macOS capability refusal (#527) and corrected Greyproxy publication remain explicit
release prerequisites; fixture success is not a Linux/macOS/WSL2 live pass.
