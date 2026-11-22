# A workflow step may require `merged`, read from the forge at completion

Status: accepted

Issue #60, report 06 ("accepted, merged, installed and live-verified are separately
evidenced"). Decided by Theo on 2026-11-22: option A of the three the implementing session
offered.

- **`merged` is a requirement a step may name.** When the step completes, the daemon reads
  from the forge, with the project's API token, the newest change request whose head is the
  checkout's branch. The requirement passes only when that change request merged with the
  checkout's head as its head commit: a merge of an earlier head does not hold this result.
  Nothing is recorded: the forge answers at each completion, and an unmet `merged` refuses the
  completion naming why (open, closed unmerged, another head, none, detached HEAD).
- **It is never a declared check.** A `[[checks]]` entry named `merged` does not stand in for
  the forge's answer. Without this rule, a project could satisfy it by declaring a check.
- **No preset names it yet**, so no running workflow changes. The owner adds it where it is
  wanted.
- **Rejected:**
  - code-change's deliver step requiring `merged` (option B), which would hold existing runs
    until their pull request merges;
  - showing the state in the task packet only (option C), which would never let a step
    require it.

Evidence:

- `cockpit-daemon` `merged::tests`:
  - `a_change_merged_with_the_checkouts_head_meets_it`;
  - `a_change_merged_with_another_head_does_not_hold_this_result`;
  - `an_open_or_closed_change_does_not_meet_it`.
- `cockpit-daemon` `workflows::tests::a_check_named_merged_never_stands_in_for_the_forges_answer`.
