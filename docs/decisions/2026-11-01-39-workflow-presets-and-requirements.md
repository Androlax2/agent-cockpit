# Workflow presets, their default, and who meets a requirement

Issue #39. Decided while implementing `cockpit-workflows` (2026-11-01), within report 06 and
the ownership sentence "next required step and its progress; never calls the scheduler".

- **Three presets, versioned in code.** `research`, `code-change` and `design-led`, each
  version 1. A run stores its preset's name and version; a stored run whose preset version
  this build lacks fails loud instead of being read against another version.
- **Default preset.** The owner's named preset, else the project's `workflow.preset`, else
  `research` for a research item and `code-change` for every other kind. A preset name no
  preset has is refused naming the known ones, never replaced by the default. Proof:
  `cockpit-workflows` `presets::without_a_name_a_research_item_follows_research`,
  `cockpit-daemon` `workflows::the_owner_starts_a_workflow_with_the_preset_the_project_asks_for`
  and `a_project_preset_no_preset_has_is_refused_for_the_owner_to_fix`.
- **Capabilities before dispatch.** Every agent step declares `events` and `outcome`; the
  daemon checks them against the session's provider when the session begins the step. An
  owner step (design approval) declares none.
- **Requirements are met by verification or waived by the owner, never claimed.** A step
  names the evidence it needs (`checks`, `review`, `design-match`). `cockpit-workflows`
  takes the names that passed from its caller and never evaluates evidence; that is
  `cockpit-verification` (#40), and the boundary is the `Completion { satisfied, waived }`
  type. The wire has no `satisfied` field, so no request can claim one passed; until #40
  is wired into the daemon, such a step completes only with the owner's `--waive`. Handoff
  prose never meets a requirement. Proof: `cockpit-daemon`
  `workflows::a_step_with_requirements_completes_only_with_the_owners_waiver`,
  `cockpit-protocol` `workflows::a_requirement_a_completion_claims_passed_never_reaches_the_daemon`.
- **Checkpoints are the doing session's.** The owner completes or releases a step; it does
  not write a session's checkpoint.
- **Rejected: step definitions stored in the database or project config.** "Configurable
  per project" (report 06) is met by choosing among the shipped presets; a preset's steps
  are code, reviewed in pull requests, so a stored run always reads against a definition
  this build has.
