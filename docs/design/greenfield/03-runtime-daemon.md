# 03. Runtime and daemon

Status: proposed; latency numbers are `validate`

## Decision

One headless **Tokio daemon per OS user** owns all authoritative operations. Frontends and
agents are clients over a Unix socket (`$XDG_RUNTIME_DIR/cockpit/`). Running agent executions
**survive ordinary daemon restarts** and are reattached on startup.

## Rules

- **Control path reserved.** Input, status, stop and approval requests have their own bounded
  capacity; bulk output, builds, provider calls and log floods cannot starve them. Stops get
  a small lane of their own (2 workers), so a stop that waits out its grace period neither
  holds a control worker nor waits behind launches (#217). A stop of a session whose own
  launch still runs is answered `conflict` at once rather than waiting for it (#230).
- **Bounded everything.** Every queue has a size; a full queue returns an explicit busy
  result. Every IPC exchange has a deadline, a cancellation meaning and an overload behavior.
- **Blocking work off the executor.** Finite blocking ops use `spawn_blocking` behind a
  semaphore; long-lived blocking workers (tmux reader, database writer) get dedicated threads.
  Started blocking work cannot be aborted; design for it.
- **Reservations outlive tasks.** A dropped Rust task or a timed-out launcher never releases a
  capacity reservation or a checkout occupancy. Only an observed workload state does.
- **Cancellation has two facts.** "Caller stopped waiting" and "operation stopped" are
  recorded separately. A permit is released on the second, never the first.
- **Live output is disposable; control events are durable.** Terminal bytes stream with byte
  bounds and gap markers for slow viewers; approvals, launch intents and outcomes are committed
  with replayable sequence ids.
- **Panics are contained** per request/worker; a worker crash is logged and restarted, never
  silent.

## Restart contract

1. Load durable launch intents, reservations and execution identities.
2. Reconcile each against the real process group / container (report 04 identities).
3. Reattach positively identified executions; terminate and confirm the rest before reusing a
   checkout; keep uncertain occupancy charged.
4. Only then accept new dispatch.

## Targets (validate)

- Input latency p95 ≤ 100 ms, p99 ≤ 250 ms, no stall over 1 s, measured over 30-minute runs
  with concurrent builds, a log flood, a stalled provider and database contention, on named
  hardware. These are hypotheses until measured.

## Rejected

- GLib/GTK event loop as the execution boundary (audit F3/F4).
- Distributed consensus or multi-node design (Restate-style): no justification for a
  personal daemon.
- Releasing permits on task drop (RAII guards for execution capacity).

## Sources

- Tokio `spawn_blocking` semantics: <https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html>
- Tokio `Semaphore` fairness and cancel safety: <https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html>
- systemd kill modes for whole-group termination:
  <https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html>
