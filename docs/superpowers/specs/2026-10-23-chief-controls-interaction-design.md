# Global Chief controls: interaction design

Task: `5fa54c26`. Designer-owned artifact; no product implementation authorization.

Status: Theo approved the assembled full design for developer handoff in this session,
after approving the individual sections and follow-up choices. Builder recheck passed.
The existing feature spec and implementation plan remain unchanged.

## Evidence and scope

- Behavioral authority: `2026-10-23-global-dispatcher-chief-design.md`.
- Implementation boundary: Task 7 of `../plans/2026-10-23-global-dispatcher-chief.md`.
- Read-only UI evidence: `apps/cockpit/src/teams/view.rs` and `apps/cockpit/src/app.rs`,
  particularly `draw_team_view`, `handle_team_view`, the searchable action menu and `confirm`.
- No FigCraft file exists in this checkout. Theo accepted browser wireframes, then explicitly
  accepted in-chat terminal wireframes when host localhost could not reach the sandbox preview.
- Initial alternative reference:
  `.superpowers/brainstorm/4732-1792767466/content/chief-entry-wireframes.html`.
  That file is a location comparison, not the approved complete interaction design.
- Keep the existing terminal font, terminal-theme palette, blue outline, selected-row highlight,
  plain-text detail, escaping and ordinary cockpit focus/navigation. No web redesign or new tokens.
- Example task names/counts are illustrative, not real approvals.

## F1: Entry and root — approved

Theo chose Option A: a separate Chief view, opened by `Chief` in the existing searchable
action menu. Team remains unchanged. No new global shortcut.

```text
┌ Chief · global coordination ─────────┐
│ Paused · 1 decision needs you        │
│                                     │
│ > Decisions                      1  │
│   Work                           3  │
│   Workers and roles                 │
│   Chief controls                    │
│   Request summary                   │
│                                     │
│ j/k move · Enter open · Esc close    │
└─────────────────────────────────────┘
```

Use actual host state, never optimistic local state. Distinguish Off, Active, Paused,
Unavailable and unknown/error states. Paused/unavailable does not mean scheduling is off.
Attention counts unresolved decisions, not routine successful progress.

## F2: Lifecycle — approved

```text
┌ Chief controls ─────────────────────┐
│ Paused · [chief session name]        │
│                                     │
│ > Resume chief                      │
│   Disable chief                     │
│                                     │
│ Workers continue while paused.      │
│ j/k move · Enter review · Esc back   │
└─────────────────────────────────────┘
```

- Off: Select chief lists existing verified sessions, excluding active team workers.
  Never start an agent. Selection leads to review, not enablement.
- Enable review shows full session ID, canonical project and exact verified runtime.
  Explain cross-project operational status/bounded-brief visibility, not file access,
  credentials, transcripts, raw approval payloads or extra permissions.
- Active offers Pause chief and Disable chief. Paused offers Resume chief and Disable chief.
- Pause confirmation explains new chief actions and automatic wake-ups stop; accepted worker
  assignments continue. Resume revalidates identity and grant.
- Disable confirmation explains observation/dispatch access is revoked and unsent chief
  messages cancelled; workers and teams continue independently. No claim of undoing work.
- Unavailable remains explicit. Never silently replace/restart/recruit a chief.
- Every mutation opens a review, then requires explicit `y/n`. Bind confirmation to the exact
  reviewed runtime/grant epoch; changed state requires fresh review, never a replacement target.

## F3: Work approval and revocation — approved

```text
┌ Chief · work awaiting approval ─────────┐
│ Selected: 2 exact brief revisions       │
│                                        │
│ > [x] agent-cockpit / task A · rev 2     │
│   [x] other-project / task B · rev 1     │
│   [ ] other-project / task C · rev 3     │
│                                        │
│ Space select · Enter inspect brief      │
│ a review selected · Esc back            │
└────────────────────────────────────────┘
```

Review selected displays each selected item's full canonical project/task, exact revision,
approval hash, complete brief, worker role, exact prerequisite revisions and required test/QA/apply
gates, including anything not applicable. Approval-critical text is scrollable, never silently
cut off. `PgUp/PgDn` scroll; `Esc` cancels.

