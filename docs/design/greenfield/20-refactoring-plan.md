# 20. Refactoring plan

Status: proposed. Replaces `docs/superpowers/plans/2026-10-30-cockpit-stabilization-roadmap.md`
for structural work; that roadmap's stabilization items stay valid for the current code until
cutover.

## Strategy

- **Same repository, new workspace.** Root `Cargo.toml` + `crates/` grow beside `apps/`.
  `apps/` keeps running and receiving only stabilization fixes until the parity gate.
- **Parity, not translation.** Each milestone claims rows of the coverage map (report 16) and
  proves them with the acceptance scenarios (report 15), never by porting files.
- **Cutover once**, when M5 passes: `make install` switches to the new binaries; `apps/`,
  shell adapters, hooks plumbing and old units are deleted in the following release.
- One owner per milestone; reviewable units of one capability each; no big-bang PR.

## Milestones and gates

| M | Scope (capabilities) | Gate (must pass before the next starts) |
| --- | --- | --- |
| M0 | `protocol`, `client`, `daemon` skeleton, `turso`, `diagnostics`, `config`, `authorization` with grants model, crate-graph CI rules | Daemon starts on a clean home on Linux/macOS/WSL2; versioned protocol handshake; `COCKPIT_LOG_LEVEL` and support bundle work; forbidden-edge check green |
| M1 | `projects`, `processes`, `tmux`, `sessions`, `provider-api` + `cockpit-claude`, `approvals`, agent channel (17), plain TUI + CLI | Single-session parity: start/restart/sleep/exit/resume, approvals end to end, executions survive daemon restart, latency run (03) recorded |
| M2 | `tasks`, `workflows`, `verification`, `scheduling`, `resource-admission`, `messaging` change feed | Todo parity incl. queue dispatch with explanations; code-change preset with evidence-bound completion; walkthroughs 1–3 (04) green; enforced-limit report (10) |
| M3 | `cockpit-opencode`, `cockpit-codex`, `agent-context`, instruction bundle, readiness report | Provider conformance ×3; clean-home onboarding scenario (09); context budget measured (08) |
| M4 | `teams`, `chief` on the new messaging; `audit` | Team parity (link, recruit, handoff, QA fresh copy); Chief answers with habits; zero no-change wakes in replay (07) |
| M5 | `usage`, `github`, `credentials` account routing, `containers`, `sandbox` profiles, `notify`, `web` | Full coverage-map parity minus `polish`/`plugin` rows; release bundle (13) installs and upgrades on three platforms; **cutover** |
| M6 | Designed UI/UX (FigCraft, `design-first`), polish rows | Usability pass on real use; latency targets re-measured |
| M7 | Plugin runtime (14), FigCraft and desktop plugins | First external plugin runs under declared grants |

### 2026-11-01: milestones are releases

Decided by Theo on 2026-11-01 (#389). The table above keeps its text; the GitHub milestones
now carry release names, each with a checklist issue (the old gate) and a live acceptance
walk. Gates are checkpoints, not blockers. No stable release is planned until users have used
the betas for a long time ([Releasing](../../RELEASING.md#channels-and-what-each-promises)).

| Old | Release milestone | Checklist |
| --- | --- | --- |
| M0, M1 | Closed under their old names | #27, #37 |
| M3 | Alpha 1: first public pre-release, opens the alpha channel | #52 |
| M2 | Alpha 2: tasks, scheduling, verification, workflows | #46 |
| M4 | Beta 1: upgrades keep data, Teams and Chief; opens the beta channel | #57 |
| M5 | Beta 2: cutover | #66 |
| M6 | Beta 3: designed UI and the whole-codebase audits | #69 |
| M7 | Plugins, then Desktop app and Native phone apps (research) | #73 (Plugins) |

`Hardening` holds follow-ups no release needs.

## Order inside a milestone

1. Write the capability's one-sentence ownership and dependency edges; add the CI rule.
2. Types, state machine and SQL with real-database tests; then the daemon wiring; then CLI
   verbs; then TUI views. Entry-point tests drive the CLI/protocol, not internals.
3. Restart, stale-result and duplicate-delivery fixtures before the milestone gate.
4. Update the coverage map rows to `done` with the proving test named.

## Risks and how the plan answers them

| Risk | Answer |
| --- | --- |
| Long parallel life of `apps/` and `crates/` | Stabilization-only policy on `apps/`; parity gates measured against the map, not a date |
| Turso pre-1.0 | M0 includes backup/restore and crash-recovery fixtures; file stays plain SQLite; fallback to `rusqlite` is a `cockpit-turso` internal change |
| Provider APIs move (ACP draft, OpenCode V2, Codex app-server) | Capabilities negotiated per connection; adapter conformance tests pinned to versions |
| Platform enforcement differs | Readiness reports requested/supported/effective; unsupported controls hold work rather than pretend |
| Designed UI slips | Plain TUI is complete by M5; M6 is polish, not function |
| Teams/Chief scope creep | M4 starts only after the keep/drop list from the implementation review (07) |

## Not in this plan

Shared multi-human control, native Windows execution, a self-written terminal runtime,
distributed deployment. Each is a new decision, not a deferred item.
