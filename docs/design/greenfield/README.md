# Cockpit greenfield design reports

Decisions from the 2026-10-30/31 architecture research for a from-scratch Cockpit: a
personal, terminal-first agent orchestrator distributed to other users.
Written for agents and humans: one topic per file, decisions first, sources last.

Audience rule: read `README.md` plus the file for your topic; do not load all files.
Each file is under 120 lines, except the coverage map (16), which is a checklist.

## Status legend

- `decided`: Theo settled it in chat; implement as written.
- `proposed`: researched recommendation; implement unless a later decision overrides.
- `validate`: must be proven by a test or measurement before it counts as true.

## Reports

| # | File | Topic | Status |
| --- | --- | --- | --- |
| 01 | [scope-and-principles](01-scope-and-principles.md) | Personal install, platforms, terminal-first, greenfield rules | decided |
| 02 | [crate-architecture](02-crate-architecture.md) | Capability crates, ownership and dependency rules | decided |
| 03 | [runtime-daemon](03-runtime-daemon.md) | One Tokio daemon, bounded work, control-path protection | proposed |
| 04 | [state-durability](04-state-durability.md) | Turso (SQLite-compatible), identities, recovery, change feed | decided |
| 05 | [terminal-and-tui](05-terminal-and-tui.md) | tmux hosting, Ratatui reference TUI, UI last | decided |
| 06 | [tasks-workflows-verification](06-tasks-workflows-verification.md) | Workflow presets, change kinds, evidence-bound completion | proposed |
| 07 | [teams-and-chief](07-teams-and-chief.md) | Teams as workflow instances, Chief as decider | proposed |
| 08 | [agent-onboarding-context](08-agent-onboarding-context.md) | Shipped guidance, launch brief, provider capability negotiation | proposed |
| 09 | [project-setup-readiness](09-project-setup-readiness.md) | Project config, readiness report, verification receipts | proposed |
| 10 | [resources-scheduling](10-resources-scheduling.md) | Shared budget, verified limits, fairness, pause semantics | proposed |
| 11 | [security-isolation-platforms](11-security-isolation-platforms.md) | Per-user daemon, agent authority, sandbox profiles, WSL | proposed |
| 12 | [diagnostics-logging](12-diagnostics-logging.md) | `COCKPIT_LOG_LEVEL`, runtime reload, support bundle | decided |
| 13 | [release-upgrades](13-release-upgrades.md) | Release bundle, running-session compatibility, migrations | proposed |
| 14 | [plugins](14-plugins.md) | Plugin-ready now, WASM plugins later | decided |
| 15 | [validation-open-questions](15-validation-open-questions.md) | What is unproven and the acceptance scenarios | validate |
| 16 | [feature-coverage-map](16-feature-coverage-map.md) | Every current feature → owner, keep/change/polish/drop/plugin | proposed |
| 17 | [agent-channel-and-approvals](17-agent-channel-and-approvals.md) | One authenticated agent channel, `ask run`/`ask guide`, host installs | proposed |
| 18 | [usage-quota-and-accounts](18-usage-quota-and-accounts.md) | Plan windows, cost, time, digest; account routing simplified | decided |
| 19 | [desktop-and-remote-clients](19-desktop-and-remote-clients.md) | `cockpit-notify`, desktop extras out of the daemon, phone client | proposed |
| 20 | [refactoring-plan](20-refactoring-plan.md) | Milestones M0–M7, parity gates, cutover | proposed |

## Decision register (short form)

1. Personal installation only: one human owner, one daemon per OS user, many clients.
2. Platforms: Linux and macOS native; Windows runs Cockpit entirely inside WSL2.
3. Terminal-first: CLI + TUI are the product; phone/web is an optional client.
4. Root `crates/` with capability-owned crates; no `core`, `domain`, `common`, `host`.
5. State in Turso Database, SQLite-compatible mode, CDC on; no MVCC or experimental flags.
6. tmux is a required dependency and hosts agent terminals; `cockpit-tmux` owns the integration.
7. Plain reference TUI first; designed UI/UX is the last milestone.
8. Diagnostics log Cockpit's own operation only, never hosted work; `COCKPIT_LOG_LEVEL`.
9. Plugin-ready boundaries now (versioned protocol, extension traits, grants); WASM plugins later.
10. Teams and Chief are redesigned as thin behaviors over tasks, workflows, messaging, scheduling.
11. The daemon has no desktop dependencies; notifications, bars and window tricks are clients.
12. One authenticated agent channel replaces the report socket, team socket and file queues.
13. Build the new workspace beside `apps/` and cut over once at milestone M5 (report 20).
14. Accounts switch only at launch and at a quota-stop restart; no mid-turn routing gates.
15. Greywall and greyproxy are Cockpit's own patched forks, pinned and built by the release.

## Relation to earlier documents

- `docs/audits/audit-2026-10-30.md` and companions hold the evidence; their five-crate
  "keep `apps/`" recommendation is superseded by report 02.
- `docs/superpowers/specs/2026-10-29-bounded-diagnostic-logging-design.md` (approved in
  another session, not yet on `main`): its storage and host-only control limits carry forward;
  its content rule is relaxed only as report 12 states.
- `docs/superpowers/plans/2026-10-30-cockpit-stabilization-roadmap.md` needs realignment
  to these reports before execution.
