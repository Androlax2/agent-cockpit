# On macOS the agent channel reads process trees from `ps` and `sysctl`, in whole seconds

Issue #336. Decided by the worker: it needs neither a new dependency nor `unsafe`, and
`cockpit-processes` already reads the same sources on macOS.

- **`/bin/ps -A -o pid=,ppid=,lstart=` and `/usr/sbin/sysctl -n kern.bootsessionuuid`.**
  `crates/cockpit-agent-channel/src/process.rs` reads the whole table once per ancestry (one
  spawn, one instant) and `ps -p` for a single identity, under `LC_ALL=C TZ=UTC0` so the date
  has one format and no daylight-saving hour repeats. The start is seconds since the Unix
  epoch; Linux keeps clock ticks since boot. Identities are compared only on the host that
  read them.
- **Whole seconds are enough to tell a reused pid apart.** A pid shared by two processes
  started in the same second needs the first to exit and the kernel to hand its pid out again
  (out of 99999) within that second. A parent that started in the same second as its child
  stays in the chain, as on Linux the same tick does.
- **Each `ps` or `sysctl` has five seconds** to print and exit, then the read is
  `Unreadable` and the sender is refused; a failed `ps -A` is refused too, never walked as a
  partial table. Lines are parsed only when the walk reaches them, so a malformed line of an
  unrelated process refuses nobody.
- **Accepted: a wall clock stepped back** between a parent's start and its child's makes the
  parent look younger, and the walk stops there: the child is refused or unidentified (fails
  closed) until it is restarted. Linux's ticks since boot never step back.
- **Rejected:** `sysctl(3)` `kern.proc.pid` or `proc_pidinfo` (microseconds, no spawn): both
  need `unsafe` FFI, which the workspace denies, and the `libc` dependency. `sysinfo`
  reports seconds too; the `libproc` crate needs `bindgen` at build time.
- Proven on macOS by the `cockpit-agent-channel` tests (`identity`, `identify_bound`) and the
  TUI end-to-end `ask`, `needs_you` and `idle` tests, both run in CI's `greenfield-macos`.
