# The Claude hook bridge ships in `bin/` beside the daemon and is the manifest's first helper

Issue #185. Decided by the worker: the daemon finds `cockpit-claude-hook` only in its own
directory (`crates/cockpit-daemon/src/main.rs`, `claude_code`), so the bundle puts it there.

- **`bin/cockpit-claude-hook`, built with `-p cockpit-claude`.** `tools/release.py` lists it in
  `HELPERS`, which `BINARIES` includes, so `package` refuses a build without it and
  `verify-install` fails an install that lacks it or a manifest that does not list it.
  `tools/tests/test_release.py` checks that `release.yml` builds and gathers every entry of
  `BINARIES` and that the daemon's lookup names each helper.
- **`helpers` maps a helper's name to its version**, the bundle's own, since helpers build from
  the same revision. Report 13 wants each session to record the helper version it started with;
  a name-to-version map is the smallest record that serves it, and `verify-install` refuses
  any other content. Schema stays 1: the field
  existed and only its content changes from `{}`.
- **Rejected:** a separate `libexec/` directory, because the daemon would then need a second
  lookup rule; and a smoke run of the hook in `verify-install`, because it needs a Claude
  payload and a `cockpit` socket, which the bridge tests in `cockpit-claude` already cover.
