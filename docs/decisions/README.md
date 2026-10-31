# Decision records

One file per decision, so parallel pull requests never edit the same file.
`docs/DECISIONS.md` is the frozen history up to 2026-10-31; nothing is added there.

## Naming

`docs/decisions/YYYY-MM-DD-<issue>-<slug>.md`, for example `2026-10-31-29-process-containment.md`.
The date is the day the decision was made, `<issue>` the GitHub issue it belongs to, the slug
lower-case words joined by `-`. The directory listing is the index; no index file to maintain.

## Content

```markdown
# <Decision in one sentence>

Issue #<n>. Decided by <Theo | the worker, with the reason>.

- **<Rule or choice.>** Why, and what it rules out.
- **Rejected:** <alternative> because <reason>.
```

Keep it to what a reader needs to not reopen the question: the choice, the reason, the
alternatives rejected. Code and tests are the evidence; link them by path.

The crate-graph rules crate checks the file names (`crates/cockpit-workspace-rules/tests/decision_records.rs`).
