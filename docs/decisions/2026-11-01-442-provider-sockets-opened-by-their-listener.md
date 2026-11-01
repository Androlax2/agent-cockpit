# The daemon opens a session's provider socket only when the session's own process tree listens on it

Issue #442. Decided by the worker within Theo's instruction on the issue: the daemon must
never follow an agent-writable path to a socket.

- **The listener is checked, not the path.** `#437` makes the session's directory writable
  by the agent, and Codex 0.161.0 leaves `codex.sock` there as a link into
  `/tmp/codex-daemon-<uid>/` as the sandbox sees it (#446). No path check can tell the
  session's app-server from a link the agent planted, to the owner's unsandboxed app-server,
  another session's, or any socket the daemon may open. So `Provider::deliver` takes a
  `DeliveryTarget` whose `SessionSockets` opens the socket, and the daemon's
  `SessionTreeSockets` keeps the connection only when the kernel's peer process descends from
  the session's agent root (`cockpit-agent-channel::ancestry`, the root recorded at launch).
  Nothing is written before that check.
- **A listener the agent starts in its own tree is accepted.** It already holds the
  conversation the daemon delivers to it; the exposure was reaching processes outside it.
- **Rejected:** a daemon-owned socket directory passed to Codex (Codex picks the real
  socket's place under `/tmp/codex-daemon-<uid>/` itself, measured on 0.161.0, and the
  sandbox must write it); opening with `O_NOFOLLOW` alone (the link is how Codex publishes
  the socket, and the folder it points into is the agent's to write, #446).
- **The connect waits 5 seconds at most** (`CONNECT_TIMEOUT`), so a listener that never
  accepts cannot hold a delivery. The connect itself reaches whatever the path names before
  the check; nothing is written.
- **Left:** the kernel records a listener's pid when it called `listen()`; that process may
  have exited and its pid been reused inside the session's tree, and the pid is read after
  connecting (#448, `SO_PEERPIDFD`). The daemon wiring of provider delivery is #431.