Final confirmation: “Approve these N exact briefs—not future revisions. y/n”. Explain this
permits beginning bounded work, not additional tool, install or Git permissions.
Freeze the reviewed selection and full content. Any change invalidates confirmation.
No approval of future backlog, automatic selection of newly arrived items or approval of todo text
merely because it exists. Empty selection cannot approve anything.

The backend currently accepts individual ApproveWork requests. Report every actual result on
partial batch failure and leave unsent items unapproved; never claim batch atomicity or retry
uncertain submissions blindly. An already-applied approval is not silently rolled back.

Revoke reviews an exact approved revision and confirms: “Pending assignments are cancelled.
Running work receives a stop-at-safe-boundary notice. Completed effects are not undone.”
Do not reinterpret revocation as release of uncertain ownership.

## F4: Workers and roles — approved

```text
┌ Chief · workers and roles ────────────┐
│ > OpenCode · project A · developer    │
│   OpenCode · project A · unbound      │
│   Claude   · project B · monitor only │
│   Codex    · project B · monitor only │
│                                      │
│ Selected: unavailable · awaiting you │
│ Source: runtime activity report      │
│ Observed: [time]                      │
│                                      │
│ j/k move · Enter inspect · Esc back   │
└──────────────────────────────────────┘
```

Inspect shows full session ID, verified runtime, canonical project, role, observation age and
every dispatch blocker. Standalone Bind role offers Designer, Developer or QA, then reviews
the exact session/project/role before `y/n` confirmation. Existing team mappings remain unchanged;
binding cannot move projects/teams or make the chief a product worker. Binding does not grant
readiness, access, live verification or dispatch eligibility. Unknown/stale availability is
unavailable. No force-ready action. Runtime reports are labelled as reports, not independently
proven activity. OpenCode admission still requires host live verification; Claude/Codex remain
monitor-only until their admission paths are verified. Preserve all scope/ownership/input guards.

## F5: Decisions and recovery — approved

```text
┌ Chief · decisions ─────────────────────┐
│ > Delivery uncertain · project A/task A │
│   Missing QA evidence · project B/task B│
│                                        │
│ Assignment: [full identity]             │
│ Brief revision: 2                      │
│ Host fact: acceptance unknown          │
│ Chief recommendation: check worker     │
│                                        │
│ Enter inspect · j/k move · Esc back    │
└────────────────────────────────────────┘
```

Ordinary detail shows concise blocker, exact choice requested, source references and a separately
labelled chief recommendation. Resolution requires a note and confirmation bound to decision
revision. A note never waives evidence or resolves a permission prompt.

Uncertain delivery offers Confirm handled (record note, retain ownership, not completion) or
Confirm safe stop and cancel (review exact worker/assignment, record why the previous worker
cannot execute it, then confirm). Unknown/unresponsive workers keep ownership blocked. No blind
retry, automatic reassignment or mark-complete shortcut. Host checks remain authoritative.

New evidence updates the same decision without moving selection or scroll position and without
repeat notifications for the same blocker. Human permission prompts stay separate, visible and
directly actionable. Critical host failures remain visible when the chief is unavailable.

## F6: Evidence — approved

```text
┌ Chief · evidence review ───────────────┐
│ Work: [canonical project/task] · rev 2 │
│ Gate: Tests                           │
│ Reviewed commit: [full commit]        │
│ Source: host-approved run [source ID] │
│ Result: exit 0                        │
│ Command and directory: [full details] │
│                                       │
│ Import this exact evidence? y/n       │
└───────────────────────────────────────┘
```

Import recorded evidence selects a protected source record, then reviews exact work revision,
gate, source/result and commit. The host resolves the source, never an agent-provided result/path.
Host attestation is a separate deliberate action: state what Theo personally attests and review
the same exact identities/result before confirmation. Label “Theo’s attestation—not independently
observed execution.” Full details scroll. Changed/mismatched records reject confirmation.

Worker prose remains an unverified report. QA attestation identifies reviewer and reviewed
handoff, not independently proven command execution. Apply identifies reviewed work and
installation outcome. No gate waiver or not-applicable toggle; no command execution from this
screen. Host completion advances automatically only after all required evidence is accepted.

## F7: Summary and shared navigation — approved

