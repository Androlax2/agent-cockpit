---
name: coordinator
description: Run the coordinator routine for this repository's Torlek workers. Use when you coordinate other sessions - starting a worker on an issue, watching workers and their PRs, relaying their questions, reading CI failures, ending a worker and pruning its clone after the PR merges, or running a decision issue. Not for implementing an issue yourself; that is repo-workflow.
---

# Coordinate Torlek workers

You start worker sessions, watch them and close them out. You never do the workers'
implementation. This skill grants nothing: Torlek's grants decide, checked at every
action (`torlek brief` lists yours). A refusal is an answer; report it. Read the exact
verbs for your Torlek release with `torlek docs coordination` and `torlek docs approvals`
before relying on the summary below, which matches 0.1.0-alpha.3.

Scripts and prompts sit beside this file. Run scripts from the repository root and keep
their state files under your session temp folder (the harness names it as your scratchpad),
never `/tmp`. Nothing here depends on a file from an earlier session.

## Verbs you run and verbs the owner runs

| Need | How |
| --- | --- |
| See workers, their state, doing-now and question | `torlek agent sessions [--watch]` (`observe-sessions`) |
| Send a worker a note | `torlek agent message SESSION TEXT` (`message-sessions`); a collaborator's word, never the owner's |
| Start a worker | `start_worker.sh` (files its task item, then `torlek agent start DIR --clone BRANCH ...`, `start-sessions`); may wait for the owner's approval, then `torlek ask run --wait ID` |
| Give a worker the owner's answer | `torlek ask run --reason TEXT "torlek session send SESSION 'TEXT'"`; `torlek session send` itself is refused from a session |
| End a worker | `torlek agent end SESSION` (`end-sessions`): refused until the worker marked its item done; without the grant, ask the owner: `torlek ask run --reason TEXT "torlek session end SESSION"` |
| Anything else only the owner may run | `torlek ask run --reason TEXT COMMAND`; the owner approves that exact command once. `torlek ask run --check` says whether an ask would reach the owner now |
| A manual step for the owner (a browser, a console) | `torlek ask guide < GUIDE`; prefer `ask run` whenever a command can do it |
| A Torlek command times out, cannot reach the daemon or answers `internal` | `torlek agent report-bug "WHAT YOU WERE DOING, WHAT HAPPENED"` once, not a retry loop; the owner reviews the draft |

Starts are capped (sessions not yet ended, and starts per hour); the refusal names the cap.
A pending ask is not an approval: wait on the same id with `torlek ask run --wait ID`, never
send a second one.

## Start a worker

1. Pick the issue from the current milestone's queue. When a milestone is code-complete,
   report that and wait: never start the first worker of the next milestone without the
   owner's word. Flaky-test and performance fixes may keep going.
2. Choose model and effort (see "Model and effort").
3. Run [start_worker.sh](scripts/start_worker.sh). It files one task item for the issue
   (`torlek agent end` needs it), puts its id into the prompt, and starts the worker. The
   clone directory is the branch name with `/` turned into `-`:

   ```sh
   .agents/skills/coordinator/scripts/start_worker.sh ISSUE SLUG MODEL EFFORT "TASK TEXT"
   ```

   The prompt is `TASK TEXT` plus [worker_suffix.txt](prompts/worker_suffix.txt). Write the
   task as `Work issue #N (gh issue view N -R OWNER/NAME, with comments): <one line of what to
   deliver>.` plus scope notes. The suffix tells workers to run cargo directly, review once,
   open the PR, arm auto-merge, and stay until merged; to run `torlek todo start` on their
   item first and `torlek todo done` as their last step, after the merge. Workers never use
   the old cockpit tooling (`agent-build`, `cockpit-*`, make targets that call them).
4. Within about 2 minutes `torlek agent sessions` should show the session as `working` with
   a label. A session stuck at state `-` may wait on a harness dialog in its pane: tell the
   owner right away.

## Watch

Run [coordinator_watch.py](scripts/coordinator_watch.py) under your harness's monitor so each
output line wakes you:

