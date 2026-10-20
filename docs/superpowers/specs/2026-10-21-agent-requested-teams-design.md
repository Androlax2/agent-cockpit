# Agent-requested teams: new teammates started on Theo's approval

Date: 2026-10-21 · Todo `2a0779fa` · Builds on linked cockpit sessions
(`2026-10-20-linked-cockpit-sessions-design.md`).

## Why

While building the jobpilot UI, Theo's agent went straight to UI code, and he had to tell it to use
FigCraft. Linked sessions already let a Designer, a Developer and a QA message each other, but only
sessions Theo had already started could be linked. What he wants: an agent working alone notices it
needs teammates (a design before UI code, a review after it), asks for them once, and after a single
yes the team exists and works without Theo relaying anything.

## The flow (as Theo agreed it)

1. **Any agent can ask.** It submits a request naming the task, its own role, the roles it lacks
   (Designer, Developer, QA) and a short starting brief for each new teammate.
2. **Theo answers yes or no** in the cockpit's Team view. Nothing starts before the yes; no agent can
   approve, edit or widen a request. The request shows each brief, read-only, so Theo sees what each
   teammate will be told.
3. **After the yes**, the cockpit starts each new teammate:
   - in the **agent named by the global settings** (the Settings menu's default agent,
     `claude-sessions` setting `default-agent`), never one the request picks;
   - **Designer and Developer** in the folder of the agent that asked (they share the code and the
     FigCraft design file);
   - **QA** not at approval but **per handoff**: each Developer handoff makes a fresh copy of the
     asker's repository at exactly the handed-off commit (made host-side, as `claude-sessions parallel`
     makes its copies) and starts a new QA session there with the review request. Theo's one yes for
     the team covers these QA sessions. A new QA session has no memory of earlier rounds; the
     earlier findings of this task are part of its review request;
   - each one linked to the task with its role, and started with its brief as its first message.
4. From then on everything is the existing linked-sessions behaviour: messages, three-round
   clarifications, handoff → one QA request, findings, escalations to Theo.

## Goals

- One yes from Theo replaces "start three sessions, then link them".
- The requesting agent keeps working in its own session; it becomes the role it named.
- The team is exactly what Theo saw: task, roles, which session is the asker, the briefs.

## Non-goals

- Choosing the app per teammate (always the global default agent).
- Teams larger than one Designer, one Developer and one QA.
- Codex-specific work (Codex is tracked separately).
- Agents starting sessions without Theo, or any session start outside the cockpit's host side.

## Anti-goals (failure even if it "works")

- A new session starting before Theo's yes, or a yes covering something he was not shown.
- A brief or a teammate's message granting tool use, scope, installs, permissions, Git writes, merges
  or releases (messages stay collaborator content, as today).
- A spawned session claiming a role or a team by saying so: membership comes from what the host
  launched, never from what a sandbox reports.
- Two teams sharing one session, or a spawned session joining a team other than the approved one.

## Constraints

- Approval stays in the host-only Team view (`: team`) and the host socket; agents only submit.
- Sessions start through `claude-sessions` on the host (outside the sandbox). Sandboxed agents cannot
  start sessions themselves.
- Identity of a spawned session must be proven host-side, like today's launch proof (boot id, pid,
  start time of the first host process). The daemon must link "this new session" to "this approved
  spawn" from facts the sandbox cannot forge (for example a value the host launch passes and the
  daemon reads from the launch's host process, as it reads `CLAUDE_SESSIONS_TEAM_CHANNEL`-style
  launch arguments today), never from the session's own report.
- A session belongs to one team at a time (existing rule). If the asker is already in a team, the
  request may only fill that team's missing roles, same yes/no.
- Existing limits hold: rate limits, queue caps, durable store in `teams.sqlite` hidden from sandboxes,
  audit records.
- Briefs are text shown escaped, length-bounded, never interpreted by the terminal.
- FigCraft stays per checkout: the Designer in the asker's folder uses that checkout's FigCraft port.

## Decided (by Theo)

- Any agent may ask; its own role is part of the request.
- Theo decides only yes or no.
- New teammates run in the global default agent.
- Designer and Developer share the asker's folder; QA works in its own copy on the handed-off commit.
- QA (option A, 2026-10-21): a fresh copy and a new QA session for each handoff, covered by the team's
  one approval; the earlier findings travel in the review request.
- The approval shows each brief (Theo confirmed the flow including this).

## Left to the builder

- The request format (an extension of `cockpit-team propose`, or a new action) and its validation.
- How the daemon binds a newly reporting session to its approved spawn, within the identity
  constraint above, and what happens when a spawned session never reports (timeout, shown to Theo,
  no automatic retry that could start a second session).
- How each QA copy is named; it must be at the exact handoff commit, made without running git on the
  host inside a folder a session can write (the existing clone rule).
- What happens to the previous QA session when a newer handoff starts a new one.
- How the Team view lays out a spawn request next to plain link requests.
- How agents learn they can ask (help text and instructions); see "Open" below.

## Calls made while writing (check these)

- **The brief is the new teammate's go-ahead.** Theo approved its exact text, so the new session
  starts the work it describes. Tool approvals, greywall and every existing permission rule still
  apply; the brief is labelled as written by the asking agent and approved by Theo.
- **The agent is fixed at approval and shown on it.** The request shows which agent each teammate
  runs in (the global default at that moment); a later settings change does not alter an approved
  team. Codex is allowed if it is the default (its team adapter exists; its live check is tracked
  separately).
- **Previous QA sessions** are unlinked when a newer handoff starts the next one and are left running
  for Theo to close; nothing is killed automatically.

## Open (for Theo)

- Whether agents should be told *when* to ask (for example in the global instructions or the
  design-first skill: "UI with no design → request a Designer"), or only *how*.
