# Approvals: digest, redaction and pseudo-terminal dependencies

Issue #34. Decided by Theo (option A): `cockpit-approvals` takes three pinned dependencies
instead of hand-written replacements.

- **`sha2 = "=0.10.9"`** computes the action digest an approval is bound to (SHA-256 over
  the length-prefixed working directory and command). It pulls `digest 0.10.7` and
  `block-buffer 0.10.4`.
- **`regex = "=1.13.1"`** carries the legacy redaction patterns (token shapes, named
  secrets, credentials in URLs and headers) and the terminal-control stripping unchanged.
- **`rustix = "=1.1.6"`, features `pty` and `termios`**, opens the pseudo-terminal
  (`openpt`, `grantpt`, `unlockpt`, `ptsname`, `tcsetwinsize`) without `unsafe`. The
  features **`process` and `std`** were added beyond the approved pair: `process` provides
  `setsid` and `ioctl_tiocsctty` for the session leader, `std` the `OwnedFd`/`OsString`
  conversions. `cockpit-processes` already enables both on the same pin.
- `cargo deny` (`make crates-deny`) stays green with these pins.
