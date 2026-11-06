# No adapter lists models or effort levels: it asks the installed harness or passes the value through

Status: accepted

Issue #743. Decided by Theo (2026-11-04: Torlek must not assume which models, effort levels
or capabilities a harness has); the mechanism is the worker's, as the smallest that meets it.

- **Effort levels are asked of the harness, once per build.** `Provider::check_choices` runs
  after the trial launch of `Sessions::check_launch_choices`, before a session is recorded or
  the owner is asked. The Claude adapter reads the levels `claude --help` lists after
  `--effort`, with the version `claude --version` prints, and reads them again when the
  program file's modification time or size changes. A level an update adds works at once; one
  it removes is refused at once, naming the levels and the version.
- **Help that lists nothing passes the value through.** When `--help` cannot run or no longer
  words the option with its levels in parentheses, Claude Code answers for itself, and the
  miss is kept for that build. Refusing everything would make a rewording a Torlek outage.
- **Models stay shape-only.** `check_model` checks one word that cannot read as a flag;
  Claude Code refuses a model the account cannot use. `Provider::launch` checks the same
  shape of an effort, so restart, wake and restore never ask the harness again.
- **The rule is a test.** `cockpit-workspace-rules` fails when a `const` or `static` in
  `cockpit-claude`, `cockpit-codex` or `cockpit-opencode` holds a quoted effort level or
  model name. Codex and OpenCode report neither `effort` nor `model`; OpenCode's `variants()`
  already asks its server and stays the pattern for when they do (#407).
- **Rejected:** parsing the warning `claude --effort bogus` prints (2.1.319 warns and starts
  with its default, so it is not a refusal and its wording is no contract).

Evidence: [effort choices](../../crates/cockpit-claude/tests/effort_choices.rs),
[`torlek agent start`](../../crates/cockpit-daemon/tests/coordinator_kit.rs) and
[no list in an adapter](../../crates/cockpit-workspace-rules/tests/harness_choices.rs).
