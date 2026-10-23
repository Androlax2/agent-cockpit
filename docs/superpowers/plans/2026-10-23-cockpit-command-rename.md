# Cockpit Command Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give agent-neutral cockpit commands, units, state, runtime files and environment variables cockpit names without breaking existing sessions or pairing.

**Architecture:** Rename canonical entry points and their callers together. Keep explicit forwarding commands and legacy environment fallbacks; migrate persistent state only through a reviewed host-side step with collision checks, and restart services in a controlled order. Do not change the security boundaries or underlying data formats.

**Tech Stack:** Rust workspace, shell launchers, systemd user units, greywall JSON configuration, Python/Playwright tests.

**Spec:** User-approved Option B in this conversation; todo `df08aece` requests the remaining agent-neutral command rename with forwarding names and preservation of `claude-audit` storage.

## Global Constraints

- Canonical prefix: `cockpit-`. Rename todo, remote, design, guide, fade, run-approve, demo, needs-input, session-label, health-check and idle-prompt. Include design per the todo, correcting the earlier exclusion.
- Keep old command names as forwarders preserving argument boundaries, exit codes and signal behavior; restore `claude-sessions` forwarding to `cockpit` if absent from the current base.
- Do not rename Claude Code update commands/units, Claude-specific usage, credentials, transcripts, status caches, or `claude-audit` and its protected folder.
- Use `COCKPIT_*` for agent-neutral configuration; an explicitly set new variable takes precedence, with old variables accepted for existing callers. Do not rename agent-owned environment variables such as `CLAUDE_CODE_SESSION_ID`.
- Preserve pairing secrets, push subscriptions, queued approvals, todos, canvas links, and sandbox restrictions. Never merge conflicting state directories silently or delete user data.
- No dependency or manifest changes without separate permission. Automatic Rust binary discovery should suffice for source renames.
- Host-side scripts, Makefile, units, hooks and sandbox configuration changes must be called out before application. Do not claim migration verified from unit tests alone.

## Review Focus

- Already-running sessions using old commands, environment variables and mounted socket directories must retain a working path until explicitly restarted.
- Phone cookies and subscriptions must still authenticate after moving remote state; secrets must remain inaccessible inside greywall.
- Both old and new state locations existing must stop migration before any service shutdown or data movement.
- An outstanding approval or guided task must not disappear, execute twice, or become orphaned by migration.
- Only one phone server and one health-check timer may remain enabled; application must not strand an old active unit.

## Exact migration map

- `$XDG_STATE_HOME/claude-{remote,stuck-exits}` → `cockpit-{remote,stuck-exits}` in the same state home (`COCKPIT_STATE_HOME` wins).
- `~/.cache/claude/{runs,designs}` → `~/.cache/cockpit/{runs,designs}`. Other `.cache/claude` entries remain Claude-owned.
- `$XDG_RUNTIME_DIR/claude-{report,team-host,answers,runs,requests}` → corresponding `cockpit-*` directories. Move, do not copy, preserving existing mounted inodes; leave exact legacy symlinks.
- `$XDG_RUNTIME_DIR/claude-{todo-updates.jsonl,todo-editor.lock}` → corresponding `cockpit-*` files, also preserving inode and legacy aliases.
- Transient away markers and draft filenames transition at their explicit readers/writers, not through glob-based host migration. Outstanding requests/results keep their JSON formats and identifiers.
- Canonical configuration: `COCKPIT_{REMOTE_DIR,REMOTE_ADDRESS,TODO_PATH,TODO_UPDATES,DESIGNS_DIR,RUNS_DIR,ANSWERS_DIR,ANSWERS_KEY,DOTFILES_WORK,DOTFILES_LIVE,GREYWALL_WORK,GREYPROXY_WORK,REVIEWS_FIXTURE,HEALTH_COCKPIT,HEALTH_DOTFILES}`, with each old `CLAUDE_*` spelling accepted as fallback. `CLAUDE_GUIDES_DIR` is already unused and is not a migration input.
- Keep phone cookie `claude_phone` (if used by the existing server), public routes, wire formats, request ids and agent-owned retry/session identifiers stable; changing those would break existing conversations rather than rename a command.

---

### Task 1: Establish a current base and rename command entry points

**Files:** `apps/cockpit/src/bin/claude-{todo,remote,design,guide,fade,run-approve,demo}.rs`, `apps/osd/src/bin/claude-idle-prompt.rs`, corresponding scripts under `config/bin/.local/bin/`, all active callers and test fixtures found with `git grep`; historical design/audit documents remain unchanged.

**Interfaces:** New commands have identical arguments and behavior under the `cockpit-` prefix. Old shell commands forward via `exec` to their canonical sibling.

- [ ] Bring this clean feature branch onto current `origin/main`, excluding the already-applied phone commit. Inspect current `CLAUDE.md` and current removal of `claude-sessions` before changing either.
- [ ] Add regressions in `apps/cockpit/tests/cockpit_script.rs` and relevant command integration suites: old command forwarding preserves a spaced argument and a nonzero exit code; new names reach the same real command entry points. Use existing sandbox fixtures, not real host state.
- [ ] Run the new regressions and record their expected failure before implementation.
- [ ] Rename binary sources and canonical wrappers; retain executable legacy wrappers. Update active call sites in Rust, shell, hooks, plugins, test fixtures and current documentation. Inventory every remaining `claude-*` occurrence and classify retained uses explicitly.
- [ ] Run command integration suites and wrapper regressions. Commit the command-only change without the todo completion trailer.

