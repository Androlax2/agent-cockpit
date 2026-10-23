# Team members survive cockpit sleep/wake

Todo: 491ae1b6

## Intent and scope

Idle Claude and opencode teammates should free memory just like other managed sessions, and
wake in the same conversation, task and role. Automatic reattachment applies **only to
cockpit-managed sleep/wake** (the recommended scope Theo accepted). Normal restarts and
manual conversation resumes retain the existing fresh-approval requirements.

No new UI, permission grant or team recruitment flow. The `s` key and automatic sleep use the
same lifecycle. Existing draft/background-work, terminal-session and resumable-conversation
checks stay. Unknown membership still means stay awake. Codex remains unsupported for sleep.

## Current constraints

- `members.runtime` is a verified host process identity, not just a conversation ID.
- The daemon derives identity from peer credentials and processes outside the sandbox.
- `store::recover` cancels queued messages when a different runtime appears.
- `messages.recipient_runtime` also pins delivery to the old process.
- A fresh QA handoff can replace a teammate; a revoked member must never regain membership.
- Uncertain deliveries are never automatically retried.

Removing `SleepBlock::Team` without addressing these constraints is unsafe.

## Proposed lifecycle

### Prepare before exit

The host-side `cockpit sleep` requests a sleep handoff through the existing private team host
socket. The sandbox-facing `AgentAction` API gains no lifecycle authority. The host request
must succeed for every sleep, including non-team sessions: it establishes that membership
was checked, rather than assuming there are no teams when the daemon is unavailable.

For a member, the daemon records a random one-use resume permit in its protected team database,
bound to the session ID, agent, canonical project, original checkout, old runtime, task, role and
membership epoch. Preparation is idempotent for the same current member/old runtime and rejects
conflicting permits. The host retrieves the permit; sandbox requests cannot retrieve or issue it.

Preparation refuses sleep while the member has queued/ready/submitting/uncertain team messages
as sender or recipient. That avoids losing delivery state and avoids needless sleep/wake churn.
It does not cancel messages, change epochs or revoke membership. The preparation transaction
also pauses new delivery claims and outgoing sends for this member. Incoming messages may still
be queued. This closes the prepare-to-exit race: no new native delivery can start after the
pending-delivery check. Cancellation resumes normal claims/sends; receipt handling for existing
attempts retains its normal authorization rules.

The sleep note records the permit alongside the existing resume metadata, written atomically
before exit. The permit is not logged or exposed by phone/team snapshots. A non-member gets
an explicit successful response with no permit. Last-moment pane safety checks still run before
exit. If they refuse sleep or exit genuinely fails with the process alive, cancel the permit and
remove the sleep note. If the daemon is unavailable at any of these boundaries, do not invent
successful preparation or discard the only resume metadata for a process already gone.

### Launch the authorized replacement

`cockpit wake` passes the permit only to the new host launcher environment, using the same
verified-ancestor approach as `COCKPIT_TEAM_SPAWN`. Agent JSON claims and environment changes
inside the sandbox cannot supply a trusted permit. Claude and opencode detached restore paths
must preserve this value on the outside-sandbox launch process.

The permit's security property is trusted provenance, not confidentiality from the resumed
agent: inheritance into its sandbox is acceptable, as with the existing spawn token. A token
copied into agent JSON or a different sandbox process's environment grants nothing. Only the
value recovered from the verified outside-sandbox launcher is considered.

The daemon reads the permit from that verified host ancestor when the replacement reports.
Before serving team context, claiming messages, or running startup recovery, it verifies:

- Same session ID, agent, canonical project and original checkout.
- Old runtime is dead; replacement runtime is verified and different.
- Original membership still exists with the same task, role and epoch.
- Permit is unused, unexpired and corresponds to the host sleep record.

Then one database transaction updates the member runtime and queued recipient messages' runtime,
clears the old idle-turn marker, and consumes the permit. Membership epoch stays the same:
this is continuation of the approved member, not a new approval. Already received messages are
not resent; attempts/receipts are not reassigned to a new process. Repeated reports from the
same already-bound replacement are harmless; a second process cannot consume the permit again.

