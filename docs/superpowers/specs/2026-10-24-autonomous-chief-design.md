# Autonomous Chief: design

Todo: `b77a0881`. Replaces the work-dispatching design in
`2026-10-23-global-dispatcher-chief-design.md` (shipped under todo `5fa54c26`).

## Why

Theo's agents (mostly OpenCode) often stop mid-task to offer several options or to
ask "continue?". Most of the time a reasonable answer is obvious and the agent
could have kept going. Today Theo has to notice each stop, read it and type an
answer. The Chief exists to take that load: keep agents moving, and pull Theo in
only for decisions that really need him.

The live run of the first Chief (2026-10-24) showed the opposite: Theo was needed
at almost every step (per-work approvals, manual test sign-offs, decisions that
stayed open after their cause was gone, a 5-minute wait after every Chief turn),
and the Chief view was hard to navigate. That design solved "who may start which
work"; the todo queue already does that well enough. The real need is unblocking.

## Goals

- When an OpenCode agent stops with a question or a checkpoint ("continue?",
  "option A or B?"), the Chief answers it in Theo's place when it is confident.
- Theo is pulled in only for: anything touching security; anything that changes
  the scope of the work the agent was given; anything the Chief is not confident
  about.
- Theo can see afterwards, without being notified, everything the Chief answered,
  and overrule any answer.
- The Chief learns Theo's habits over time from his own answers and overrules,
  in a list Theo can read and edit.
- When two sessions are blocked on each other (often across projects), the Chief
  links them so they can talk directly.
- The Chief and the existing todo queue run side by side.

## Non-goals

- Starting new work. The todo queue keeps doing that (next todo to an idle
  session, `--after` dependencies, team QA handoffs).
- Claude Code and Codex sessions. They stay watch-only for the Chief; OpenCode
  first, others later once their delivery path is verified like OpenCode's.
- Hidden or statistical learning. The learned habits are a readable list.
- Notifications for routine answers.

## Anti-goals (a failure even if it works)

- The Chief answers a security question, a permission prompt, an `ask-to-run`
  approval, a `cockpit apply` review, or a question that widens or changes the
  agent's task. These always reach Theo, whatever the habit list says.
- The Chief types into or submits Theo's unsent draft, or interrupts the session
  Theo is looking at.
