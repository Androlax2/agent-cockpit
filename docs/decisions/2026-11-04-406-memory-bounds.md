# The daemon's feed buffers, its session lock cache and the tmux history are bounded by `[memory]` owner settings; a session with no agent is reported, never ended

Issue #406 (memory bounds). Decided by the worker, reasons below.

- **Three owner settings, read at the daemon's start** (`cockpit-config`, `[memory]`):
  `tmux_history_lines` (10,000), `feed_buffer_kib` (1024) and `agentless_session_minutes`
  (10). Owner-only: a project file that sets them is refused, since they size the owner's
  machine, not a project.
- **tmux history: 10,000 lines per pane, was 50,000.** Measured with tmux 3.8, four panes
  of 60,000 coloured 200-column lines: 6.4 MiB a pane at 2,000 lines, 24.8 MiB at 10,000,
  116.3 MiB at 50,000 (plain text: 3.4, 9.8 and 41.8 MiB). Twenty agents with full history
  held 2.3 GiB before and hold 0.5 GiB now. The limit is the one fixed line of the tmux
  config that became a parameter (`cockpit_tmux::config`, `ServerSpec::history_limit`).
- **tmux 3.8 applies a changed `history-limit` to panes already open**, raising and
  lowering, and trims at once when lowering. The #308 decision says it applies "to panes
  opened afterwards"; that is all the supported range (3.6 and later) guarantees, so the
  tests assert only that (`cockpit-tmux/tests/sessions.rs`). The first start after this
  change therefore trims open sessions' scrollback beyond 10,000 lines on tmux 3.8.
- **Change feed: a byte budget per subscriber beside the 8-batch channel.** A batch is
  encoded to its wire line where it is built and holds its length of a per-subscriber
  semaphore until the connection wrote it (`cockpit-daemon/src/feed.rs`). The feed waits
  for room; `Feed::next` then reads everything committed meanwhile as one batch, each entity
  once, so a stalled subscriber costs the buffer, not a batch per commit. Measured (debug
  build, 24 subscribers that stopped reading, 20 commits of 16 states of 32 KiB): 147 MiB
  held before (6.1 MiB a subscriber), 50 MiB after (2.1 MiB: the 1 MiB buffer, the batch
  waiting for room, and the one being written). A batch longer than the whole buffer takes
  all of it and passes alone, so the frame limit (16 MiB) stays the only size refused.
- **No cap on the number of subscribers.** One feed per connection and 64 connections per
  socket already bound it.
- **Replay reads commits 256 at a time** (`cockpit-messaging/src/feed.rs`), not all of a
  cursor's backlog in one list; the batch built from them is unchanged.
- **The session lock map drops entries nothing holds**, at each periodic pass. It was the
  one daemon cache that grew for the daemon's life (one entry per session ever touched).
  An entry is held while its `Arc` or its mutex's `Arc` has another owner; both are read
  under the map's lock, which every new holder takes first.
- **A session left running with no agent is reported, not ended.** After the window the
  periodic pass opens a dialog question on the session, through the path #702 uses for a
  launch that never reports, so every client that shows reports shows it as waiting on
  the owner, with no new wire field. The text names `torlek session end ID`. What still runs
  may be the owner's dev server or build: ending it is their call.
- **The pass looks at terminals after it released every session lock.** The tmux calls
  take long enough that an owner's send or cancel would find its session busy. The report
  is written only for a session still running that launch, read in the writing
  transaction, as an agent's own report is written without the lock.
- **Rejected:** dropping batches or closing a slow subscriber's connection (a viewer on a
  suspended laptop would resnapshot at every wake; waiting loses nothing); ending agentless
  sessions automatically (kills work the owner may want); a new `SessionView` field for the
  agentless state (a second way to say "waits on the owner"); persisting when a session was
  first seen agentless (a restart only delays the report by the window).
- **Not covered:** an agent's leftovers that keep its terminal open leave the pane
  "running" for tmux and are not seen.
