# Cockpit security and performance audit — 2026-10-19

Scope: `claude-sessions` (bash), `apps/cockpit` (column, ask-to-run, approval terminal, claude-todo,
claude-design, guides), `apps/desktop` (OSD daemon), the Claude hooks, the greywall launcher and
profile. Read-only audit from inside a dotfiles sandbox; the mount claims were checked against this
session's `/proc/self/mountinfo` (stacked mounts: the topmost wins).

Threat model: a Claude session inside greywall that is prompt-injected (by a repository, a web page,
an issue, another session's text) or malicious. Goals: run code on the host, get a command approved
without Theo really seeing it, act through another session (above all the dotfiles one), hide its
tracks, or freeze the cockpit.

## Summary

The approval terminal, the daemon's report validation and the tmux/hyprctl quoting are solid. The
weak points are the **filesystem boundaries**: the dotfiles sandbox can write code the host runs
without review, every sandbox can write `.git/config`, other projects' Claude memory and the shared
`~/.cache/claude` folders, and the log can neither prove what was approved nor resist injection.

| # | Severity | Finding |
|---|---|---|
| C1 | Critical | The dotfiles session writes host-executed code with no review step |
| H1 | High | `.git/config` is writable in every project; the host runs git there |
| H2 | High | Every sandbox writes `~/.claude`: other projects' memory and transcripts |
| H3 | High | Run requests can be rewritten by another sandbox before they are shown |
| M1 | Medium | A restart can resume a session in another project's folder and token |
| M2 | Medium | FigCraft's control port reaches every project's sandbox |
| M3 | Medium | Every sandbox reads every command output and guide answer |
| M4 | Medium | The todo channel accepts forged projects, sessions and statuses |
| M5 | Medium | Untrusted files and sockets can freeze the column or the daemon |
| M6 | Medium | The log is readable, injectable, short-lived and lacks the approved command |
| L1–L5 | Low | Design role forgery, long commands off screen, guide links, OSC 52, GH token |

## Critical

### C1. The dotfiles session writes host-executed code with no review step
- Evidence: `/home/theo/dotfiles` is mounted rw on top in the dotfiles sandbox. Writable and run by the
  host as Theo: `config/bin/.local/bin/*` (live through stow symlinks: `claude-sessions`, `osd-show`,
  `craft-open`, `claude-update`, `clipboard-image-serve`, and **`greywall-agents/claude`, the
  sandbox launcher itself**), `Makefile`, `tools/` (`handoff-install.sh`), `apps/` (built and tested
  on the host by `make cockpit` / `make test` through ask-to-run, where Theo sees the command, not the
  diff), `config/hyprland` (Hyprland reloads its config on change: an `exec` there runs at once),
  `config/tmux`, `config/waybar`, `config/kitty`, `config/osd`, `config/environment`.
  `greywall.json` denyWrite covers only `config/{greywall,fish,systemd,claude}` and `.git/hooks`.
- Path: anything that steers the dotfiles agent (an error hand-off quoting other sessions' text, a
  `claude-todo blocked` report, a todo, a web page) → an edit to one of those files → host code
  execution, without any approval.
- Fix: the dotfiles session works in a **separate worktree** (`~/dotfiles-work`, not stowed, not
  built from); the live checkout only moves forward through an ask-to-run that shows `git diff`
  and fast-forwards (`claude-sessions apply-dotfiles`). Until then, add denyWrite for
  `config/bin`, `Makefile`, `tools`, `config/{hyprland,tmux,waybar,kitty,osd,environment}` and
  `apps/target`, and keep editing those through `.handoff/` (diff shown, y asked).
- Note: `apps/target/release/deps/<bin>-<hash>` are hardlinks of the read-only release binaries,
  under the writable mount.

## High

### H1. `.git/config` is writable in every project
- Evidence: greywall denies `.git/hooks` only (`greywall.json:88`); `claude-sessions:1204` says
  `.git/config` is read-only, which is not true.
- Sinks on the host: `push_run` runs `git symbolic-ref` (1218) and `git log @{upstream}..HEAD`
  (1232) without `SAFE_PUSH_CONFIG`; `prs_window` (1106) runs `git remote get-url`; any host shell
  prompt or IDE running `git status` in the folder executes `core.fsmonitor`. `core.hooksPath`,
  `core.pager`, `include.path` also run code.
- Fix: denyWrite `.git/config`, `.git/info`, `.git/modules/*/config`; give every host git call in
  claude-sessions `SAFE_PUSH_CONFIG` plus `GIT_CONFIG_NOSYSTEM=1 -c include.path= -c core.fsmonitor=
  -c core.hooksPath=/dev/null`; fix the comment at 1204.

### H2. Every sandbox writes `~/.claude`
- Evidence: `learned/claude.json:162` allowWrite `~/.claude`; denyWrite covers settings, hooks,
  skills, CLAUDE.md only. From the dotfiles sandbox,
  `~/.claude/projects/-home-theo-IdeaProjects-opusline/memory` is on an rw mount.
- Path: a session in project A writes `~/.claude/projects/-home-theo-dotfiles/memory/*.md`; the
  dotfiles session loads it as Theo's own memory at its next start. Persistent prompt injection.
- Fix: allowWrite only `~/.claude/projects/<this project's slug>` (the launcher knows the cwd) plus
  the files Claude Code needs (`~/.claude.json`, `todos`, `shell-snapshots`…); denyWrite
  `~/.claude/projects/*/memory` except the own one.

### H3. Run requests can be rewritten by another sandbox before they are shown
- Evidence: all sandboxes write `~/.cache/claude/runs/`. `registry.rs:476-488` queues the id;
  `claude-run-approve.rs:22-38` reads `<id>.request` only when its turn comes. The project shown is
  derived from the request's `cwd`, also sandbox-written; the `session` field is neither checked nor
  shown. Guides: same (`claude-guide.rs:41-49`).
- Path: session B rewrites A's pending request: same command (`make check`), `cwd` set to a folder B
  controls with its own Makefile. The window shows "dotfiles asks you to run make check", which A
  just announced; Theo approves; B's Makefile runs on the host.
- Fix: send `run-request` / `guide-request` over `osd-report.sock` (SO_PEERCRED); the daemon copies
  the request at once into a host-only `$XDG_RUNTIME_DIR/claude-runs/<id>.request` (0600) bound to
  the sender's verified session; the approval reads only that copy and shows the verified project,
  with a red warning when `cwd` is outside that project.

## Medium

### M1. A restart can resume a session in another project's folder
- `restart_session:695` / `restore_sessions:670` → `launch_detached:765 tmux new-session -c
  "$directory"` with the reported `cwd`, never through `require_plain_directory`; the daemon only
  checks the text. A session of A reporting `cwd=~/IdeaProjects/B` resumes, on r or ⌘R, in B's
  sandbox with B's GitHub token.
- Fix: `require_plain_directory` in `launch_detached`; the daemon keeps the first cwd per session
  (taken from the pane's real path) and refuses a later one outside that repository.

### M2. FigCraft's control port reaches every project
- Launcher 115-118 forwards 7979 to every sandbox; while FigCraft serves project X, any session can
  drive it (read and write X's design files).
- Fix: craft-open records the served project; the launcher forwards 7979 only to that project.

### M3. Every sandbox reads every command output and guide answer
- `~/.cache/claude-answers` is allowRead for all; `claude-run-approve.rs:119-123` writes full output,
  `claude-guide.rs:158-175` writes typed answers unredacted.
- Fix: one answers subfolder per session id, only that one bound into its sandbox.

### M4. The todo channel accepts forged updates (`todo.rs:398-441`)
- `add` lands straight in To do under any project chosen by `cwd` (dotfiles included); `session` is
  taken from the JSON; any session can mark any item done; the blocked-report cap counts per derived
  reporter (any folder outside a repository is a new reporter); no size or line cap.
- Fix: per-session update files (named by the verified session); `add` from a session lands as
  Proposed unless its cwd is under the sender's project; status changes only on items handed to that
  session; blocked reports capped per sender per day; cap lines per apply.

### M5. Untrusted files and sockets can freeze the column or the daemon
- Column, every tick: `updates.jsonl`, `designs/<id>.{json,role}`, `statusline-<id>.json`, whole
  transcripts (`sessions.rs:259`). Daemon: `<id>.request`, the session folder's `.git`. A FIFO
  planted there blocks `open()` forever; a huge file or `/dev/zero` symlink exhausts memory.
- Daemon: the report socket is read on the GTK thread with a 300 ms timeout per read (slow-loris);
  the FIFO line buffer is unbounded; any valid-looking id queues a run banner.
- Fix: one `util::read_untrusted(path, cap)` (O_NOFOLLOW|O_NONBLOCK, fstat regular file, size cap)
  at every site; socket reads off the main thread with a total deadline; 64 KiB line cap; queue only
  ids whose request file exists, cap per sender.

### M6. The log cannot support incident response
- `~/.local/state/claude-sessions` is read-only but **readable** from every sandbox (log, todo.json,
  error reports, usage.sqlite); `hook.log` is sandbox-writable.
- Injection: Rust `log_as` keeps ESC and C0/C1 controls (`util.rs:49`); bash `log_event` strips only
  `\n`; sandbox text reaches log lines (todo, blocked, project names). `claude-sessions logs` prints
  it raw to a terminal.
- Gaps: the approved command is not kept (only in the sandbox-writable request file); run shown /
  input phases / early refusals not logged; rejected todo updates dropped silently; design links
  and roles, greyproxy holds, sidebar kills, first report of a session not logged; `say` logs length
  only.
- Retention: 1 MiB + one `.1` ≈ 3–4 days; a sandbox can flush it through todo updates; tests write
  to the real log.
- Fix (audit log): `~/.local/state/claude-audit/audit-YYYY-MM-DD.jsonl`, 0600, denyRead in greywall,
  mirrored to journald (`systemd-cat -t claude-audit`). One JSON object per line from
  `util::audit(event, fields)` and `claude-sessions audit`: `ts`, `seq`, `prev` (sha256 chain),
  `component`, `event`, `outcome`, `reason`, verified `session`/`project` vs `claimed_*`, content as
  `{sha256, len}`; full original and final command of each run in a 0600 sidecar. Events: run.*,
  guide.*, todo.applied/rejected, blocked.filed, design.linked, report.rejected, session.*,
  prompt.queued/sent, say, error.handoff, greyproxy.held, validation.refused. 90 days, gzip after 7.
  The debug log keeps going, controls stripped in both writers, `label-state`/`focus-viewer`
  successes dropped (3 224 of 8 157 lines).

## Low
- L1 Design role and canvas forgery for another session's id (`designs/` shared): per-session dirs.
- L2 A command taller than the 45-row window pushes its start off screen; `claude-guide` `run:` steps
  (2000 chars) have no length warning: pager or refusal past rows − 12.
- L3 Guide links (Ctrl+O) open any http(s) URL in the host browser, outside greyproxy: show the host
  and confirm, or allow-list.
- L4 An approved command's output reaches the pane raw: OSC 52 clipboard writes, fake prompts.
  Turn `set-clipboard` off for `claude-run-*` sessions.
- L5 Info: the GitHub token is in the sandbox as a real value (documented trade-off): short expiry,
  no admin/workflow scopes.

## What holds (verified)
- Approval screen: control/bidi/zero-width/tag characters shown escaped in red, non-ASCII listed,
  Decline by default, keys in the first 500 ms dropped, the shown command is exactly what runs
  (`fish -c` quoted, test), cwd resolved before display.
- Ids: 12-hex run/guide ids, UUID session ids, everywhere they are used as paths.
- Answers written only by the host via `util::write_json` (random temp name, create_new, rename).
- Sandbox FIFO limited to `run-request` / `guide-request`; phases, banners, focus only via
  `osd-host.fifo`. Reports via the socket with SO_PEERCRED; pane bound to the sender's process;
  text checked (length, controls, `#(`, `#{`).
- ratatui drops control and zero-width characters; no shell between the cockpit and tmux, hyprctl,
  xdg-open; URLs regex-checked; jq always `--arg`; `fish -ic '…' -- "$@"`; push refspec fixed,
  origin must be GitHub, y/N first; `~/.cache/claude-answers`, skills, settings, hooks, CLAUDE.md
  read-only; auto-update disabled.

## Performance (by felt latency)
1. Every key press runs a full tick (`claude-sessions-sidebar.rs:73-76`): tmux, pgrep, snapshot
   parsed twice, per-card files — 15–40 ms per j/k. Rebuild from cached state on keys.
2. Synchronous `manager()` in key handlers and in the tick (`open` p50 41 ms, max 500 ms; `say`
   52 ms): use `manager_child` and collect in the tick.
3. `view_session` / `show`: three jq passes, ~15 tmux clients, forks in `log_event`: one jq @tsv,
   one `list-panes` read, chained tmux calls, `printf '%(…)T'` — 41 → ~15 ms.
4. OSD daemon main thread: /proc scanned twice every 2 s with a regex recompiled, one capture-pane
   per session, 5 ms poll sleeps, `khal` each minute, banner click `.status()`: move to a worker.
5. `read_title` reads whole transcripts (up to 386 MB) to keep 2 MB: `take()` the first 2 MB.
6. Per tick: errors snapshot parse outside its gate, usage read twice, pgrep, mtimes: gate/cache.
7. Regexes compiled per call (`sessions.rs:44-45`, `util.rs:284`, `reviews.rs:195`, `errors.rs:19`,
   `registry.rs:1286`): `LazyLock`.
