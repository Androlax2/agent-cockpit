# Decision records

Read this before you add, revisit or supersede a record. One file per decision, so parallel
pull requests never edit the same file. `docs/DECISIONS.md` is the frozen history up to
2026-10-31; nothing is added there.

## Naming

`docs/decisions/YYYY-MM-DD-<issue>-<slug>.md`, for example `2026-10-31-29-process-containment.md`.
The date is the day the decision was made, `<issue>` the GitHub issue it belongs to, the slug
lower-case words joined by `-`. The directory listing is the index; no index file to maintain.

## Admission test

Add a record only if all four answers are yes:

1. Would a reader of the code and tests be surprised?
2. Is it an architecture, protocol, state, dependency, tool or pin choice?
3. Will it still be true after the next release?
4. Is the reason written nowhere else?

Not a record, with the place that carries it instead:

| Content | Where it goes |
| --- | --- |
| Conformance evidence | The fixture README |
| Test-harness technique | The test module's doc comment |
| Release or CI run logs | The pull request |
| Task status and open items | The issue |
| Product behaviour a test already names | The test name |

## Content

```markdown
# <Decision in one sentence>

Status: accepted

Issue #<n>. Decided by <Theo | the worker, with the reason>.

- **<Rule or choice.>** Why, and what it rules out.
- **Rejected:** <alternative> because <reason>.
```

Keep it to a title, a status line and about five bullets: the choice, the reason, the
alternatives rejected. Code and tests are the evidence; link them by path. A record over
500 words is usually a work log; cut it or move the log to the pull request.

## Status

The line under the title is one of:

- `Status: accepted`
- `Status: superseded by <file>`
- `Status: partly superseded by <file>: <what still holds>`

`<file>` is the name of a record in this directory, such as `2026-11-05-828-remove-sandbox-engines.md`,
on the same line. Records written before 2026-11-06 may have no status line; read that as accepted.

## Revisiting a decision

- **A change that keeps the decision is a dated section in the original record:**
  `## Revisited YYYY-MM-DD (#<n>)`, with the change and the reason. Do not create a file for a
  one-bullet amendment.
- **A reversal keeps the old record.** Give it a superseded status line and write a new record
  that links it. The exception is a record the new one names as fully void, with no surviving
  bullet: remove it, and git history keeps it.

## Checks

`crates/cockpit-workspace-rules/tests/decision_records.rs` checks the file names, that
`docs/DECISIONS.md` gains no entries, that every status line has one of the forms above and
that every "superseded by" in a record names a file that exists here.