### Task 2: Rename environment configuration and plan state migration from actual consumers

**Files:** `apps/cockpit/src/{paths,remote,approval}.rs`, `apps/osd/src/{paths,registry}.rs` and other consumers discovered on the current base; `apps/cockpit/tests/common/sandbox.rs`, corresponding integration suites; launcher scripts under `config/bin/.local/bin/greywall-agents/`.

**Interfaces:** New names replace only agent-neutral `CLAUDE_*` overrides. Defaults use canonical cockpit paths after host migration. Existing data schemas and the phone cookie name stay unchanged.

- [ ] Inventory all path producers and consumers, including approval/guide caches, remote state, todo update queue, design links, socket directories, tmux session names, window classes and unit probes. Record an exact old-to-new map in this plan before coding migration.
- [ ] Add isolated tests proving new-variable precedence and old-variable fallback for each renamed override, using parametrized cases where practical.
- [ ] Add migration regressions with a fake HOME/runtime: old-only state survives byte-for-byte, permissions remain private, a second run is harmless, conflicting directories fail before mutation, custom override paths are not moved, and legacy paths cannot escape via unexpected symlinks.
- [ ] Run failing tests, then implement explicit compatibility at the configuration boundaries and a single host migration operation. Do not replace `.cache/claude` wholesale: it also contains Claude-owned data.
- [ ] Preserve stable runtime socket paths for already-running mounts through an explicitly tested transition, or require a clearly reported restart boundary before switching; do not assume a symlink changes an existing bind mount.
- [ ] Run Rust integration tests covering both legacy and canonical configuration. Commit the migration change.

### Task 3: Update sandbox restrictions and service lifecycle together

**Files:** `config/greywall/.config/greywall/greywall.json`, `config/bin/.local/bin/greywall-agents/launcher-common.sh`, other affected agent launchers, `config/systemd/.config/systemd/user/claude-remote.service`, `claude-health-check.{service,timer}`, timer wants links, `Makefile`, host migration script under `tools/`, associated `tools/tests/` and `apps/cockpit/tests/{greywall_launcher,cockpit_script,health_check}.rs`.

**Interfaces:** `cockpit-remote.service`, `cockpit-health-check.service` and `.timer` are canonical. Preserve enabled/active intent while disabling legacy scheduling; report errors rather than leaving duplicate services.

- [ ] Add fake-systemctl lifecycle tests: fresh install, active legacy phone server, inactive server, enabled legacy health timer, repeated migration, conflicting state, and an outstanding approval. Assert ordering and no duplicate active units.
- [ ] Add sandbox configuration/launcher tests: both legacy and canonical private remote paths are denied; canonical command wrappers and binaries are readable; report socket mounts match daemon producers. Do not widen writable paths or expose host approval sockets.
- [ ] Run failing tests, update units and hooks, then wire migration into `after-apply` before relink/reload can activate new defaults. Reuse existing installation patterns rather than introduce a separate service framework.
- [ ] Ensure the migration refuses unsafe in-flight changes with an actionable restart requirement, and preserves private state before starting the new server. Do not dump secret contents to logs or tests.
- [ ] Run lifecycle, launcher and sandbox regressions, then commit.

### Task 4: Verify compatibility through real entry points

**Files:** `apps/cockpit/tests/{mobile_browser.py,claude_remote.rs,claude_todo.rs,approval_terminal.rs,guide_terminal.rs}`, relevant OSD integration tests and test fixtures; current README and CLI help.

**Interfaces:** Unchanged public phone routes, pairing cookie, approval/report wire formats and todo persistence.

- [ ] Add a pairing migration integration test: pair through the old configuration, migrate isolated state, restart under the canonical name, and authenticate with the original cookie; verify saved push-subscription data survives without sending real pushes.
- [ ] Exercise old and new todo commands through the real reporting entry point; verify exactly one queued update and no loss during migration.
- [ ] Verify approval and guide continuation across the supported transition, including an explicit refusal when safe migration requires completing an in-flight action first.
- [ ] Run `make test` and the full Playwright phone suite against freshly built binaries. Record full exit statuses and failures, not piped output that masks failures.
- [ ] Scan remaining old-name references. Keep only compatibility code/tests, historical records and documented Claude-specific exceptions. Review shell syntax, executable bits, systemd unit references and symlink targets. Update current documentation with migration/restart requirements.

### Task 5: Apply with host review and verify live behavior

**Files:** `.outgoing/` patch series; any external-file handoff under `.handoff/` with `SHA256SUMS`.

**Interfaces:** Existing `cockpit apply` review/approval; legacy todo commands remain callable throughout.

- [ ] Coordinate any external global-instruction updates with the dotfiles project; use a handoff or an explicitly scoped cross-project todo rather than edit inaccessible running copies. Forwarders must make this nonblocking.
- [ ] Review diff stat and call out every host-executed script, Makefile, unit and sandbox configuration change. Commit verified work with `Todo: df08aece` only when the requested rename is complete.
- [ ] Export only this task's commits; ask the host to run `cockpit apply <this-clone>` and `make -C ~/agent-cockpit test` through `ask-to-run`, keeping the human review.
- [ ] Verify live unit state and timer scheduling, phone health, legacy and new command resolution, preserved pairing without printing secrets, and a fresh sandbox's restrictions/reporting. Report any existing session that needs restart instead of silently breaking it.
- [ ] Run `claude-todo list`, then mark `df08aece` done using the retained legacy command (as requested), including the applied commit summary. Leave unfinished checks visible rather than claim completion.
