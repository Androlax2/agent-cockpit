# The compaction threshold is one opaque string in the harness's own words, detected per install

Status: accepted

Issue #783. Decided by the worker, within #743 (detected capabilities) and #701 (model and
effort in the harness's words).

- **One `compaction_threshold` text per session, stored as typed** (sessions migration 15),
  passed to every launch like model and effort. Each adapter validates it in its harness's
  vocabulary at launch: Claude Code `--autocompact <auto|window>`, Codex
  `--config model_post_turn_compact_threshold_percent=<0..100>`, OpenCode `compaction.buffer`
  (`"N%"` or a token count). Rejected: one cockpit-wide percent translated per harness, because
  the three options measure different things (a window, a percent of the window, a buffer) and
  a translation would hide which harness word the owner chose.
- **The capability is detected, not pinned to a version.** `Capability::CompactionThreshold`
  comes from the probe: the `--autocompact` flag in `claude --help`, the `compaction.buffer`
  setting in the `opencode` program, and for Codex the key itself, proved by
  `codex --config model_post_turn_compact_threshold_percent=torlek-probe features list`
  naming the key in its refusal (an unknown key is accepted silently, so `--config` alone
  proves nothing). Rejected: a version table, which the tested-versions file (#732) already
  rules out for other capabilities.
- **A withheld capability carries its reason.** `Capabilities::withholding(capability, reason)`
  keeps the missing requirement ("flag --autocompact in `claude --help` is missing in 9.9.9"),
  and `Capabilities::require` repeats it in the refusal, so the owner reads what the installed
  harness lacks rather than only that the setting is refused. Capabilities the tested version
  has are never withheld.
- **Off by default adds nothing.** Without the setting no flag, config key or
  `OPENCODE_CONFIG_CONTENT` entry is added, so the harness's own settings apply; OpenCode's
  inline config is only written when the bridge or the threshold needs it.
- **Not yet:** `torlek agent start` (the coordinator path) and the TUI form send no threshold,
  and there is no per-project default; each is a follow-up.
