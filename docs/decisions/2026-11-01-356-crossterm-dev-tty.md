# The TUI reads its keys through crossterm's `use-dev-tty` reader

Issue #356. Decided by Theo, on the worker's evidence.

- **`crates/cockpit-tui` enables crossterm 0.29's `use-dev-tty` feature.** The default reader
  (`event/source/unix/mio.rs`) polls edge-triggered and returns a resize as soon as the resize
  signal is in the batch, dropping the tty's readiness: a key that arrives with the resize
  attach causes waits for the next key. The CI flakes of the end-to-end tests were this, on
  the owner's first key after attach. The `use-dev-tty` reader (`tty.rs`) polls level-triggered
  and reads the tty before the signal, so no key waits. Proven by `cockpit-tui`
  `e2e attach::a_key_pressed_while_the_terminal_resizes_reaches_the_list`.
- **It adds `filedescriptor` 0.8** (and `thiserror` 1 beside 2), which `cargo deny` accepts.
- **Rejected:** making the tests press again or wait out the resize, because the owner hits the
  same held key; a vendored or patched crossterm, because a supported feature does it; the
  `event-stream` feature, because it sits on the same mio reader.
