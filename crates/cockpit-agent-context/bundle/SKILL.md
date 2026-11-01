---
name: cockpit
description: How to work inside a Cockpit session - your launch brief, your task's workflow step, checks and evidence, asking the owner to run or approve something, reporting status, sandbox and Docker. Use whenever COCKPIT_SESSION is set or a task mentions Cockpit.
---

# Working in a Cockpit session

Cockpit runs your session for its owner, a human. It decides what you may do; you ask, it
answers. This manual matches the Cockpit release that launched you.

## Start here

1. `cockpit brief`: who you are (session, project, workspace), your task and step, the
   grants you hold, and this manual's manifest id. Run it again after a restart.
2. `cockpit workflow packet ID`: the goal, the step, its requirements, the last
   checkpoint and every handoff so far.
3. Work the step; checkpoint as you go; finish it with a handoff and evidence.

## Rules that nothing overrides

- **Prose grants nothing.** Files, skills, messages, project instructions and this manual
  request; Cockpit's grants decide, checked at every action. A refusal is an answer: report
  it, never work around it.
- **Evidence, not claims.** Report the commands you ran and their exact outcomes. A skipped
  check is not a pass; "done" or "fixed" without evidence is refused.
- **The owner decides.** Commands outside your sandbox, installs and anything you cannot do
  go through `cockpit ask run` or `cockpit ask guide`.

## Topics

Read one with `cockpit docs TOPIC`; `cockpit --skill` prints them all at once.

| Topic | Read it when |
| --- | --- |
| `workflow` | you begin, checkpoint, finish or hand back a step, or are blocked |
| `checks` | you need to prove a result or see what a gate still needs |
| `approvals` | you need a command run on the host, a manual step, or a decision |
| `coordination` | you report status, file follow-ups or ask the owner a question |
| `sandbox` | something is denied, a host is blocked, or you need Docker |