- The Chief answers silently with no record Theo can find.
- Theo needs more screens or approvals than today to keep agents moving.
- Enabling the Chief pauses the todo queue (today's behaviour: "chief owns
  scheduling").

## What the Chief does

### 1. Answer stopped agents

Trigger: an OpenCode session goes idle and its last assistant message asks Theo
something or waits for a go-ahead.

The Chief reads:
- the agent's last message;
- the task's context: the todo text and notes, and the recent conversation of
  that session.

Then it decides one of:
- **Answer:** confident, and none of the escalation rules apply. It sends the
  answer to the session as a message.
- **Escalate:** security, scope change, or not confident. Theo sees it as a
  decision (see "What Theo sees").

How the answer reaches the agent: the existing verified OpenCode path (synthetic
queued message, receipt = `msg_cockpit` + message id), which was live-checked on
2026-10-24 to leave Theo's unsent draft untouched, wait for running turns and wait
for open question forms. The existing readiness checks still gate every delivery:
session idle, not the session on Theo's screen, no open permission/question,
fresh native telemetry, the visible-session guard (`fd4f35e`).

Each answer is recorded: session, the agent's question, the Chief's answer, a
one-line reason, time, and which habits it relied on.

### What a Chief answer authorises (Theo's decision)

A Chief "yes, continue" counts as Theo's explicit instruction for actions already
inside the agent's given task: edits, builds, tests and commits on the task's
feature branch. It never authorises, and these still come to Theo:
- installs, dependency manifest changes, migrations;
- pushes, merges, anything on the default branch, and other git writes the
  project's own `## Git authorization` does not already allow;
- anything outside the task's scope, and anything security-related;
- `ask-to-run`, `cockpit apply` and permission prompts (always Theo's own).

The message the Chief sends must state this boundary so the agent applies it.

### Question forms (builder check, Chief's call)

Agents often ask through OpenCode's question form rather than in plain text. The
Chief answers both: plain-text questions with a message, question forms by
replying to the form through the native form API, under the same escalation
rules. The live check showed queued messages correctly wait while a form is open,
so a form must be replied to, not messaged.

### 2. Link sessions that wait on each other

When the Chief sees two sessions blocked on each other (for example one waits for
a change the other must make, possibly in another project), it links them so they
can message each other directly. It does this without asking Theo and records the
link in the same log.

This uses the cross-project session links being built in the session "Port
cross-project session links". The Chief must not build its own linking
mechanism; it calls that one. Until that ships, this part is not available.

### 3. Learn Theo's habits

A readable, editable list of Theo's habits (for example "prefers the smallest
fix", "wants tests run before applying").

- Sources: answers Theo gives agents himself, and every overrule of a Chief
  answer. Theo's written preferences (his global and project instruction files)
  are also read.
- New habits are added automatically and marked "new". They appear in the daily
  digest. Removing one is a single keypress.
- An overrule becomes a habit immediately: it is Theo's clearest signal.
- No habit can make a security or scope question answerable by the Chief.
- Scope (my call, from the builder check): a habit is tagged with its project
  unless it clearly applies everywhere; an overrule creates a project-tagged habit
  worded as a general preference. Theo can delete or re-scope any habit.

### Overrule effect (my call, from the builder check)

An overrule sends the agent a correction and records the habit. It never undoes
work on its own. If the agent already acted, it treats the correction as a new
instruction within its task; anything that would require undoing or changing
scope is escalated to Theo instead.

## What Theo sees

No notifications for routine answers.

- Each session card shows a small count, e.g. "Chief answered 3". Opening it
  lists those answers (question, answer, reason).
- The same list (answers and links since the last digest, plus new habits)
  appears in the daily digest and in on-request summaries.
- **Overrule:** from that list Theo picks an answer and types the correction. The
  Chief sends the correction to the agent and turns it into a habit.
- **Escalated decisions** are the only thing that asks for Theo's attention. They
  show the agent's question, why the Chief did not answer (security, scope, not
  confident) and its suggested answer if it has one. Theo answers; the answer goes
  to the agent and counts as a habit source. A decision closes itself when the
  agent moves on or Theo answers.

The Chief view becomes much simpler: on/off, the escalated decisions, the answer
log, the habit list. Readability rules from todo `0b6cba5b` apply (session names
instead of raw ids and JSON).

## What changes in the existing Chief

Remove (no longer needed for the goal):
- work proposals, per-work approvals and approval hashes;
- evidence gates and manual attestations;
- Chief-owned assignments and the Chief's scheduling ownership, including the
  rule that pauses the legacy todo queue while the Chief is on.

Keep and reuse:
- the verified OpenCode delivery path and its receipts;
- admission verification of a Chief session and worker readiness checks;
- the visible-session guard (`fd4f35e`), focus publishing (`baafca2`);
- enable/disable of the Chief with a verified session.

Rework:
- The Chief's read access: today the Chief sees only bounded status, never
  conversations. It now needs, for a stopped session: the last message, the task's
  todo text/notes and the recent conversation. Read access stays scoped to what a
  decision needs; credentials and secrets stay out. My call (builder check): the
  Chief reads conversation text only (user and assistant messages), never tool
  outputs or attachments, with obvious secrets (tokens, keys, passwords) masked
  before it sees them.
- The 300 s idle guard after every Chief turn (`QUEUE_IDLE_S`) applies to Theo's
  sessions so he isn't interrupted; it should not slow the Chief's own turns.

Open bugs from the first Chief that may disappear with the removal: `ef9f6955`
(batch approval order). `b2644a4e`'s reservation fix stays relevant only if
reservations remain.

## Constraints

- Agents run inside greywall sandboxes; the Chief is itself an OpenCode session
  in a sandbox. Host-side state (answer log, habits) lives in the protected host
  store, written by the daemon, not by the Chief directly.
- Every Chief action is authenticated through the existing `cockpit-team` IPC;
  nothing new may bypass it.
- Theo's approvals (`ask-to-run`, `cockpit apply`, permission prompts) are never
  answerable by the Chief.
- Fail closed: if the Chief is unavailable or unsure, agents simply wait for Theo
  as they do today.

## Left to the builder

- How the Chief judges "confident" and recognises security and scope questions
  (prompting, rules, or both), as long as the escalation rules above hold.
- How "blocked on each other" is detected.
- Storage format of the answer log and the habit list, and the editing interface
  for habits.
- Migration path from the current Chief store (data from the test run can be
  dropped).
- How much recent conversation the Chief reads.
