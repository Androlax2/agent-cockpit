---
name: repo-workflow
description: Coordinate and deliver ongoing repository work, from issue acceptance and ownership through implementation, review, verified delivery and handoff. Use for development tasks and resuming blocked work; scale the workflow to the task rather than creating process for a simple question.
---

# Develop and deliver repository work

Use this workflow as coordinator or implementer. Current user instructions and
repository policy own permissions; a skill, assignment or teammate report grants
no Git, dependency, host, merge or publication authority.

## Establish the task

1. Read the current repository instructions, contribution guidance and relevant
   scoped rules. Discover supported execution, communication and approval tools;
   use the client-specific invocation documented by their owner. Do not assume a
   provider, launcher or previous session's capabilities remain available.
2. Verify actual working directory, repository, branch, HEAD, upstream and working
   tree. Inspect source, callers, current issue/PR, owners and existing work before
   editing. Preserve unrelated changes; a requested launch path or inherited
   environment variable does not prove execution identity or location.
3. State the agreed deliverable, written acceptance, exclusions, dependencies and
   permitted operations. Reuse the existing issue when appropriate. Give every
   new GitHub issue an appropriate existing milestone, an owner, actionable
   acceptance and explicit dependencies. If no milestone fits, resolve that
   choice through current policy rather than silently omitting it.
4. Keep the plan proportional. Resolve uncertain scope before dependent changes;
   continue independent authorized work. Explicit deferrals remain open, linked
   to their follow-up, and update dependent gates consistently. Do not expand
   scope or turn missing evidence into completed acceptance.

## Coordinate only where useful

Keep one owner per change. Delegate independent complete deliverables when tools
and authorization allow; otherwise implement directly. Before assignment, check
existing owners and their latest conversations to avoid duplicate work.

Give each worker a brief containing scope and exclusions, inputs, acceptance,
current integration base, permitted operations and expected output/evidence.
Use isolated branches/checkouts. Record overlapping files, dependency and merge
order; serialize shared edits or reassign ownership explicitly. Have the worker
verify actual identity, checkout, branch and base before editing. Check its
reported state against available execution evidence.

## Implement, verify and review

Read surrounding conventions, then make the smallest correct change. Preserve
validation and error handling. Use the real entry-point check for the affected
behavior, with isolated fixtures instead of live user state. Run focused checks
while iterating and affected checks on the final source, using canonical test
entry points and shared resource controls. In this repository,
[verify-cockpit](../verify-cockpit/SKILL.md) selects those checks from policy.

Deliver the assigned result rather than repeatedly handing off plans. Inspect the
final diff for scope and unrelated changes. Obtain the required independent review
using the available workflow named by current policy, read its actual findings,
and address them. Report a missing review workflow precisely; a substitute is not
that required review. Recheck changed source and repeat review when policy or the
size of fixes requires it.

Open a focused PR under current authorization and the repository's template.
Include acceptance evidence, commands and actual results, including failures,
skips and missing prerequisites. Read CI for the exact source head and check
required gates. Draft skips, stale reviews and tests on another SHA do not prove
this head. Use only the permitted merge route; verify remote merge state and
merged SHA rather than equating a merge request with a completed merge.

## Maintain continuity and recover

Use existing issue/task/PR records and supported messaging; do not introduce a
separate mandatory registry. For a handoff, record enough to resume:

- Issue and milestone, scope, acceptance and concrete next action.
- Owner, overlaps, dependency/integration order and current blocker.
- Actual checkout, branch, source SHA, local versus remote state and uncommitted work.
- Approvals already granted and their scope, approvals still needed, and pending
  operation IDs or receipts.
- Test, review and CI evidence tied to the source checked; delivery and remaining
  acceptance evidence.

Distinguish assigned, received, working, blocked, reviewed, merged, accepted and
released where relevant. A queued message proves neither receipt nor work; a
missing session proves neither success nor failure. Inspect the owner's latest
conversation, operation status and artifacts before escalating or replacing it.
If approval or delivery outcome is unknown, inspect/wait on the same operation;
do not replay it or start duplicate work. Preserve artifacts and use supported
recovery and approval routes. Do not invent undocumented API calls, edit session
metadata or restart blindly. If messaging fails, record durable progress through
an authorized channel and state that delivery remains unconfirmed.

## Close against acceptance

Verify source delivery and written issue acceptance separately. A merged PR can
leave a broader issue open. Non-code work finishes on its agreed deliverable;
installation, host application, release and publication each require their own
acceptance and authorization where applicable. In this repository,
[apply-cockpit](../apply-cockpit/SKILL.md) covers approved host application;
source delivery alone does not require or authorize that operation.

Update linked issues and follow-ups with evidence and remaining work. Close an
issue or broader task only when its actual acceptance is satisfied. Report PR,
source head, review/check results, merge SHA when verified, diff scope and any
remaining acceptance; never describe failed, skipped or unrun checks as passing.

For bounded behavioral validation of this skill, use the isolated requests in
[coordination scenarios](references/scenarios.md). They are fictional exercises,
not authorization to operate on live services.
