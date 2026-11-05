# The native sandbox's live probes run as steps of the existing Linux and WSL 2 CI jobs

Issue #433, re-scoped by #546 (proposed issue 7). Distribution decided by Theo on 2026-11-05;
the rest by the worker, because the issue settles the runners and adds no new one. Supersedes
the Greywall-era runner of [2026-11-02-433-sandbox-observations.md](2026-11-02-433-sandbox-observations.md)
for how enforcement is measured; its observation bounds and storage stand.

- **Two ignored daemon tests are the probe.** `crates/cockpit-daemon/tests/live_enforcement.rs`
  launches the fake agent under `agents.sandbox = "required"` through the real daemon. Inside,
  it cannot read the owner's `~/.ssh/id_ed25519` or the state database, `example.com` is
  refused `403` by the session proxy, and `api.anthropic.com` (the claude adapter's host) is
  reached through it. The owner's `session sandbox-record` attaches what it saw to the
  launch's identity, so `session sandbox` shows `observed: owner probe: ...`. A second test
  runs the probe on four launches in a row of one session, each a new launch identity.
- **Ignored because they reach the internet.** Local and gate runs stay offline; CI runs them
  with `--run-ignored only --no-capture`, so the evidence of each launch prints in the log,
  and `--retries 2`, so a network blip on a required check is reported flaky, not failed.
  No credential is used: the request to the allowed host is a plain unauthenticated `GET /`.
- **Steps in the jobs that already exist, no new runner or sudo.** The `greenfield` job runs
  the live tests after the gate, reusing its bubblewrap install and its existing user-namespace
  setting. `greenfield-wsl2` adds bubblewrap and tmux to its packages and runs the daemon's
  sandbox fixture suite and the live tests.
- **`greenfield-wsl2` runs Ubuntu-26.04 (Theo).** Sessions need tmux 3.6 (#523); Ubuntu-24.04
  packages 3.4. The WSL 2 startup and admission checks move to 26.04 with it.
- **Rejected:** building tmux 3.6 from source on 24.04 (a minute or two on every run), and a
  second WSL 2 job for the sandbox (doubles the job's minutes).
