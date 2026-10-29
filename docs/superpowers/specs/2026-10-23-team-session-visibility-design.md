# Desktop team session visibility

## Intent and approval

Theo cannot readily see which running sessions work together, what they are doing, or who is waiting on whom. He approved grouping sessions by shared task inside the existing project list, then approved the terminal-style FigCraft design for implementation on 2026-10-23.

Authoritative visual reference: `team-visibility.figcraft`, page `02 Terminal design` (46):

- Frame 47: `Desktop cockpit / Linked team active`.
- Frame 119: `States / Honest status and narrow content`.

Both frames are marked ready for development. `DESIGN.md` describes the scoped visual language. `design/tokens.json` and `design/tokens.css` are exports of its FigCraft variables. The reference is a static mockup with sample data, not live telemetry or a new web interface.

## Goals

1. Make task boundaries and all linked roles apparent without opening terminals.
2. Show a member's session identity, current runtime state and latest available reported activity together.
3. Explain recorded handoffs, planned QA and explicit blockers without guessing.
4. Keep a team's need for Theo visible even when collapsed.
5. Preserve standalone sessions, existing terminal focus and existing session actions.

## Presentation and navigation

- Preserve project grouping; nest teams within their project using authoritative `(project, task_id)` identity. Different tasks with identical titles remain separate.
- A task header has a disclosure marker, task title, linked-member count, planned QA indication when recorded, and a concise attention or handoff summary.
- Member order is Designer, Developer, QA. Show role/status, session name, then activity and its age. Show the running agent in full details. A missing or sleeping teammate is not silently dropped.
- Independent sessions keep their existing compact rendering. A session belonging to a team must not also appear as independent.
- Initially expand teams; retain explicit folds during the current App lifetime. Keep project/start-order stability rather than continually moving active teams. Pinned sessions must remain reachable without duplicating them.
- Up/down navigates visible targets; Enter toggles a task heading or opens a real member. Planned/unavailable members must never be treated as terminal targets. Existing left/right focus behavior stays unchanged.
- Next-needing-me and search find members inside collapsed teams, expand the appropriate team, and preserve the actual session action target. A fold must not change the currently viewed terminal or wake a sleeping session.
- In a narrow sidebar, ellipsize session names by terminal display width, keep the status/reason readable and show the full identity in selected details. The narrow reference is a desktop stress case, not a mobile redesign.

### Builder calls for plan review

These are implementation decisions proposed by the Designer after the builder check, not additional decisions attributed to Theo:

- Resolve a task heading from the existing todo/issue entry with the exact task identity and verified project association. If its human-readable title is unavailable, show `task_id`. Never use a teammate's independently editable session title as the shared task identity.
- When `n` encounters task-only attention with no actionable member, select and expand its task heading. `i` on that heading opens the existing team modal so its approval, escalation or delivery issue is actionable. Enter remains disclosure-only on headings.
- In attention-only mode, retain all members of an attention-bearing team as context and retain its actual member count. Do not classify filtered-out members as unavailable. Teams with only task-level attention remain visible; genuinely independent sessions retain their existing actionable-only filter. Group from the complete session set before applying this view filter.

## Evidence and truthfulness

- Membership comes from the daemon team snapshot, not labels, branch names or cwd guesses. Match actual session IDs, including conversation/launch aliases already supported by the application.
- Existing `Session` fields and `sessions::activity_text` are the starting point for reported activity. Do not add transcript scraping, an LLM summarizer or new telemetry for this feature. If no usable report exists, show `No activity update`. Do not manufacture an update timestamp from the UI refresh time.
- A current handoff means submitted, not delivered or reviewed. Use recorded spawn/message states to distinguish QA planned, starting, started, failed and unavailable. Only show a planned QA slot when a standing QA plan exists.
- `Turn ended` is not task completion. A readable findings message may be summarized as `QA findings received`; do not infer a structured approval from arbitrary prose. `QA approved` is allowed only if an authoritative structured verdict exists. Review approval never implies applied or deployed.
- Attention comes from actual session questions/host-command approval/input, pending link approvals, open escalations, uncertain delivery and failed/refused starts. Scope each reason to its actual task; do not turn routine QA waiting into a request for Theo.
- If no blocker/handoff evidence exists, say `No handoff recorded` or `Waiting reason unavailable`, not `Waiting for developer` by inference.
- If team loading fails, keep session cards visible and report `Team links unavailable`; do not describe unknown memberships as independent or let cached links look current. Loading and successfully empty data are distinct.
- Escape all agent-written titles, reports and message text using the existing terminal sanitization rules.

