# Desktop notifications and sounds run the desktop's own programs from a separate client

Status: accepted

Issue #62 (with the sounds of #682). Decided by the worker within the issue's terms (Linux
D-Bus first, the daemon headless, no failure without a backend) and Theo's rule that a new
crate dependency needs his approval.

- **`torlek-notify` is its own process, a client of the change feed.** The daemon gains
  nothing; a host with no desktop never runs it.
- **Outlets are programs, not crates.** `notify-send` speaks the freedesktop notification
  protocol over the session bus; `canberra-gtk-play` plays the owner's sound theme by name,
  `pw-play` or `paplay` the theme's file. Every Linux desktop has them, so no D-Bus or audio
  crate enters the build, and a host without them loses only that outlet.
- **#57 does not block it.** #57 is the Beta 1 gate (upgrades keep data, Teams and Chief);
  the notifier needs only the change feed, the reports, the queue and the network requests.
- **Rejected:** `notify-rust` (zbus) and `rodio` (ALSA, a C library on the build host), two
  dependency trees for what two programs already do; notifications in the daemon (report 19,
  decision 11); notifications in the TUI, which would stop when it is closed.