```text
┌ Chief · requested summary ────────────┐
│ As of [time]                         │
│                                      │
│ Completed: 2 · evidence references    │
│ Active:    3 · current assignments    │
│ Decisions: 1 · needs your choice      │
│ Idle:      2 · no approved work       │
│                                      │
│ Worker claims labelled unverified.   │
│ PgUp/PgDn scroll · Esc back           │
└──────────────────────────────────────┘
```

Request summary creates one bounded briefing only on explicit request. Show pending/failed
requests honestly; retain old summary timestamp if present. No automatic briefing refresh or
routine progress notifications. Distinguish accepted evidence from worker claims.

Shared keys: j/k or arrows move lists; Enter opens; PgUp/PgDn scroll details; Esc goes back one
level or closes the root and restores prior cockpit selection. Incoming events do not steal
focus or replace confirmations. Existing session drafts, human prompts and ordinary navigation
remain untouched. Changed reviewed records require fresh review.

## Builder review questions

Before handoff, verify that the host snapshot exposes the full records necessary to implement
these reviews. Do not fabricate pending briefs, evidence sources, verified identities or recovery
choices from agent prose. Identify missing data/entry paths explicitly rather than inventing UI
or authorizing backend changes. Exact nested action placement not drawn above still needs a
concrete handoff; section approval does not silently approve a builder's new interaction choices.

No source/tests/manifests/shared plans edited. No builds/tests run for this design artifact.

## Initial read-only builder check

The reviewer found five material handoff gaps at the time of inspection:

1. Pending exact briefs have no defined intake; the existing snapshot only exposes approved work.
2. Host read projections omit full approval/evidence/identity/decision/summary records required
   by these screens. Do not reconstruct trusted fields from agent prose.
3. Nested action placement and input/review/return states need concrete frames.
4. Safe-stop needs an explicit assertion, not a note interpreted as proof. Host attestation
   must match an already-approved host-attestation gate, not substitute for host-run evidence.
   Existing claimed-work revocation is refused by the backend, conflicting with the approved
   running-work stop-notice behavior; this needs implementation reconciliation, not weaker copy.
5. Scrollable frozen review and final confirmation need distinct states; the existing one-key
   `confirm` helper cannot implement scrollable review by itself.

Resolve human interaction choices one at a time; report backend gaps to the developer when the
approved task messaging link is restored. No backend work is authorized by this artifact.

### Intake choice — approved after builder check

Theo chose A: agents prepare proposed exact briefs, including roles and completion gates, for
human review. This is proposal intake only; proposals confer no work/permission authority.
The existing backend lacks that intake contract; do not pretend existing approved-work snapshots
are proposal records. Host canonical identities, revision freshness and computed approval hashes
must be validated independently of untrusted proposal content.

### Nested action locations and review states — approved after builder check

Theo approved this navigation tree, including renaming the root's Approved work row to Work:

```text
Chief
├ Decisions
│ └ Selected decision → Resolve / Recovery
├ Work
│ ├ Awaiting approval → select proposals → Review selected
│ └ Approved → inspect revision → Revoke / Evidence
│                                  └ Import / Attest
├ Workers and roles → inspect worker → Bind role
├ Chief controls
└ Request summary
```

Every sensitive action has distinct review and confirmation states. Full frozen review uses
PgUp/PgDn to scroll, Enter to open confirmation and Esc to return without submission. `y` while
browsing has no effect. Only `y` in confirmation submits; `n` or Esc returns to the same review.
Changed identities/revisions/evidence invalidate confirmation and require fresh review, rather
than silently refreshing frozen content. Batch review is one continuous scrollable document
containing every selected full brief, not only a row-count summary.

Return to the originating item after success or cancellation, preserving its position where
it still exists. Show failures there. If an item disappears, do not retarget its action to the
next row: return to its parent list with no actionable replacement selected until human movement.

### Safe-stop and evidence safeguards — approved after builder check

Safe-stop cancellation requires a nonempty note and a separate initially unchecked assertion:

```text
Assignment: [exact assignment / worker / revision]
Reason: [your note]

[ ] I confirm this worker can no longer execute
    this assignment.

Space toggle · Enter review · Esc cancel
```

The note never checks the assertion automatically. Unknown/unresponsive workers remain blocked;
the chief cannot attest. Submission binds the explicit assertion and note to the reviewed
assignment/revision. Host enforcement is still required.

