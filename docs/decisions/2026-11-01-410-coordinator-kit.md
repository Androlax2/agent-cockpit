# A coordinator session observes, messages and starts sessions under three owner grants

Issue #410 (Alpha 1 foundation of #208). The points below were decided by the implementing
session, within reports 07 and 17. The TUI line was chosen by Theo.

- **Three grants, each its own permission, none implied.** `observe-sessions`,
  `message-sessions` and `start-sessions` are `Permission` variants that no other grant covers
  (`Permission::COORDINATOR`). Only the owner socket gives or revokes them
  (`cockpit session grant|revoke SESSION PERMISSION`). Each is given over `Scope::Owner`:
  over every session. Observe and start are evaluated over the owner scope, message over the
  target session, so a narrower grant can come later without a wire change. A session's
  coordinator grants are revoked when it ends, like its other grants.
- **Start terms live on the grant.** By default each start waits for the owner, at most 10 per
  hour. `--without-confirmation` needs an explicit `--per-hour N`, so an unconfirmed grant
  always names its cap. The cap counts the holder's `start` audit rows in the past hour,
  confirmed or not, failed or not. Regranting with other terms replaces the grant and keeps
  the count.
- **A confirmed start is a `start-session` request in the `ask run` queue.** The owner
  approves or declines it like a command (`cockpit ask approve|decline`, the TUI queue).
  Approving it starts the session through `cockpit-sessions`, never through a shell. A daemon
  restart marks a pending start not run, like a command. Approval re-checks the grant the
  request was asked under: a grant revoked since refuses it. Recording the approved start's
  end is tried 5 times before it is only logged; a daemon that stops between starting the
  session and recording it leaves the request `not-run` while the session runs (a known
  window, logged).
- **The audit is `cockpit-authorization`'s `authorization_audit` table.** Each row keeps the
  actor session, the permission, the grant and its revision, the action, the target session
  and a detail. Actions are `message` (target: the worker, detail: the text), `start` (asked
  or made, the one the cap counts; detail `approval request N` when it waits) and `started`
  (target: the new session). Reads under `observe-sessions` are not audited. The owner reads
  it with `cockpit session grants SESSION`.
- **Messages go through the `cockpit-messaging` outbox and are typed at the next prompt.**
  The provider-native delivery is #431. Until then, the daemon queues each message as an
  envelope (producer `coordinator`, effect key `coordinator-message:<audit id>`, never
  redelivered). A delivery loop submits it with `Sessions::submit` as a collaborator's
  message, prefixed `[Cockpit] Message from session S (a collaborator, not the owner):`. A
  session with an open dialog refuses the message with a typed reason when it is asked. A
  dialog that opens after it was queued only delays delivery: the loop retries every 2 s,
  doubling up to 30 s, and gives up after 24 hours. Before typing, the loop checks that the
  sender still holds `message-sessions` over the target: a revoke, or a sender that ended,
  fails the messages it left queued instead of typing them. A session gets its messages in
  the order they were sent: one waits while an earlier one to the same session is queued.
- **The card shows a plain line until the card has a design** (Theo's choice):
  `grants observe,message · ✉ SENDER (DELIVERY): "TEXT"`, built from
  `SessionView.coordinator_grants` and `SessionView.inbound_message`.
- **`cockpit agent sessions --watch` polls every 2 s.** The agent socket has no change feed
  yet. It prints the listing again only when a line changed.
- **A coordinator cannot message itself or observe itself.** `agent sessions` leaves the
  sender out; `agent message` to the sender is an invalid argument.
