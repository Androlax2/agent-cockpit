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
| Start a worker | `torlek agent start DIR --clone BRANCH ...` (`start-sessions`); waits for the owner's approval |
| Give a worker the owner's answer | `torlek ask run --reason TEXT "torlek session send SESSION 'TEXT'"`; `torlek session send` itself is refused from a session |
| End a worker | `end_prune.sh` through `torlek ask run`; `torlek agent end` needs an `end-sessions` grant and a todo marked done |
| Anything else only the owner may run | `torlek ask run --reason TEXT COMMAND`; the owner approves that exact command once |

Starts are capped (sessions not yet ended, and starts per hour); the refusal names the cap.
A pending ask is not an approval: wait on the same id with `torlek ask run --wait ID`, never
send a second one.

## Start a worker

1. Pick the issue from the current milestone's queue. When a milestone is code-complete,
   report that and wait: never start the first worker of the next milestone without the
   owner's word. Flaky-test and performance fixes may keep going.
2. Choose model and effort (see "Model and effort").
3. Build the prompt: `Work issue #N (gh issue view N -R OWNER/NAME, with comments): <one
   line of what to deliver>.` plus any scope notes, then a space and the contents of
   [worker_suffix.txt](prompts/worker_suffix.txt). The suffix tells workers to run cargo
   directly, review once, open the PR, arm auto-merge, and stay until merged. Workers never
   use the old cockpit tooling (`agent-build`, `cockpit-*`, make targets that call them).
4. Start it. The clone directory is the branch name with `/` turned into `-`:

   ```sh
   torlek agent start PROJECT_DIR --clone greenfield/<issue>-<slug> \
       --model opus --effort high --prompt "$PROMPT"
   ```

5. Within about 2 minutes `torlek agent sessions` should show the session as `working` with
   a label. A session stuck at state `-` may wait on a harness dialog in its pane: tell the
   owner right away.

## Watch

Run [coordinator_watch.py](scripts/coordinator_watch.py) under your harness's monitor so each
output line wakes you:

```sh
python3 .agents/skills/coordinator/scripts/coordinator_watch.py "$SESSION_TEMP/watch-state.json"
```

It wakes you only for these lines; everything else is one `SUMMARY` every 10 minutes:

| Line | Action |
| --- | --- |
| `QUESTION` | Relay it (below) |
| `CI ... PR` on a new head commit | Read the failure (below); tell the worker only if it has not moved within one turn |
| `PR ... MERGED` or `CLOSED` | Close out (below) |
| `IDLE without merged PR` | Message the worker once; if it does not act by its next turn end, tell the owner |
| `WATCH ENDED` | The monitor exited 1. Re-arm it; the state file keeps what was reported |

State survives a re-arm. After a reboot, Torlek resumes workers but each sits idle until
messaged: re-arm the watcher, `torlek agent message` each worker to continue, and check which
PRs merged meanwhile.

## Questions from workers

A question from a worker you started waits on you first (`held-by=` your session in
`torlek agent sessions`). Answer only what the instructions you gave already settle. A
decision is the owner's: relay it, or run `torlek agent escalate SESSION`. It reaches the
owner by itself after 15 minutes. A queued message is not delivery: `torlek agent sessions`
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

Check whether the failure is the PR's or main's drift or a flaky test; file a flaky test as
an issue. Ignore `greenfield-macos` (macOS is not supported yet; the watcher already does).

## When a PR merges

1. The PR is merged on main: `gh pr list --head BRANCH --state merged`.
2. The issue is closed. If it is still open, compare the written acceptance with what merged
   and close it only when it is met; otherwise say what remains.
3. The clone is clean and pushed: `git -C CLONE status --short` is empty and nothing is
   missing from the pushed branch or main. A squash merge leaves the branch looking ahead; that
   is fine.
4. End the worker and prune its clone through the owner:

   ```sh
   torlek ask run --reason "End the finished worker and delete its clone's target/ to free disk" \
       ".agents/skills/coordinator/scripts/end_prune.sh CLONES_ROOT SESSION -- CLONE_DIR_NAME"
   ```

   `CLONES_ROOT` is the folder holding the clones (the watcher prints it as `workers under`).
   The script skips a clone a process still works in, and refuses names with a slash. Torlek
   also sweeps a clone when its session ends; the prune covers `target/` of idle clones.
5. Log the trial row (see "Model and effort") in your own notes.

## Decision issues

A decision issue has no product code: a report, then the owner's decisions, then changes to
the issues it covers. Decisions that need sandbox access (a socket, a credential, a network
protocol, a display) are decided in the sandbox milestone before anyone implements them: file
a separate decision issue there and link the implementation issue to it.

1. Fill the placeholders of [decision_prompt.txt](prompts/decision_prompt.txt) (the reports to
   read first, the related issues, the date, issue and slug), append the worker suffix, start a
   worker. Research reports have gone cleanly on Sonnet at high effort.
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
  reports and contained features with clear acceptance (up to about 500 lines); Fable on
  about one non-trivial task in three, mixing risky and contained work.
- Effort: medium for single-file or doc work; high for the rest; xhigh only when the owner
  asks or for a costly design call.

Rotation: about one start in three deviates by one step (Fable instead of Opus or Sonnet,
medium instead of high, low on a trivial docs change) so every cell collects samples. Never
deviate downward on security or data-loss work. When a worker's PR merges, add one row to your
own log: issue, PR, model, effort, kind (risky, contained, docs, research), lines added, CI
fix rounds as the watcher saw them (`gh run view` overwrites failures on rerun), review passes,
open-to-merge time, notes. Re-compare every ten new rows, update the rule and tell the owner
the change in one line.
