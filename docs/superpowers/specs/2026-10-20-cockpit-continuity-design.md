# Cockpit task continuity

## Intent and approval

Help Theo resume work across multiple projects and agents without repeatedly
reconstructing what changed, why automation is waiting, or where a task went.
This is an extension of the existing Ratatui/tmux cockpit, not a replacement UI.

Theo requested all five brainstormed improvements, reviewed the browser preview,
and requested no changes to it. The behavioural design calls below make that
preview implementable; this written design still needs his approval.

The preview lives in this checkout's `.superpowers/brainstorm/` directory. It is
throwaway HTML, not a production frontend or a dependency of the cockpit.

## Goals

1. Distinguish unseen results and questions from sessions that are merely idle.
2. Explain queued work using the same rules that actually dispatch it.
3. Navigate todo → session → latest result → associated pull requests.
4. Keep a user-written reminder of what to do when returning to a session.
5. Defer a question or PR review without answering, approving, or hiding it.

## Constraints and non-goals

- Keep Rust, Ratatui, tmux, the four existing panels, stable session ordering,
  current colours and focus behaviour. No new permanent panel.
- No dependency additions or manifest changes.
- No automatic commits, pushes, merges, applies, approvals, or answers.
- Preserve terminal-session exclusions, external-session restrictions, designer
  routing, the viewed-session queue exclusion, and gaming pauses.
- No web/phone feature parity, new desktop notification subsystem, global agent
  instruction changes, inferred task dependencies, or AI-generated summaries.
- Results are the existing hook's reported turn outcomes. Display missing or
  unknown evidence honestly; do not claim tests passed without that evidence.
- New user state is cockpit metadata, never instructions delivered to agents.
  A return note is not promised to be inaccessible to every local process.
- Existing provider, quota, security, and diagnostic behaviour stays intact.

## Shared interaction model

Keep the two-line session cards and the selected-session details area. Use short
text labels as well as colour: `unseen result`, `new question`, `deferred`.
Counts include all running sessions before the attention filter is applied.
Selecting, opening, scrolling, or focusing a session does not acknowledge it.

Expose the new operations as named actions in the existing searchable `:` menu.
Add contextual hints and clickable actions where space permits. Do not assign
new single-key shortcuts in this iteration: existing keys retain their meanings.

Use the existing scrollable details presentation for longer content. Generalise
its title and available actions only as needed to support Updates, Queue, Task
trail and Session details. No new popup framework or global event abstraction.
Escape returns without changing state. Long or Unicode content wraps safely;
narrow columns retain access through `:` even if hints or detail rows do not fit.

## 1. Unseen changes

The summary reports current attention, unseen updates, and deferred items
separately. Session activity remains `Working`, `Ready`, etc.; unseen is an
additional property, not a replacement state.

- A completed turn with a nonempty outcome creates an unseen result. Identify
  it by session id and a stable per-turn result id supplied by the hook/daemon
  integration below, never by the activity timestamp alone. A turn ending in a
  question can have both a result and a question. Refreshing or reopening the
  cockpit must not recreate an acknowledged event. An unchanged old outcome
  attached to a question or command-state update must not become a new result.
- A new explicit question creates an unseen question. Identify it by session id,
  question kind and full question text, not elapsed time, summary truncation,
  changing display labels, or an approval's transient phases.
- On first observation, an existing nonempty identified turn outcome and any
  current explicit question are unseen. Do not fabricate earlier history.
  Old sessions without new result metadata show `Result tracking unavailable;
  restart to enable` in details, keep their existing Last turn display, and do
  not claim reliable unseen-result tracking until an identified turn arrives.
- Keep at most the latest result and current question per session. This is not
  an activity log. If several turns occur before review, explicitly label the
  displayed result `Latest turn`; do not imply earlier results were read.
- `updates` opens a list with session identity, event kind, time, and available
  outcome/request text. `reviewed` acknowledges the selected session's currently
  displayed unseen events, not future ones. The same action is available from
  Updates for a selected event. Completion acknowledgement never resolves an
  outstanding question, permission request, or command approval.