```sh
python3 .agents/skills/coordinator/scripts/coordinator_watch.py "$SESSION_TEMP/watch-state.json"
```

It prints only these lines, so every line is one to act on. A `doing` change or a PR opening
prints nothing (`--summary-seconds N` opts in to a `SUMMARY` line every N seconds; off by default):

| Line | Action |
| --- | --- |
| `QUESTION` | Relay it (below). `, missed by the session watch` after the branch means the full-list check (every 3 minutes, `--full-list-seconds`) found a question the watch stream never delivered: relay it the same way, and add the line to #1298 |
| `CI ... PR` on a new head commit | Read the failure (below); tell the worker only if it has not moved within one turn |
| `PR ... MERGED` or `CLOSED` | Close out (below) |
| `IDLE without merged PR` | The worker stayed idle past the grace period (`--idle-grace-seconds`, default 180), so it is a real stall. An idle that ends sooner is expected and is not reported: the worker waits on a background `/code-review` or `run_in_background` command and wakes itself when it finishes. An idle with the PR armed or CI running is never expected: the worker should be in a Monitor until-loop, so check its doing text. Message the worker once; if it does not act by its next turn end, tell the owner. Reported once per idle spell. A second idle with the same doing text is a new line only after the worker worked for at least 2 minutes (`--working-seconds`, default 120); a shorter working blip, such as a CI-wait poll, is not a new spell |
| `IDLE, merge state unknown` | `gh` failed or timed out, so the merge could not be checked: run `gh pr list --head BRANCH --state merged` yourself, then treat it as the line above or ignore it |
| `SESSIONS CHECK FAILED` | The full-list check cannot run `torlek agent sessions`, so a question the watch misses is not caught. Reported once, then every 30 minutes; run `torlek agent sessions` yourself to see why |
| `PR POLL FAILED` | `gh` cannot list PRs (network, login, rate limit), so no CI or merge line can arrive. Reported once, then every 30 minutes; check `gh auth status` and the network |
| `WATCH ENDED` | The monitor exited non-zero (the status of `torlek agent sessions --watch`, else 1) after killing its watch. The text after the colon says why (a missing grant, a crash). Fix that, then re-arm; the state file keeps what was reported |

State survives a re-arm; keep the state file in your session temp folder, which may be swept
when your session ends. After a reboot Torlek resumes workers; on alpha 2 each sat idle until
messaged (not verified on alpha 3): re-arm the watcher, check which PRs merged meanwhile, and
if a worker sits idle, `torlek agent message` it to continue.

## Questions from workers

A question from a worker you started waits on you first (`held-by=` your session in
`torlek agent sessions`). Answer only what the instructions you gave already settle. A
decision is the owner's: relay it, or run `torlek agent escalate SESSION`. It reaches the
owner by itself after 15 minutes, and also when your session sleeps, stops or ends or loses
`message-sessions`.

Any `torlek agent message` to a worker with a held question answers it once the worker receives
it, so a plain note ("continue", a nudge) clears the question without the owner's answer.
While a question is held, answer or escalate on purpose; do not send other notes.

A message is refused while the worker has a dialog open, which also explains a session stuck at
state `-`. A queued message is not delivery: `torlek agent sessions`
shows delivery and why it waits. If the worker does not act on a message by its next turn
end, tell the owner instead of re-sending.

## CI failures