Evidence Attest is available only for a gate already approved with source `host_attestation`.
It cannot substitute for `host_run`, `qa` or `reviewed_apply`. Review Theo's assertion, result,
exact work revision, commit and gate before confirmation. Missing required host capability
renders its control unavailable with a reason, not a fake-success fallback. Missing required
capabilities block shipping the complete feature; these are not optional deferred features.

## Concrete state contract for the builder

The following small mechanics make the approved tree implementable. They are designer choices
in this assembled document, not additional claims about earlier individual approvals; final
full-design approval covers them.

- List and action-menu screens use j/k or arrows, Enter to open, Esc to parent. Detail screens
  use PgUp/PgDn and Enter to open their labelled action menu. Footers always identify the current
  meaning of Enter. No mutation occurs merely by opening an action menu.
- Work opens a two-row menu: Awaiting approval, Approved. Proposals show sender provenance
  labelled agent proposal; protected host validation supplies canonical identities/revision/hash.
  New or edited proposals never inherit selection/review consent. No inline brief-authoring
  editor or approve-all-future option. Requesting a new proposal is outside these controls.
- Awaiting approval uses the F3 selection keys. Enter inspects the full proposal; Esc returns
  to the same selection. `a` opens frozen batch review. No new rows join a frozen batch.
- Approved lists exact approved revisions; Enter opens full detail, then Enter actions offers
  Revoke and Evidence. Evidence opens required gates; Enter opens selected gate detail, then
  its action menu offers Import recorded evidence and Attest evidence with availability reasons.
- Import opens a host-owned source-record picker scoped to that work/gate; selecting a record
  opens F6 review. No arbitrary path/source-ID text entry. Include failed/mismatched source
  records with their honest status; they cannot be imported as passing evidence.
- Attest asks for a nonempty single-line note using the existing text-entry convention (Enter
  finishes, Esc cancels), then presents an initially unchecked “I attest this exact gate passed”
  assertion with Space toggle, Enter review and Esc back. Work/gate/commit come from the approved
  record and are read-only. The reviewed result is `passed: true` with that note, explicitly
  host-attested. Unchecked assertion cannot advance. No result JSON editor or inferred pass.
- Worker detail actions offer Bind role only for eligible standalone sessions; team-bound
  sessions display their existing mapping and why standalone binding is unavailable. Role
  picker order is Designer, Developer, QA. Selection opens exact binding review.
- Lifecycle Select chief opens a verified-session list, then exact enable review. Unavailable
  grants show their retained status and offer Disable, not automatic replacement; a new
  selection is possible after explicit disable. Empty eligible-session list never spawns one.
- Decision detail actions offer Resolve with note, or Recovery for an uncertain assignment.
  Recovery menu offers Confirm handled and Confirm safe stop and cancel. Handled asks for a
  nonempty note then opens review. Safe stop asks for a note then the explicit checkbox above,
  then review. Ordinary Resolve similarly asks for a nonempty note then review. Note entry is
  not confirmation. Missing required host recovery validation makes the action unavailable.
- Enter in single-line note entry accepts the note, not the action. Esc cancels note editing
  and returns to its originating action menu, with no host mutation. All mutation confirmations
  use the distinct frozen-review → confirmation → submit flow, not the existing one-key
  `confirm` helper as a substitute for review.
- Request summary opens pending state, then the resulting read-only F7 summary. Enter does not
  generate another request there. To request another, return to root and explicitly choose
  Request summary again. Display request failure without losing an older timestamped summary.
- q has no new Chief-specific binding. Tab/BackTab retain existing panel navigation semantics;
  they never approve an action. Existing permission/input surfaces retain precedence, and
  returning to Chief restores its local state without submitting a suspended confirmation.
- Short rows may abbreviate identity for space, but full details always expose full escaped
  values with scrolling. Escape terminal controls in every agent-originated field. Narrow
  screens wrap details and keep navigation/confirmation accessible; never hide consent content
  behind a falsely complete preview. Empty lists explicitly say nothing is available, not ready.

## Host data requirements / integration blockers

These are read requirements, not invented endpoint names or authorization to implement them:

- Pending immutable proposal records with provenance; full protected reviewed brief fields,
  canonical work identity, host-validated revision/hash and freshness/invalidation information.
- Full verified chief/worker identity, grant/binding epochs, team mapping, observation age,
  provenance and all eligibility blockers. A short session label is insufficient.
