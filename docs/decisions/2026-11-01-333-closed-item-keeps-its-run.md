# A workflow whose item closes outside it keeps running and offers no step

Issue #333. Decided by the worker (the issue left the choice open), for the reasons below.

- **The run stays `running`; `ready_steps` skips a run whose item is not `todo` or
  `doing`, or no longer exists.** One read-time rule covers every path that closes an item,
  with no hook in each task verb and no call from tasks to workflows. Proof:
  `cockpit-workflows` `runs::a_run_whose_item_closed_outside_it_offers_no_ready_step`.
- **Reopening the item resumes the run at its checkpoint.** Proof:
  `runs::a_reopened_item_offers_its_runs_step_again`.
- **The packet of a run whose item was deleted still answers**, with the item's number and
  no goal (`goal: None` on the wire), so the owner sees what the run did and cancels it.
  Proof: `runs::the_packet_of_a_run_whose_item_was_deleted_answers_without_a_goal`,
  `cockpit-daemon` `workflows::the_packet_of_a_run_whose_item_was_finished_then_deleted_still_answers`.
- **"Declined" does not arise:** only a proposal can be declined, and a run starts only on an
  open item.
- **Rejected: cancel the run when its item closes.** An agent that finishes its item before it
  completes the last step would find the run cancelled and its delivery lost, and a reopened
  item would lose its checkpoint.
