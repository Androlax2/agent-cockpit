# Chief supervision of in-progress todos

How the protected Chief follows work it did not start to a verified end, and
where that still needs Theo. This describes the implemented interface, not
deployment or live acceptance. Source: `apps/cockpit/src/chief/store.rs`
(`supervised_tasks_record`, `observe_supervision`, `inspect`,
`record_todo_completions`) and `apps/cockpit/src/chief.rs`.

## What the Chief sees

Context carries one `supervised_tasks` record: at most 20 todos in progress,
12 kB, the rest counted in `omitted`. Each names the todo, its project, its
owner session and `owner_state` (`missing`, `gone`, `stopped`, `busy`, most
urgent first), `paused` for an owner put to sleep on purpose, the owner's
latest stop and the latest Chief or Theo delivery to it. A change to the
record is a `supervision_changed` event in the incremental feed and wakes the
Chief once; the Chief's own busy/idle turns never do.

The record reads the todo list, which agents write. It is no evidence and
grants no authority, dispatch, admission or pause change.

## When a todo leaves the list

Leaving the list is not completion: the todo may be done, back in To do,
removed, or pushed into `omitted` by more urgent todos. Only todos the Chief
filed come back as `todo_completed` with their result.

For any other todo that disappears, the Chief uses the existing read-only
`Inspect {todo}` action (at most `INSPECTIONS_PER_TURN` = 5 per admitted turn):

1. Inspect the todo id it last saw in `supervised_tasks`.
2. `status` and `result` say what the worker reported. The result is
   `session_report_not_evidence`: check each cited commit, test run or host
   receipt before treating the work as finished.
3. Done with verified evidence: record it. Done without evidence, or with a
   failed check: ask the owner (`Message`) for the missing step, or escalate.
   Back in To do or removed: report it; do not restart it on its own.

The Chief never polls: it inspects on the wake the change already caused.

## Known limits

These are real gaps, not things the fallback covers:

- **Omitted todos.** A todo beyond the first 20 is only a count. When it
  finishes, the Chief cannot know its id, so it cannot inspect it.
- **More than five disappear at once.** The rest wait for the next admitted
  wake; no new wake is raised for them.
- **GitHub issues.** `supervised_tasks` lists in-progress issues, but `Inspect`
  only finds local todo items.
- **Done items expire.** The todo list keeps done items for a week; past that,
  `Inspect` answers that no todo has the id.

Carrying the result of every finished supervised todo to the Chief (proposal
75a9bb8d) would automate step 2. It forwards other projects' results, so it
waits for Theo's scope and privacy decision; the fallback above works without
it.
