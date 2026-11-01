# The TUI reads its keys through crossterm's `use-dev-tty` reader

Issue #356. Decided by Theo, on the worker's evidence.

- **`crates/cockpit-tui` depends on crossterm 0.29 with `use-dev-tty` and reads its keys
  through it.** The default reader (`event/source/unix/mio.rs`) polls edge-triggered and
  returns a resize as soon as the resize signal is in the batch, dropping the terminal's
  readiness: a key that arrives with the resize attach causes waits for the next key. The CI
  flakes of the end-to-end tests were this, on the owner's first key after attach. The
  `use-dev-tty` reader (`tty.rs`) polls the terminal (stdin when it is one, else `/dev/tty`)
  level-triggered and reads it before the signal, so no key waits. Proven by `cockpit-tui`
  `e2e attach::a_key_pressed_while_the_terminal_resizes_reaches_the_list`.
- **`src/main.rs` imports `crossterm::event`, not ratatui's re-export,** so the fix does not
  hang on feature unification: should ratatui move to another crossterm, the key reader keeps
  this one. Keep the two on one version, or the terminal is driven by two crossterms.
- **Accepted:** that reader blocks on the terminal after bytes that parse into no event, so a
  resize then waits for the next byte to be reported; the TUI redraws every 200 ms and ratatui
  reads the size on each draw, so the screen still follows. It adds `filedescriptor` 0.8 (and
  `thiserror` 1 beside 2), which `cargo deny` accepts.
- **Rejected:** making the tests press again or wait out the resize, because the owner hits the
  same held key; a vendored or patched crossterm, because a supported feature does it; the
  `event-stream` feature, because it sits on the same mio reader.
