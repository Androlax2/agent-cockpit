# Desktop notifications and sounds run the desktop's own programs from a separate client

Issue #62 (with the sounds of #682). Decided by the worker within the issue's terms (Linux
D-Bus first, the daemon headless, per-event on/off, no failure without a backend) and Theo's
rule that a new crate dependency needs his approval.

- **`torlek-notify` is its own process, a client of the change feed.** It follows sessions
  and view stamps on the feed and asks for the agents' reports, the approval queue and the
  network requests when their stamp moves, as the TUI does (#199, #706). The daemon gains
  nothing; a host with no desktop never runs it.
- **Outlets are programs, not crates.** `notify-send` speaks the freedesktop notification
  protocol over the session bus; `canberra-gtk-play` plays the owner's sound theme by its
  freedesktop name, and `pw-play` or `paplay` play the theme's file when it is missing. Both
  are on every Linux desktop, so no D-Bus or audio crate enters the build. A missing session
  bus or program turns that outlet off with one log line; a program that fails or hangs is
  logged and killed after ten seconds.
- **Five events, most urgent first:** `failure`, `hold` (an approval, a refused host, a
  provider's permission dialog), `question` (the agent's question, a guide, a command waiting
  for input), `quota` (a turn stopped by the usage limit), `finished`. Each is on or off apart
  for the desktop and for sound. Events that land together make one notification and one
  sound, the most urgent event's.
- **Told once, when raised.** The state the notifier started on is not news, nor a session
  the owner ended, nor a question a coordinator holds (#710): the TUI bell's rules (#682).
- **#57 does not block it.** #57 is the Beta 1 gate (upgrades keep data, Teams and Chief);
  the notifier needs only the change feed, the reports, the queue and the network requests,
  all on `main`.
- **Rejected:** `notify-rust` (zbus) and `rodio` (ALSA, a C library on the build host): two
  dependency trees for what two programs already do; notifications in the daemon (report 19,
  decision 11); notifications in the TUI (they would stop when it is closed).
