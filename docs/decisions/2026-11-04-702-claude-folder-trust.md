# Torlek records Claude Code's folder trust for a parallel clone of a trusted project, and shows a launch that never reports as waiting on the owner

Issue #702. Decided by Theo on 2026-11-04 (option A, "with the safe write"), from what the
worker measured in Claude Code 2.1.317.

- **A launch whose agent never reports waits on the owner.** When a provider's bridge reports
  as the harness starts (`Provider::reports_at_launch`: Claude Code's `SessionStart`, not
  OpenCode's, which needs a first prompt) and the current launch's agent has not reported
  within 30 s, the sessions' periodic pass records an open dialog question naming the likely
  screens (folder trust, sign-in, update). The agent's first report of what it does answers it.
  Provider-neutral: any harness screen before the first hook is caught, not only Claude's trust
  dialog. Rejected: reading the pane for known screens, which reads the terminal of a resumed
  conversation too, and a launch failure after the deadline, which would kill an agent the owner
  can still unblock. (`crates/cockpit-sessions/src/lifecycle.rs` `flag_if_unreported`.)
- **Where Claude Code keeps trust (measured on 2.1.317).** `projects[<git root>].hasTrustDialogAccepted`
  in `$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`. Its lookup walks from the working
  directory up to the git root only, so trusting a project never covers a clone: each clone is
  its own root. Claude Code saves the file under a `<file>.lock` directory lock, re-reading it
  and merging `projects`. No command-line option skips the dialog in an interactive session.
- **Torlek writes that record itself, safely.** Under Claude Code's own lock, from the file as it
  is then, changing only the clone's trust key (or adding the entry Claude Code itself writes),
  keeping every other byte, replacing the file by an atomic rename with its mode. A file that is
  missing, a link, not JSON, has a duplicated key on the path, or stays locked for 2 s is
  refused and left untouched; the dialog then shows and the detection above flags it.
  (`crates/cockpit-claude/src/trust.rs`; fixture tests in `crates/cockpit-claude/tests/trust.rs`,
  never the owner's file; the conformance test pins the key and lock in the installed binary.)
- **Only where Torlek's own trust already applies.** A parallel clone (never a review clone, never
  a main checkout) of a project in `projects.trusted`, whose `origin` is the main checkout's
  (`cockpit_projects::harness_trust_source`), and whose harness settings (`.claude/`,
  `.mcp.json`, in every directory from where the agent starts up to the clone's root) each
  exist with the same bytes at the same place in the main checkout
  (`cockpit_claude::trust::unexpected_settings`). Files only the main checkout has, such as the
  owner's untracked `.claude/settings.local.json`, are allowed: the clone applies fewer settings,
  never other ones. A branch that changes them gets the dialog: clones are reused, so a launch
  whose settings differ, or cannot be compared, also turns a trust recorded earlier for that
  clone back to `false` (only an existing `true`; no entry is added). Each launch in a parallel
  clone reads `projects.trusted` from the owner's configuration as it is then, and records the
  trust off the async workers once the launch passed its gates (sandbox, checkout, execution
  reserved), before the agent runs; any refusal is logged and the launch goes on
  (`crates/cockpit-sessions/src/lifecycle.rs` `trust_clone`).
- **Accepted:** the settings are compared moments before the agent reads them, so a process
  writing into the clone in between is not caught; a stale `<file>.lock` a crashed Claude Code
  left makes each clone launch wait 2 s and show the dialog until Claude Code reclaims it.
  Files Claude Code itself saves in a clone only (an "always allow" in
  `.claude/settings.local.json`) bring the dialog back on the next launch there.
- **The main checkout's Torlek trust stays a setup step.** Adding a project to
  `projects.trusted` is the owner's decision; nothing here sets it.
- **Rejected:** launching with `CLAUDE_CODE_SANDBOXED=1`, an undocumented variable that skips
  every trust check of the process (including after a directory move) and could change meaning
  in any release; trusting the `.parallel/<project>` parent, which Claude Code's root-bounded
  lookup ignores.