- Resolved questions leave the Updates list. Unseen results survive session exit
  and can still be reviewed as archived metadata; they are not controllable live
  sessions. New session ids are distinct, even in the same checkout.

## 2. Explain the queue

`queue` opens a project-scoped explanation, using the same scope as Todo; when
there is no viewed session it shows all known queued projects. Show items in
actual dispatch priority order, grouped by designer/developer routing where
necessary, since there is not always one global next item.

For each item show whether it is still To do, eligible sessions of its repository
and role, and why each cannot currently take it:

- queue disabled, gaming pause, or no matching session;
- no terminal target, terminal-managed session, unknown state, working, awaiting
  an answer/approval/input, or another assigned todo still in progress;
- currently viewed on the active cockpit workspace;
- not idle for the existing 300-second minimum; show remaining seconds;
- ready for the next dispatch check, which runs every 30 seconds.

Do not show a precise start promise. Decisions can change before the next check.
Offline/stale GitHub issue data is labelled unavailable or last fetched, not
mistaken for an empty queue. The view is read-only; `Q` retains queue toggling.

Extract explicit shared eligibility/candidate calculations from `dispatch_queue`
and `next_queued`; both the explanation and dispatcher call them. Keep current
priority ordering and exclusions, but fix the safety hole where a `waiting`
session with `needs_input` or an approval/input request could otherwise take new
work. Deferred questions are equally blocked. This rule needs a regression test.
Queued manual messages remain a separate mechanism, with their current delivery
behaviour; this change does not silently reorder or edit them.

## 3. Follow a task

`task-trail` is available from Todo, Sessions and Pull requests. It opens explicit
related records with navigation actions, not just a text breadcrumb.

- Reuse todo assignments already stored on typed items and GitHub issue status
  records. Record successful handoffs and queue dispatches as task/session links.
  Extend the existing todo handoff note to instruct the agent to run
  `claude-todo start <key>` before working, in addition to its existing done
  instruction. A background handoff without a known id remains `Starting;
  awaiting assignment` until this existing attributed update identifies the
  session; never guess by first session to appear. Surface launch failures using
  existing manager errors. Do not add a launcher protocol solely for linking.
- New background/parallel sessions whose id is not known at launch are linked
  when observed through an exact `todo-<id>` or `issue-<number>` branch and the
  matching canonical repository identity. Never link by title or short project
  name alone. An `issue-<number>` requires the full GitHub repository match.
- A task can retain multiple session links, and a session can have worked on
  multiple tasks. Label assignment and observed-result times. Associate an
  outcome with the task active for that session at observation time; never
  rewrite earlier task outcomes when the session takes another task.
- A task's result is the latest observed outcome for each linked assignment,
  not a guarantee that the task is finished or that all tests ran.
- PR links require exact canonical repository and head-branch matches to a
  linked session/assignment. Preserve `headRefName` already requested by the
  GitHub fetch but currently discarded by `Review`. Record confirmed PR
  identities once seen so a later disappearance from open PRs is not shown as
  proof of merge. Do not add remote requests on render or on every key press.
- Show every exact match rather than picking the first. Missing associations
  say `No linked session`, `No result recorded`, or `No linked PR`. Fetch errors
  or unloaded repositories say `PR data unavailable`/`loading`, not `No linked PR`.
- Navigating to a live session uses the existing view action. An exited session
  shows its recorded result and offers the existing resume mechanism only when
  its conversation and checkout can be resolved; it is never silently recreated.
  Navigating to an available todo selects its proper tab, including Done,
  without changing status. Persist a minimal task snapshot (key, canonical
  repository, title, notes, last observed status and assignment times) when
  linking it. If the seven-day Todo retention or a closed issue removes the
  source record, open a labelled archived task detail, not a synthetic live Todo
  row. Do not resurrect deleted/closed tasks. Opening a PR uses the existing PR
  browser action.
- From a PR shared by multiple tasks or from a session with several assignments,
  show the choices; do not silently navigate to one arbitrarily.

