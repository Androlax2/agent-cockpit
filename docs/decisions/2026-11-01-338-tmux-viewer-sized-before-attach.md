# The viewer gives an agent's window its own size before the nested client attaches

Issue #338. Decided by the worker, from the end-to-end tests below.

- **Resize first, then attach.** `Server::show_in_viewer` reads the viewer pane's size,
  runs `resize-window` on the session's window to that size and then `respawn-pane` with the
  nested client, in one tmux command. A session starts at the size its launch names (200x50),
  wider than the viewer; resized while the client attached, tmux rewrapped a line wider than
  the viewer and the client drew a screen that lost or shuffled rows (cut short after the
  wrapped line on tmux 3.6, from the second line on macOS; rows differing on tmux 3.8, about
  one run in 14). The attach now changes no size. Proven by `cockpit-tui`
  `e2e new_session::the_viewer_shows_every_line_after_one_wider_than_itself`, which compares
  the viewer row for row with the agent's own pane.
- **`window-size` is unset right after.** `resize-window` sets the window's `window-size` to
  `manual`, which would pin it; unsetting it lets the window follow the viewer again
  (`the_shown_agent_follows_the_terminal_when_it_is_resized` fails without it).
- **Rejected:** starting sessions at the viewer's size. Sessions start before any viewer
  exists, the CLI starts them with no viewer at all, and the viewer's size changes with the
  owner's terminal.
