# Gate Alpha 3: live acceptance runs (#136)

Proof for [gate #46](https://github.com/Androlax2/agent-cockpit/issues/46)'s "live
acceptance #136 completed by Theo". The guide is [docs/acceptance/m2.md](../acceptance/m2.md) (the `m2`
filename keeps the issue's link; Alpha 3 is the release name), in `torlek ask guide` format; it
is re-run after fixes and each run is appended below.

## What it covers

- Todo items: add, edit, image note, hand to a session (`h`), queue toggle (`Q`), queue
  explanation (`?`) and the queue placing an item by itself.
- The code-change preset from task packet to evidence-bound completion: a `checks` receipt the
  owner approves, `review` refused until the owner waives it. Pull request state as evidence
  is not in this build: [#60](https://github.com/Androlax2/agent-cockpit/issues/60) (Beta 2).
- Resource admission: queued work that does not fit the budget waits, with its explanation.
- The merged Alpha 3 work: session temp folders (#766), the TUI's idle redraw and change feed
  (#705, #706), the attention bell (#682), worker questions held by their coordinator (#710),
  the coordinator's cap of active workers (#739), a coordinator ending its worker (#713), the
  sweep of an ended worker's clone (#767), Claude folder trust in a parallel clone (#702),
  owner verbs refused inside a session (#729), a delivered message line clearing (#728),
  `session send` submitting (#730), `torlek project remove` (#727).
- #406's live acceptance: `torlek storage` before and after a prune on the isolated home.
- Not covered live: the update notice (#617: a terminal, the network and a newer release),
  per-project GitHub tokens (#754: a real token), usage-error lines (#770) and the `ask run`
  output bound (#771): their tests cover them.

## How it runs

- Build this clone: `CARGO_BUILD_JOBS=2 cargo build --release --locked --bin torlek --bin
  torlek-daemon --bin torlek-tui --bin torlek-claude-hook`.
- From the clone root, the facilitator asks Theo whether he is ready, then hands the guide over:
  `torlek ask guide < docs/acceptance/m2.md`. Its `run:` steps call `tools/acceptance/run.sh`.
- Isolation from the installed Torlek: `run.sh` sets the profile `acceptance` and scratch
  `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR` and `TMUX_TMPDIR` under `/tmp/torlek-acceptance-<uid>`
  (short, for the sockets; it also holds the small scratch projects), and `XDG_STATE_HOME`,
  `XDG_CACHE_HOME` under the clone's `target/acceptance` (what grows stays off the `/tmp`
  tmpfs). It unsets `TORLEK_SESSION`, `TORLEK_LAUNCH_TOKEN`, `TORLEK_INSTRUCTIONS`, `GH_TOKEN`
  and `GITHUB_TOKEN`, so a session of the installed Torlek lends the walk neither its identity
  nor a token.
- The scratch configuration sets `agents.sandbox = "off"`: a sandbox would need this profile's
  own engines and a managed proxy, whose fixed ports the installed Torlek's proxy may hold.
  Claude runs with the owner's login, outside greywall, in the scratch project only, as in the
  M1 walk. It also sets `storage.target_prune_base_ref = "refs/heads/main"`, so a clone at
  main's commit is eligible for a prune.
- The demo project ignores `target/` and declares one check, `checks` (`./greet.sh | grep -qi
  hello`), in an untracked `.torlek/config.toml`; `review` is left undeclared so only the
  owner can waive it. `run.sh big-needs on` adds a `[needs]` of 40% of the CPU budget.
- Every stuck step or surprise becomes an issue (label `acceptance`, milestone
  v0.1.0-alpha.3). The gate closes when each is fixed and its step re-run, or deferred in
  writing on #46.

## Runs

### 2026-11-05, facilitator dry run (not Theo's walk)

- Commit `7db12bf4`, Linux, inside the facilitator's session, a stand-in `claude` (prints a
  version, reports through `torlek agent report`, then sleeps), scratch home `/tmp/ta3dry`.
- 1, start: pass after two runner fixes. The daemon printed ready after more than 10 s on a
  fresh home: the wait is now 30 s. Owner verbs from inside a session were refused (#729
  working): `run.sh` now unsets the session identity.
- Launches: refused until the scratch configuration set `agents.sandbox = "off"`. With
  `claude`, `codex` and `opencode` installed, `session new` needs `--agent`.
- Todo, image note, queue and explanation through the CLI: pass. `queue explain ITEM` before
  the first scheduling pass said the pass found it "placed, being done, finished or gone"
  while no pass had run; the second answer was right.
- Code-change: `run.sh code-change` filed the item and started the workflow; the stand-in
  never ends a turn, so placement and the agent's steps need Theo's walk.
- Admission: session launches are not charged; queued items are (needs plus 50% startup
  headroom). Needs from the demo project's file showed in `config show --project`.
- Storage: kept while any session runs (decided in
  [the #406 prune verdict](../decisions/2026-11-04-406-prune-verdict-and-unpushed-guard.md)),
  and kept without an ended session or a base ref, and while `target/` was not ignored. With
  the base ref and `.gitignore`: report 25165824 bytes eligible, prepare printed the confirm
  line, confirm removed 25165824 bytes, report after: 0 bytes, target absent.
- Finding, [#845](https://github.com/Androlax2/agent-cockpit/issues/845): while preparing this
  run, the runner's tests killed the installed Torlek's tmux server twice. `run.sh stop` ran
  `tmux -L torlek kill-server`; with the scratch `TMUX_TMPDIR` directory missing, tmux 3.8 fell
  back to `/tmp`, where the installed server lives. The daemon resumed two sessions; others
  running then may have been lost. `run.sh stop` now kills only sockets that exist under the
  scratch `TMUX_TMPDIR`, by path, and the tests run a recording `tmux`.