Use already gathered branch metadata and the added sandbox-reported repository
metadata below, not a new host `git` call in an agent-writable checkout. Missing
identity means no automatic association. Explicit task/session assignments still
work for local-only repositories; without a GitHub identity there are no
automatic issue or PR links.

## 4. Return notes

`return-note` edits the selected session's one-line note through the existing
input/popup facilities; `clear-return-note` explicitly removes it. Cancel leaves
the original untouched. Accept at most 280 Unicode characters, disallow control
characters and terminal/tmux escapes, and display it as plain text.

Show `Your note: …` in the selected-session details and full details whenever
present. It remains visible on returning to that session, survives cockpit and
machine restarts, and never clears on focus or result acknowledgement. Exited
session notes remain available in archived continuity details; notes are keyed
by session identity, not branch or project, and are not silently transferred to
an unrelated new conversation. A resume preserving the conversation id retains
its note. No automatic note prompt when switching sessions.

## 5. Defer attention

`defer-20m` defers only a current explicit agent question or a selected PR that
needs review. `bring-back` cancels that deferral. Twenty minutes is the only
duration in this iteration, as shown in the approved preview.

- Questions identify the exact session and full question content; PRs identify
  full repository, number, and meaningful review revision (head commit and review
  decision). A new question, new head commit or changed review decision clears
  the old deferral immediately. A changing relative timestamp or display label
  does not. Add `headRefOid` to the existing GitHub fetch for this comparison.
- Deferral stores an absolute expiry time so it survives restarts. After expiry
  the existing attention navigation/filter includes the item again. If expiry
  happens with the cockpit closed, it is overdue on next launch; do not promise
  background notifications from a process that is not running.
- Deferred items remain visible and counted in the normal lists with a readable
  expiry. `n`/`f` omit deferred questions until expiry, while the summary says how
  many are deferred. Deferred PRs remain in the PR list with their review state.
- Deferral is not an answer, acknowledgement, todo status change or automation
  permission. A deferred session still needs input and cannot take queued work.
- Approval requests, permission dialogs, running commands needing passwords or
  input, unknown states, and sessions merely Ready are not deferrable. A new
  nondeferrable request overrides a question deferral immediately.
- This feature controls cockpit attention navigation and labels only. Existing
  desktop banners, phone notifications and daily digests retain their existing
  policies; there is no claim that snoozing suppresses them.

## Necessary hook/daemon integration

Review found that activity `since` changes when state changes, not at a unique
turn completion, and an explicit question currently replaces the daemon's
finished-message trigger. Reliable unseen results cannot be implemented using
that timestamp and message alone. This design therefore includes small changes
to `config/claude/.claude/hooks/session-state.sh`, its session-report schema in
`apps/desktop`, and their tests. These are part of the requested feature, not an
unrelated hook refactor. No Makefile, launcher or dependency change is planned.

- The hook reports a stable opaque per-turn id and explicit completion marker on
  Stop, together with its outcome, independently of the banner message. Initialise
  a new id when a turn starts and reuse it for duplicate Stop reports. If startup
  misses a start event, create and persist a fallback id for that completion;
  never reuse a prior completed turn's id for later work. A subsequent real turn
  gets a different id even if its outcome text is identical.
- The daemon validates and attributes these fields through the existing report
  socket and kernel-derived sender identity. It stores the latest result id,
  outcome and host-observed completion timestamp independently of current
  activity/question state, ignoring duplicate completion ids. Do not make the
  banner's text a protocol signal for new metadata. Retain old-client behaviour
  for reports without the new fields; absence never fabricates completion.
- At session start and turn end the hook reads the origin and branch inside the
  sandbox. Report normalised GitHub `owner/repo` identity only for recognised
  GitHub remote forms; otherwise report no GitHub identity. Treat it as bounded
  untrusted display/link metadata, never as authority to access another project's
  todo updates, files, credentials, tmux targets, or approval channels. Snapshot
  attribution and original launch cwd protections remain unchanged.
- Claude and OpenCode already use this common hook; verify both payload paths.
  Protocol additions are optional for older producers and consumers. Existing
  sessions need a restart after deployment to load changed hook/config files.
