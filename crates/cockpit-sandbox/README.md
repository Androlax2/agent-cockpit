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
2. `Greywall::detect` checks the installed engine: the fork commits the release pins
   (`PINNED_GREYWALL_COMMIT`, `PINNED_GREYPROXY_COMMIT`) and the host capabilities Greywall
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
`cockpit-turso` (the evidence table). `cockpit-sessions` calls it for every launch; nothing
else invokes Greywall or greyproxy.
