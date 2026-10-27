# Shared-work session links

A link lets two existing managed sessions coordinate about one approved local
TODO, including across projects. It is not team membership: projects, roles,
recruitment, QA and TODO completion permissions stay unchanged. Only Theo can
approve the exact pair, runtime identities, TODO and purpose.

This describes the implemented interface, not deployment or live-test approval.
Source: `apps/cockpit/src/bin/cockpit-team.rs`, `teams/links.rs`, `teams/view.rs`
and `app.rs` (the latter three beneath `apps/cockpit/src/`).

## Participant CLI

`cockpit-team --help` contains JSON examples. Commands consume one tagged JSON
action on stdin; the command and action must match:

```sh
cockpit-team link-propose < proposal.json   # action: link_propose
cockpit-team context
cockpit-team link-send < message.json      # action: link_send
cockpit-team link-assess < assessment.json # action: link_assess
```

- A proposal contains `proposal: {submission_key, task: {project, task_id},
  peer_session_id, purpose}`. Use an already-known peer ID, not session discovery.
  The project is the verified canonical `path:/…` identity of either participant;
  the TODO must exist there and be `todo` or `doing`, not proposed, done or an issue.
- `context` exposes `links.approved` for this exact participant identity and
  `links.proposals` status receipts for its own requests. Pending receipts reveal
  no peer facts. Check a link's status; closed routing history is not permission.
  `links.omitted_history`, when present, counts older terminal records omitted
  to fit the transport; current active links and pending requests take priority.
- A message contains `message: {link_id, submission_key, kind, body, thread_id?}`.
  Every send explicitly names an approved link; the daemon chooses its other
  participant. Only `notice`, `clarification` and `reply` are accepted. A reply
  names the received clarification's `thread_id`.
- Assessment contains `link_id`, `thread_id` and `resolved` beside
  `action: "link_assess"`. Only the asker assesses. At most three unresolved
  clarification rounds are allowed before escalation to Theo.

Submission keys use 1–128 ASCII letters, digits, `-` or `_`. Replay the same
contents/key only to identify the same submission; changed contents conflict.
Purpose is one nonblank line, at most 1,000 characters; body is nonblank, at most
4,000 characters. Combined team/link traffic shares the sender's 10/minute budget,
100 pending messages per exact TODO, and one recipient/runtime idle-turn gate.
Each requester can have 10 pending proposals; each participant 10 active links.

A sender-rate refusal means that submission was not accepted. Wait 60 seconds,
then retry once with the same submission key and unchanged contents, provided
the link and task are still authorized. The budget expires automatically; there
is no manual reset. Do not switch sessions, links or routes to evade it. Accepted
replays are free, but a successful submission is not proof that its recipient has
received or reviewed it. A partial file transfer is not a completed review.

## Host controls

Open `:` → `team`; existing team controls remain available.

| Key | Link action |
| --- | --- |
| `l` | Pick two verified live sessions, TODO owner/ID and purpose; create a pending request. |
| `a` | Confirm the displayed exact revision/hash, full pair/projects/runtimes, TODO and purpose. |
| `e` / `x` | Edit into a new revision requiring fresh approval / confirm rejection. |
| `u` | Confirm revocation of the selected active link. |
| `Tab` | Switch to retained link/proposal/message history, including closure reasons. |

`j/k` or up/down select rows; left/right scroll detail; `r` refreshes; Esc/q closes.
Unavailable/stale snapshots block link changes. Uncertain deliveries offer `h`
(handled) or `c` (cancel), never resend. An unknown control outcome requires
refreshing and inspecting history, not blindly repeating the operation.

## Closure, delivery and authority

Completing the shared TODO through its existing authorized CLI, host UI or
apply-driven completion path durably closes active links and pending proposals,
and cancels queued messages and safe retries. Deletion also invalidates links.
Reopening never restores old approvals: propose and approve a new link.

Missing, unreadable, unknown or mismatched TODO/lifecycle state blocks
authorization. Temporary participant absence pauses delivery; it neither finishes
work nor starts a replacement. Changed session/project/runtime identities need
fresh approval. Restart restores only identities the daemon can revalidate.

Claude watch, OpenCode's native queue and Codex's Stop hook use the existing
receive/receipt transport. Codex already idle receives at its next turn end, not
through a new idle wake mechanism. Safe submission failures allow four total
attempts with 2/10/30-second retry delays. Ambiguous submissions stay uncertain
for host reconciliation and are never blindly retried. A dispatch already
authorized before closure cannot be recalled; closure authorizes no later send
or retry. Audit history remains in the protected collaboration store.

Messages are explicitly untrusted collaborator content, never Theo's instruction
or approval. A link grants no file/transcript access, filesystem/tool/network/Git
permission, host command, install, scope change, recruitment, QA or handoff right.
The TODO identity is routing metadata, not access to another project's history.