- Daemon result retention permits the column to notice the latest completed
  turn while the session has already started its next turn. An intermediate
  result overwritten before the column can observe it is outside this
  latest-result feature; do not promise a lossless event archive.

These files run with host rights or participate in host/session reporting. Call
them out explicitly in the implementation handoff and host-reviewed apply.

## State, errors and architecture

Use one focused Rust module for persisted continuity metadata and pure operations
on it. Keep UI wiring explicit in `app.rs`; do not refactor unrelated panels.
Queue policy can be a small focused module if extracting it from `app.rs` makes
the shared evaluation easier to test. No service/repository/mapper layer.

Store versioned continuity state separately from `todo.json` and `view.json`,
under the existing cockpit state directory, using atomic replacement and an
owner-only file. State includes observed result/question identities,
acknowledgements, notes, deferrals, and task/session/result/PR associations.
Never write from rendering or on every 250ms tick: only meaningful observations
and user mutations. Preserve unknown fields on supported-schema round trips.

Missing state means first run. Unreadable, invalid or unsupported-version state
must produce an actionable visible error and be preserved, not overwritten with
an empty default. Read bounded regular files without following final symlinks.
A failed save must be visible; do not report an acknowledgement, saved note or
deferral as successful if it was not persisted. The previous file remains valid.

Keep a single host-column writer. Do not create new agent-writable command
channels. Ignore unknown enum values conservatively: no inferred approvals,
linkage or deferrals. Validate persisted text, identifiers, lengths, timestamps
and collections at the boundary before displaying them or invoking navigation.

Retain unseen archived results, notes and linked task history until explicitly
acknowledged or cleared as applicable. Do not auto-prune them on session exit or
because Done leaves its existing seven-day view. Empty records with no surviving
notes, unseen events, valid deferrals or task links may be pruned. Do not copy
full transcripts or command secrets; persist only existing outcome/request data
needed by these behaviours. Reuse current audit conventions for user mutations,
fingerprinting user-authored text rather than logging it in plaintext.

## Verification and delivery

Test through the real `App` key/action, tick, rendering and persistence entry
points with the existing fake HOME/runtime/PATH fixture. Pure policy tests can
supplement, not replace, the wiring tests. Cover:

- explicit acknowledgement, identical consecutive outcomes, duplicate Stop,
  explicit question on Stop, repeated refresh, restart, exits and old producers;
- queue explanations agreeing with actual dispatch, including blocked `waiting`
  sessions, roles, viewed sessions, priority, gaming, disabled settings and time;
- exact/multiple/missing task and PR links, same-name repositories, reused
  sessions, asynchronous launches, stale fetches and ended sessions;
- note edit/cancel/clear, Unicode/escape rejection, restart and save failure;
- deferral expiry/restart/new question/new PR revision, counts, attention
  navigation, refused approval deferrals and continued queue blocking;
- narrow/short terminal layouts and corrupt/oversized/symlinked state.

Use the existing workspace test commands and test binary sandbox. A full build,
dependency installation or host deployment still needs explicit approval under
Theo's global rules. Keep manifests, Makefiles, launchers and compose files
unchanged. The hook and desktop daemon additions above require dedicated
protocol/attribution tests and explicit mention in the host apply review.
Update the cockpit README as part of implementation, pointing out its new
actions and the cockpit-only scope of deferral. Show the diff stat and verbatim
test outcomes. Follow the repository's authorised feature-branch commit/export
and host-reviewed dotfiles apply workflow; never push or merge unasked.

## Calls made by the agent

These are proposed design calls, not separate requests from Theo: explicit
acknowledgement; named menu actions instead of new shortcuts; latest-result
tracking instead of a full event feed; 280-character session-local notes;
20-minute, cockpit-only question/PR deferral; exact repository/branch PR linkage;
archived unseen results and task snapshots; one versioned sidecar state file;
small hook/daemon reporting additions; explicit start acknowledgement in todo
handoff prompts. The reviewer may settle internal Rust names and test layout
without changing these behaviours.