For team wakes, the sleep note is retained until the daemon acknowledges committed rebinding;
the existing early removal after a detached launch is not sufficient. The restart note continues
to hide the asleep card during this interval. A durable consumed-permit record identifies the
winning replacement so acknowledgement can be repeated after a crash without rebinding a second
process. If the replacement never reports, timeout leaves the sleep record and messages intact
with an explicit error, not an automatic second launch. Late reporting must not authorize a
different runtime after the handoff has been cancelled or revoked.

Rebinding happens before `store::recover` can cancel messages for a runtime change. New traffic
queued while the member slept is preserved, including after a daemon restart.

Revocation, replacement, explicit exit and the existing seven-day sleep expiry invalidate the
permit. The daemon checks both the protected sleep record and the current membership before
each automatic wake and before committing a rebind. Removing the note is a durable cancellation
even if the daemon is unavailable: a remaining database permit without its matching sleep note
is unusable and is reconciled as cancelled. Explicit exit also uses the existing ended-sessions
record. Note cleanup after successful binding is distinguished by the consumed-permit record,
not mistaken for revocation of an already successful continuation. An old note or token cannot
restore membership after cancellation or a membership epoch change.

`r`/`cockpit restart` on an already-asleep card keeps its existing meaning: wake, with this
handoff. Restarting a live process or manually resuming history does not pass a permit and
therefore cannot auto-reattach.

### Wake on team traffic

A valid queued team message to a sleeping member causes the daemon's periodic host work loop
to start `cockpit wake <session-id>` detached. It only wakes a member with a matching authorized
sleep note/permit and a dead old runtime. It does not type a message into the pane: delivery stays
on the existing native inbox / Claude watch / receipt path after verified reattachment.

Concurrent message/open/phone wakes reuse the existing wake lock. Automatic wake attempts are
bounded and tracked (one launch in flight per permit, with an explicit failed state after the
existing restart timeout), not relaunched on every snapshot poll. Failure keeps the queued
message and shows a team error; it does not claim delivery or start a fresh conversation.

Messages that are expired, cancelled, membership-revoked or delivery-uncertain never trigger
an automatic wake. Existing turn/readiness checks still decide when delivery is safe.

### Enable eligibility only after lifecycle support exists

Remove the unconditional team-member exclusion in `sleep::block` and `App::sleep_now` only once
preparation/rebinding/wake-on-message work. Retain the unknown-membership guard. Apply existing
non-team safety checks to teammates; no teammate with active work is interrupted.

## Regression requirements

- Team preparation available only through the host socket; sandbox attempts rejected.
- Claude and opencode wake in the same role/task with the approved epoch and a new runtime.
- Ordinary restart/manual resume cannot reattach automatically.
- Forged token, wrong project/checkout/agent/session, live old runtime, replay and expiry rejected.
- Revoked/replaced members cannot return through stale sleep permits.
- Pending or uncertain delivery blocks preparation; a message arriving during sleep stays queued,
  wakes its recipient once, and is delivered once through the existing receipt machinery.
- Startup recovery preserves authorized sleeping members' queues before rebinding.
- Failed exit cancels preparation; failed wake keeps messages and reports failure.
- Manual `s` and automatic sleep permit known teammates but refuse unknown membership.
- Existing non-team sleep, team authentication, handoff and message tests still pass.

## Builder decisions (proposed, not separately requested by Theo)

- Use a single-purpose permit table and the existing host socket, not a new service or generic
  runtime-rebinding API. Follow existing `spawns` token verification and protected database rules.
- Permit lifetime is the existing seven-day sleep-note lifetime, not an indefinite approval.
- Retry only failures known to occur before a wake launch; ambiguous launch outcomes wait for
  reconciliation or timeout. Preserve the no-duplicate-delivery rule.
- Keep host-issued permits out of agent response JSON and normal logs (the verified launcher
  environment may be inherited into the resumed sandbox). The daemon may
  expose a boolean sleep/wake/error state to the column, never the permit itself.

## Not included

Sleeping arbitrary restarts, giving agents host control privileges, changing the host DNS or
greywall pin, new UI layouts, and weakening runtime authentication are outside this task.
