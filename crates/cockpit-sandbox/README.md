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
upstream installer disabled. Greyproxy startup, configuration and health are #495;
installation readiness alone does not prove enforcement (#433).

To test an unreleased local fork, a **debug build in profile `dev` only** accepts both
`COCKPIT_DEV_GREYWALL` and `COCKPIT_DEV_GREYPROXY` as absolute paths. This explicitly bypasses
commit pins and logs a warning; release builds and other profiles refuse the override.

`make sandbox-install-smoke` verifies the real published assets through installation and
doctor in a temporary home, with fake providers and service managers. The existing Linux
`greenfield-ci` gate includes it. It does not start a proxy service or prove live sandbox
enforcement. Greyproxy `v0.4.5-cockpit.1` lacks fork PR #8; concurrent-session acceptance
requires the separately approved corrected release before #433 can claim it.