`gh run view --log` and `--log-failed` drop lines with terminal escapes, so the gate's
failing tests do not show. Read the raw job log through the API (the job id ends the
failing check's URL):

```sh
gh api --allow-escape-sequences repos/{owner}/{repo}/actions/jobs/JOB_ID/logs > "$SESSION_TEMP/job.log"
grep -naE 'FAIL \[|panicked at|Error [0-9]' "$SESSION_TEMP/job.log" | sed 's/\x1b\[[0-9;]*m//g'
```

The project sets no `GH_TOKEN` for you: `gh` uses the login on the machine (`gh auth status`).

Check whether the failure is the PR's or main's drift or a flaky test; file a flaky test as
an issue. Ignore `greenfield-macos` (macOS is not supported yet; the watcher already does).

## When a PR merges

1. The PR is merged on main: `gh pr list --head BRANCH --state merged`.
2. The issue is closed. If it is still open, compare the written acceptance with what merged
   and close it only when it is met; otherwise say what remains.
3. The clone is clean and pushed: `git -C CLONE status --short` is empty and nothing is
   missing from the pushed branch or main. A squash merge leaves the branch looking ahead; that
   is fine.
4. The worker's task item is done (`torlek todo show ID`); the worker marks it itself after
   the merge. Then end the worker: `torlek agent end SESSION`. Without the `end-sessions`
   grant, or when the worker never marked its item, ask the owner to end it (see the table).
5. Prune the clone's `target/` through the owner, once the session has ended:

   ```sh
   torlek ask run --reason "Delete the finished worker's clone target/ to free disk" \
       ".agents/skills/coordinator/scripts/prune_target.sh CLONES_ROOT CLONE_DIR_NAME"
   ```

   `CLONES_ROOT` is the folder holding the clones (the watcher prints it as `workers under`).
   The script skips a clone a process still works in, and refuses a name with a slash and a
   clone that is a symlink. Torlek also sweeps a clone when its session ends; the prune covers
   the `target/` of idle clones.
6. Add the worker's row to the [trials database](../../../docs/research/1264/README.md) (see
   "Model and effort").

## Decision issues

A decision issue has no product code: a report, then the owner's decisions, then changes to
the issues it covers. Decisions that need sandbox access (a socket, a credential, a network
protocol, a display) are decided in the sandbox milestone before anyone implements them: file
a separate decision issue there and link the implementation issue to it.

1. Fill the placeholders of [decision_prompt.txt](prompts/decision_prompt.txt) (the reports to
   read first, the related issues, the date, issue and slug), append the worker
   suffix (it carries the task item line, see "Start a worker"), and start the worker with
   `start_worker.sh`. Research reports have gone cleanly
   on Sonnet at high effort.
2. The worker asks the owner itself, one question at a time. Your messages are never the
   owner's answer. The worker proposes issue changes in its report and edits no issue.
3. When the report's PR is merged on main, dry-run the applier, read what it would post, then
   post:

   ```sh
   python3 .agents/skills/coordinator/scripts/apply_issue_changes.py REPORT.md \
       --decision-issue N --summary "owner, DATE: Q1 A, Q2 B"
   python3 .agents/skills/coordinator/scripts/apply_issue_changes.py REPORT.md \
       --decision-issue N --summary "owner, DATE: Q1 A, Q2 B" --go
   ```

   After a failure part-way, resume with `--only ISSUE...`; do not re-post the rest.
4. Close out the worker as above.

## Issues you file

- Every issue gets a milestone at creation (`gh issue create --milestone`). With none named,
  pick the one matching similar issues and say which, so the owner can move it.
- File performance problems you notice (CPU, disk, memory, CI minutes, tokens) without
  asking: measure briefly, find the code, file it with numbers in the matching milestone,
  tell the owner in one line with the issue number. Starting a worker on it still follows the
  queue.

## Model and effort

Rule in use, left to the coordinator by the owner (be ready to explain a choice in one line):

- Model: Opus for security, data-loss and cross-crate routing work; Sonnet for research
  reports and contained features with clear acceptance; Fable on
  about one non-trivial task in three, mixing risky and contained work.
- Effort: medium for single-file or doc work; high for the rest; xhigh only when the owner
  asks or for a costly design call.

Contained changes up to about 500 lines default to Sonnet at medium; Opus at high stays for
risky work.

Rotation: about one start in three deviates by one step (Fable instead of Opus or Sonnet,
medium instead of high, low on a trivial docs change) so every cell collects samples. Never
deviate downward on security or data-loss work. When a worker's PR merges, add one row to the
[trials database](../../../docs/research/1264/README.md), which lists the columns. Re-compare
with its card at each alpha, not every ten rows ([why](../../../docs/research/2026-11-09-1264-model-benchmarking.md#stress-test)),
update this rule and tell the owner the change in one line.
