# Launch-bound owner measurements and isolated enforcement checks

Status: partly superseded by 2026-11-05-433-live-enforcement-ci.md: the observation bounds and storage hold; how enforcement is measured is replaced

Issue #433, implementing report 11's distinction between requested, effective and observed
isolation. The lane brief supersedes the issue's stale source-build route: real checks
consume `engine-pins.json` through the managed release installer.

- `cockpit-sandbox` owns validation and storage of one immutable observation batch per
  launch. Existing `sandbox_launch_lines` hold the appended `owner probe:` lines; no schema
  or crate dependency changes. The caller retains transaction ownership.
- The daemon's existing owner-only session boundary accepts `SessionSandboxRecord`;
  `cockpit-sessions` serializes it with lifecycle operations and binds it to the latest
  sandbox launch's unique proxy client identity. An identical retry is idempotent; a
  different batch, a stale identity or an unsandboxed launch is refused. A busy lifecycle
  returns its existing pre-mutation `Busy` refusal. No agent grant or policy changes.
- Reports are owner attestations, not automatic proof for ordinary sessions. Bounds are
  16 unique nonblank single-line observations, 512 UTF-8 bytes each and a 256-byte client
  identity. Control characters are refused before storage. The CLI displays the provenance.
- The live runner uses a fake provider and fixture files in a temporary home outside
  `/tmp`, where the pinned Linux engine can enforce its path rules. A short private runtime
  directory keeps Unix socket names valid. All supported provider names are masked on PATH;
  no provider sign-in, credentials or billable model calls are used.
- The disposable owner profile uses the existing `agents.desktop_session = "withheld"`
  setting: the manager's bus is retained for host health commands, never delegated to the
  headless probe. Linux commands set `SYSTEMCTL_FORCE_BUS=1` to use the explicit session-bus
  endpoint while Cockpit's runtime directory remains private. A temporary-socket regression
  checks endpoint selection without contacting a real user manager.
- Four sequential launches measure an allowed checkout file, unreadable fixture SSH key,
  daemon database, proxy key/token, and existing-profile `api.github.com:443` versus
  non-allowlisted `example.com:443`. Requests are bounded unauthenticated HEADs. No extra
  destination, path, client scope or network policy is granted.
- Linux transparent routing can convert a SOCKS refusal into a transport failure; the
  pinned proxy also has a 30-second unknown-destination approval grace period. The runner
  therefore requires both successful allowed controls and the managed proxy's own exact
  client/destination allow/block decisions. A timeout alone never passes. The fixture token
  stays on the host, is sent only to the owned loopback API, and is never logged or passed
  in argv. Redirects are refused. Only selected decision fields become artifacts.
- Cleanup addresses only the owned session, profile service, daemon process and private
  tmux socket, including partial setup failures. A separate CI-only target keeps live
  services and networking out of local full suites; local verification uses fake commands.

CI wiring is a proposal until its separately reviewed host push is approved. No install,
host setting, workflow push, tag or release is authorized by this document. #397 coordinates
shared CI changes; release.yml and release automation remain with the release lane.

Theo's 2026-11-02 scope decision makes Alpha 1 Linux+WSL2 only. Greyproxy's published pin
lacks fork PR #8, which remains a Linux/WSL2 prerequisite; publication and pin consumption
require separate approval. Fixture checks do not close live enforcement, #495, #61 or
concurrent-session acceptance. Source delivery can finish after verified merge while
the PR uses `Part of #433` and actual Linux/WSL2 measurements remain open.

macOS bootstrap/enforcement and #527's public-CA isolation are deferred under #568,
not Alpha 1 blockers. Preserve known failures, fail-closed capability refusal and ordinary
required macOS development checks. Ordinary CI executes fixtures only; live execution
remains separately approved. Deferral grants no publication, pin or live dispatch authority.