- Durable decisions/revisions, assignment identities/revisions/ownership/recovery state and
  source references, independently visible critical host errors.
- Gate requirements, exact commit, protected evidence source records and import/attestation
  applicability; summary request state, timestamp, content and evidence references.
- Running-work revocation must meet the approved stop-notice semantics. A refusal to revoke
  claimed work is a visible integration gap, not permission to advertise that running work stops.

The developer must reconcile these requirements against the current backend, which is changing
in parallel. Do not infer missing trust fields from proposal prose or write host SQLite from UI.
UI submits only the existing/explicitly reviewed host contracts; host validates again at action
time. Read/data gaps block affected controls and feature completion, not review of the design.

## Required screen/interaction test evidence

Task 7 plan remains untouched; these requirements are handoff material for its implementer:

1. Exact brief/role/gates/prerequisite/hash shown before approval; changed revision, newly arrived
   proposal or changed selection cannot inherit prior confirmation. Batch partial failure shows
   each result and never claims atomic success. Empty selection sends nothing.
2. Exact verified chief/worker/runtime/epoch review; pause/unavailable distinct from Off;
   disable explains retained worker activity; no automatic chief replacement or readiness override.
3. Source-labelled decisions preserve selection/scroll and human prompts; safe-stop requires
   explicit unchecked-to-checked assertion plus note and exact assignment review. Unknown ownership
   never frees itself; resolving a note cannot waive evidence gates.
4. Import selects protected source and shows exact revision/gate/commit/result. Host attestation
   is limited to an approved host-attestation gate and explicit assertion, never a worker report.
5. Root/parent/cancel paths and text input keys match this document. Enter does not mutate from
   detail/review; y only submits in confirmation; stale confirmation and disappearing rows cannot
   retarget. No focus stealing, draft mutation or silent confirmation after returning from prompts.
6. Escaped controls, long canonical IDs/briefs, narrow screen/scroll, missing records, empty lists,
   storage/delivery errors and missing capabilities remain legible and fail visibly.
7. Summary request creates one bounded request, retains timestamp/provenance and has no automatic
   regeneration or progress notifications. Success progress never increments decision attention.

## Builder recheck

The read-only reviewer rechecked this assembled artifact after the follow-up choices and
reported: “Nothing important left.” All five original interaction questions are answered;
missing host capabilities remain explicit integration blockers. Final full-design approval
was subsequently granted by Theo. This review does not claim implementation, tests or live
verification passed.

## Approved developer handoff

- Approved references: F1–F7, the follow-up intake/action/review/safety decisions and the concrete
  state contract in this artifact. These jointly define the approved interaction, not the initial
  browser alternative alone.
- The implementation owner should reference this artifact and transfer its screen/interaction
  test requirements into Task 7 through that owner's authorized workflow. The Designer has not
  modified the shared implementation plan or source/tests.
- Host-only pending proposal intake, full review projections, source listing and claimed-work
  revocation semantics must be verified against the current parallel backend work. Missing
  required capability remains a feature-completion blocker; UI never substitutes agent claims.
- No dependency changes, product source/tests, host scripts, Git writes or application/deployment
  were performed by the Designer. The feature todo remains open pending implementation,
  verification and reviewed apply. No product tests or live checks were run for this design.
- At final approval, `cockpit-team context` returned `task: null` and `members: []` for this
  Designer session. Automated delivery to the original developer is blocked by the absent
  approved task messaging link; do not relink or infer new permissions from this artifact.

## Approved exact-target recovery extension — final fix wave

Theo explicitly approved extending the existing Recovery interaction to exact launch/message
IDs, including failed launches without a known worker. This retains the same note entry,
initially unchecked safe-stop assertion, full frozen target review, and separate `y` confirmation.
Review shows the immutable launch token or message ID, current revision, project/task and known
recipient identity; an unknown worker remains explicitly unknown, never an invented assignment.

For an unknown launch the assertion is “this exact launch can no longer execute.” For a delivery,
safe stop covers that exact delivery and its recipient work. Handled retains work ownership;
handled coordination removes only its transport blockage. Chief-wake handling tombstones the
delivery without claiming native admission, opening an allowance, or automatically retrying it.
Neither choice counts as completion or verified delivery. Host identity/revision checks remain
authoritative, and later receipts cannot revive a settled target.
