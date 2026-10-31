# Tasks: who files what where, and the owner settings that bound it

Issue #38. Decided by the worker, within the issue's scope; each choice keeps the legacy
behaviour where one existed and fails loud where it did not.

- **Agents file through the agent socket, authorized by `change-state` over themselves.**
  Launched sessions hold that grant (`2026-10-31-31-self-state-grant.md`), so no new
  permission is needed; `cockpit-tasks` evaluates it on every session request. Approving,
  declining, editing, priorities, the queue, waits, images and drafts stay the owner's.
- **`cockpit todo` picks its socket from `COCKPIT_LAUNCH_TOKEN`.** A launched session's
  verbs go to the agent socket; without the token they are the owner's. `propose` and
  `blocked` are a session's only and always go to the agent socket.
  **Rejected:** a `--as-agent` flag (an agent could forget it and hit a socket its sandbox
  hides) and trying both sockets (the answer would depend on what happens to be mounted).
- **The blocker project and the cross-project cap are owner settings** (`[tasks]
  blocker_project`, `cross_project_open_max`, default 3), refused in project config. The
  daemon reads them when a filing arrives; a config that does not load refuses the filing.
  Without a blocker project, blocker reports are refused for the owner to configure.
  **Rejected:** a hard-coded `dotfiles` target (the legacy rule) because the project names
  are installation-specific.
- **A session's project is the registered checkout holding its working directory,** as for
  approvals, until sessions record their checkout (#31). Every project a request names must
  be registered.
- **Filing in another project queues the item there and counts against the cap per
  (filing project, target project), over open non-blocker items.** Blocker reports go
  where the owner configured and are not capped.
- **Decline memory is the declined row itself,** matched on the title with case and spacing
  ignored, and applies to every session filing in that project, not only proposals.
  **Rejected:** a separate memory table, which could drift from the items it describes.
- **Images are the owner's, typed from their bytes (PNG, JPEG, GIF, WebP), at most 4 MiB
  and 20 per item, sent as lower-case hex** so a frame stays one JSON line within
  `MAX_FRAME_BYTES`.
- **`next_action` explains the item, not the queue.** It says what has to happen for the
  item to move; why the scheduler has not placed a queued item is `cockpit-scheduling`'s
  explanation (#43).
- **Summaries are stored on the finished item and shown by the items waiting on it.**
  Delivering them to the filing session waits for messaging (#41).
