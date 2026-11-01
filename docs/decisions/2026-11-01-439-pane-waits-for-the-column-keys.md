# The approval pane takes no key while the column focus it asked for is unanswered

Issue #439. Decided by the implementing session, within the pane rules of #166 and #178:
the issue's own fix idea, made exact.

- **The pane stays in its grace, hint on "Read it first", until every `manager-focus` for the
  column it sent is answered.** The answer starts the grace again (the keys tmux brings may
  be the owner's typing to an agent, #317). Before, a slow answer could land after the first
  grace ended. The hint had already offered `y`, then the answer took it back and the next
  `y` was ignored. Now the hint offers keys only once no answer can take them back.
- **A count, not a flag.** The pane can ask twice: a request shows, the queue empties, the
  next one shows, and both focus answers are on their way. A flag cleared by the first answer
  would let the second restart the grace after the hint, which is the same bug.
- **A failed focus also ends the wait and starts the grace again.** The runtime answers every
  command, with the reply or a failure at its deadline (`REQUEST_DEADLINE` in
  `crates/cockpit-tui/src/main.rs`), so the pane never waits forever. After a failure the
  keys are where they were; the owner moves them, and one grace from then is the safe side.
- **Rejected:** not restarting the grace on a late answer. Keys the owner was typing to an
  agent would then reach the pane at once and could approve.

Evidence: `crates/cockpit-tui/tests/pane.rs`
(`while_the_column_s_keys_are_on_their_way_the_pane_says_to_read_first`,
`a_key_before_the_column_s_keys_arrive_is_ignored`,
`once_late_keys_arrive_the_first_y_after_the_grace_approves`,
`a_failed_column_focus_lets_the_pane_take_keys_after_a_grace`,
`the_pane_reads_first_until_every_column_focus_it_asked_for_is_answered`).
