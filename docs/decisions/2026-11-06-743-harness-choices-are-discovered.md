# No adapter lists models or effort levels: it asks the installed harness or passes the value through

Issue #743. Decided by Theo (2026-11-04: Torlek must not assume which models, effort levels
or capabilities a harness has); the mechanism is the worker's, as the smallest one that meets it.

- **Effort levels: asked of the harness, once per build.** `Provider::check_choices` runs
  after the trial launch of `Sessions::check_launch_choices`, before a session is recorded or
  the owner is asked. The Claude adapter reads the levels `claude --help` lists after
  `--effort`, with the version `claude --version` prints, on the first start that chooses one,
  and reads them again when the program file's modification time or size changes: that is
  "cached per harness version", and an update while the daemon runs shows at the next start.
  A level an update adds works at once; one it removes is refused at once, naming the levels
  and the version they were read from.
- **Help that lists nothing: pass through.** When `--help` cannot run or no longer words the
  option with its levels in parentheses, the value passes through and Claude Code answers for
  itself, and the miss is cached for that build so a broken `claude` is asked once, not at
  every start. Refusing everything would make a rewording of the help text a Torlek outage.
- **Models: shape only, unchanged.** `check_model` still checks one word that cannot read as
  a flag; Claude Code refuses a model the account cannot use. `Provider::launch` checks only
  the same shape of an effort, so a restart, wake or restore never asks the harness again and
  a stored value a harness later drops is the harness's to refuse.
- **The rule is a test.** `cockpit-workspace-rules` fails when a `const` or `static` in
  `cockpit-claude`, `cockpit-codex` or `cockpit-opencode` holds a quoted effort level or
  model name. Codex and OpenCode report neither `effort` nor `model`, so a start that
  chooses one is refused by capability; OpenCode's `variants()` already asks its server and
  stays the pattern for when they do (#407).
- **Not done here:** showing the discovered choices, labelled with the harness version, in the
  TUI, `torlek docs` and the coordinator brief: none of them shows choices yet, so each
  reads `ClaudeCode::effort_levels` when it does. Capabilities by runtime probe belong to
  #732.
- **Rejected:** parsing the warning `claude --effort bogus` prints (2.1.319 warns and starts
  with its default, so it is not a refusal and its wording is not a contract); a levels
  list refreshed on a timer (the daemon's adapter is per version already).

Evidence: [effort choices](../../crates/cockpit-claude/tests/effort_choices.rs),
[start through `torlek agent start`](../../crates/cockpit-daemon/tests/coordinator_kit.rs),
[no list in an adapter](../../crates/cockpit-workspace-rules/tests/harness_choices.rs) and the
ignored [conformance](../../crates/cockpit-claude/tests/conformance.rs) test against the
installed Claude Code.
