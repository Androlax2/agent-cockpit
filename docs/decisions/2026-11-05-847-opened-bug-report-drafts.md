# A draft whose issue a send handed the owner is marked opened, and the TUI records its issue in place

Issue #847. Decided by the implementing session: the issue listed these options in the order to
consider and asked for the first and third together.

- **`send` marks the draft opened.** `torlek bug-report send ID` opens the prefilled issue in
  the browser, or prints it with `--print`, then sends `bug-report-opened`. The daemon stores
  when (`bug_reports.opened_at`) and the draft stays a draft: the reason of decision #427
  holds, a browser proves nothing was filed. `bug-report list`, `show` and the feed carry
  `opened_at`. A send that cannot mark the draft fails, so the owner knows the lists will not
  show it.
- **`B` lists an opened draft apart from the waiting ones and records its issue.** `j`/`k`
  select a draft, Enter asks for the issue and sends `bug-report-sent`. The needs-you card
  counts the two kinds apart. Any draft can be recorded, opened or not: a draft submitted
  before this change, or from a URL copied by hand, has no `opened_at`.
- **`opened_at` outlives an edit and a merged occurrence.** It stays true that a send handed
  the owner the issue; clearing it would list the draft as waiting again, which is #847. A
  report recorded sent keeps it too, and is opened no more (`BugReportView::is_opened`).
- **An issue is recorded by its URL or its number.** `bug-report-sent` takes `844` or `#844`
  for the repository reports go to (`cockpit_bug_reports::REPOSITORY`), in the TUI and in
  `send ID --filed`. The owner has the number in front of them after submitting.
- **The send itself stays in the CLI**, as decision #471 has it: the browser and `gh` belong
  to the CLI, and the TUI starts no program.
- **Rejected:** the daemon matching the fingerprint against the repository's issues with `gh`
  (option 2): the daemon never touches the network (decision #427), and an unspecific
  fingerprint matches every such report.
- **Rejected:** a fourth status beside `draft`, `sent` and `discarded`: the daily limit and
  every "only a draft changes" rule would need the new status too, for a fact that changes
  none of them.

Tests: `crates/cockpit-tui/tests/bug_reports.rs` (the state model), `crates/cockpit-cli/tests/bug_report.rs`
(the send), `crates/cockpit-daemon/tests/bug_reports.rs` (the feed).