## Existing implementation evidence

- `apps/cockpit/src/app.rs`: `build_sessions`, `selected_session`, `handle_sessions`, `check_team` and the team modal own rendering, targeting and snapshot polling. `check_team` currently refreshes every ten seconds and only retains role labels.
- `apps/cockpit/src/sessions.rs`: `Session`, `activity_label`, `activity_text`, sleeping/restarting state and project grouping.
- `apps/cockpit/src/teams/store.rs`: `snapshot` includes members/messages/proposals/escalations/spawns, but not handoffs or standing QA plans. Escalation entries currently lack task identity in the global snapshot. Add read-only snapshot fields; no schema migration is necessary.
- `apps/osd/src/teams.rs`: `host_snapshot` adds verified session/project identities.
- `apps/cockpit/src/tests.rs`: existing App fixture, `team_host`, `team_snapshot`, keyboard tests and buffer rendering checks.

## Visual evidence and checks

Palette/font were read from Theo's active Kitty configuration with his approval. Preserve existing ANSI roles in `app.rs`; do not replace them with a new RGB theme. Design exports are reference artifacts for this terminal product, not a new CSS runtime dependency.

Foreground on selected background is 5.53:1; secondary on selected background was 4.23:1 and was replaced with foreground. Use text labels as well as colors. Frames use auto layout, member rows use components/variants, and empty/loading/error/long-name states are documented.

Sources: [NN/g, Visibility of System Status](https://www.nngroup.com/articles/visibility-system-status/) informed visible state and actionable attention reasons; [Web Interface Guidelines](https://raw.githubusercontent.com/vercel-labs/web-interface-guidelines/main/command.md) informed focus visibility, overflow and missing-data handling. DOM-specific accessibility requirements do not describe terminal behavior; test keyboard access through the actual App.

## Non-goals and handoff rules

No new dashboard, mobile work, team scheduling changes, recruitment/authentication changes, dependency changes, terminal configuration edits, automatic approval, hidden transcript analysis or adjacent refactoring. Do not change Makefiles, hooks or launch scripts for this UI feature.

The implementation plan must be reviewed before execution. Developer work must finish with passing repository tests and a QA handoff. Keep the saved FigCraft file, token exports, spec and DESIGN.md together. Any deviation needed to represent real data should preserve the truthfulness rules; structural design changes return to Theo for approval.

## Design-only verification record

- FigCraft `inspect.readyForDev` returned frames 47 and 119 with status `readyForDev`; saved after approval and inspected again after reconnection.
- `npx --no-install @google/design.md lint DESIGN.md` on the host returned `"errors": 0, "warnings": 1, "infos": 3`. The warning is `No 'primary' color defined`; no primary alias was invented for this terminal palette. Spacing/corner omissions are informational because the implementation uses existing terminal cells rather than a web spacing system.
- `npx --no-install @google/design.md export --format dtcg DESIGN.md` exited 0 and emitted both populated categories: ten colors and two typography scales. The exporter omitted unitless line-height fields; retain the ratios in DESIGN.md and the explicit reference line-height in FigCraft JSON/CSS exports rather than treating that loss as an approved style change.
- The initial sandbox schema command failed with `EROFS` on npm's read-only cache; the host schema/lint/export checks used explicit `ask-to-run` approval and installation disabled. No cache remapping, package installation or manifest change was made.
- A fresh builder reviewed the spec and plan; the three resulting decisions are recorded under `Builder calls for plan review` and covered by plan tests.
- Product tests have not run: this handoff changes only six design/documentation artifacts, not product source. Live implementation and regression verification remain outstanding.
