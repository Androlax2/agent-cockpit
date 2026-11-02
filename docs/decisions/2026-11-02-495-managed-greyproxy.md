# Own Greyproxy configuration and its user service

Issue #495. Implemented with isolated fixture evidence; real platform lifecycle/network
acceptance remains open. Theo approved the fixed-port option on 2026-11-02.

- `cockpit-sandbox` owns configuration, service commands and bounded proxy health checks.
  The existing CLI-to-sandbox edge is sufficient; no dependency or manifest changes.
- `cockpit sandbox install` remains the single setup path: verified release installation,
  then configuration and startup of a systemd user service (Linux/WSL2) or launchd agent
  (macOS). `--engines-only` supports preparing binaries without service effects.
- State and the API token belong to `<profile-state>/greyproxy`, never the legacy data
  directory. Units have a distinct Greyproxy name and ownership receipt, so daemon
  installation discovery does not mistake them for a Cockpit daemon.
- Initialize the profile's CA through the verified engine's bounded, local-only
  `cert generate`, with explicit HOME and a private staging data directory. Commit the
  pair with the other staged files, preserve an existing pair, and refuse incomplete or
  unsafe state. Never install a host trust anchor. Explicit handler paths prevent an
  absent CA silently selecting TLS passthrough. Health requires the protected pair.
- Use the pinned engine's loopback ports: API 43080, HTTP 43051, SOCKS 43052 and DNS 43053
  (TCP/UDP). An occupied endpoint is refused without stopping or adopting its process.
  Only one profile can own these ports in a host network namespace. Multiple agent
  sessions within that profile remain supported; their concurrency proof is separate.
- Keep management API authentication and per-launch client secrets required, with no
  anonymous clients or exempt names. Do not add destination grants, credentials or
  middleware. Conversation capture and desktop notifications are disabled in managed
  configuration; the daemon does not own either product surface.
- Doctor reports the installed engine and running proxy separately. A public HTTP 200
  alone is insufficient: verify the owned unit/configuration, running service, private
  token, process ownership of all configured TCP/UDP sockets, expected health fields and authenticated API
  response. Linux/WSL2 inspect the owned PID's executable, arguments, environment and socket
  inodes; macOS checks launchd's loaded arguments/environment and its PID's listener with
  system `lsof`. Namespace TCP/UDP tables are streamed with a line limit and deadline;
  only relevant listener rows count against the retained output bound. An IPv6 management listener is refused because pinned Greywall registers
  at `localhost` while managed services bind IPv4. Bound commands, output
  and HTTP requests. Required-sandbox launches check health on blocking workers before
  reserving a launch; dry runs can still describe the requested sandbox.
- `sandbox restart` and `sandbox stop` operate only on the selected profile's owned
  service. Installation preserves a healthy running service. Agents cannot read or
  overwrite the proxy state or its unit, including through a writable ancestor. The receipt
  identifies a previous immutable pair only inside this profile's protected engine root;
  a pin change requires an explicit restart, which verifies the new binary before stopping
  the old owned service. It never executes an earlier binary to perform the upgrade.
  Lifecycle actions may retry a failed systemd reload only after checking the owned
  fragment and absence of drop-ins; readiness remains read-only and refuses stale units.
- The pinned macOS backend cannot re-expose the public CA beneath the required data-dir
  denial. Keep private-state isolation and refuse doctor/launch readiness with typed
  `proxy-engine-capability` unless the verified engine reports `public CA isolation`.
  This is a required future capability, absent from the current pin; fake engines declare
  it only for wiring tests, with a separate negative real-entry-point test. The reviewed
  upstream fix and approved release are tracked in [#527](https://github.com/Androlax2/agent-cockpit/issues/527).
- Fixtures use temporary homes, fake service/HTTP commands and ephemeral loopback ports.
  Real service and network enforcement evidence belongs in isolated CI. The running
  legacy proxy is never a validation target. Fork PR #8's corrected published release
  remains a separate prerequisite for concurrent-session acceptance under #61/#433.

Evidence: sandbox `proxy::tests` cover service overrides, process/socket ownership, IPv6
collisions, authentication, partial setup retry, command limits and startup/shutdown races.
The actual daemon/CLI tests `doctor_refreshes_proxy_health_separately_from_verified_engine_builds`
and `an_unhealthy_managed_proxy_refuses_a_launch_before_any_agent_runs` prove refreshed
readiness and pre-launch refusal with fake host commands. `a_launch_runs_the_agent_inside_greywall_as_its_own_proxy_client`
checks the managed data path, explicit traffic endpoints and protected paths. These are
fixture observations, not live enforcement measurements. This implementation PR is
`Part of #495`; keep #495 and #61 open until isolated CI measures their remaining acceptance.

Review provenance: `$code-review medium` was invoked from
`/home/theo/.codex/skills/code-review/SKILL.md` on the working delta based on `2553ebff`.
Its ten findings required fixes to ownership, launch/readiness wiring, failure cleanup and
bounded execution. The eight-angle inline workflow is recorded separately from the fresh
independent full-delta review required for the PR; earlier #470 reviews remain fresh-agent
reviews and are not relabelled as this skill.
The second native pass after substantial fixes found five lifecycle defects and two local
cleanups (working-delta SHA256 `c422d2929039792daa0edc535166ef59e8b7d9e7ed7c4909889bba818d698e23`);
all were addressed. A separate independent full-delta review of SHA256
`6d275e2263c9968585d2176233ca40edaff809714e8e5a1ba74cda372322194c` plus the typos vocabulary
exception found CA initialization/access, namespace-table bounds, reload recovery and the
CLI usage fixture. Those required substantive fixes; their final review/check evidence
belongs in the implementation PR. The macOS release requirement remains open, explicitly
fail-closed, rather than being represented as proven platform enforcement.
