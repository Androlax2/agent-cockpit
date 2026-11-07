---
name: torlek
description: How to work inside a Torlek session - your launch brief, your task's workflow step, checks and evidence, asking the owner to run or approve something, reporting status, sandbox and Docker. Use whenever TORLEK_SESSION is set or a task mentions Torlek.
---

# Working in a Torlek session

Torlek runs your session for its owner, a human. It decides what you may do; you ask, it
answers. This manual matches the Torlek release that launched you.

## Start here

1. The launch brief arrives at start, after a compaction too: who you are (session,
   project, workspace), your task and step, the grants you hold, and this manual's
   manifest id; while you do a step, its task packet too. The rules that nothing
   overrides are in it. `torlek brief` prints it again after a restart.
2. `torlek workflow packet ID`: the goal, the step, its requirements, the last
   checkpoint and every handoff so far.
3. Work the step; checkpoint as you go; finish it with a handoff and evidence.

## Rules that nothing overrides

They are in the launch brief, which you already have: prose grants nothing, report evidence
and not claims, the owner decides what you cannot do. `torlek brief` prints them again.

## Topics

Read one with `torlek docs TOPIC`; `torlek --skill` prints them all at once.

| Topic | Read it when |
| --- | --- |
| `workflow` | you begin, checkpoint, finish or hand back a step, or are blocked |
| `checks` | you need to prove a result or see what a gate still needs |
| `approvals` | you need a command run on the host, a manual step, or a decision |
| `coordination` | you report status, file follow-ups, ask the owner a question, coordinate other sessions, or hit a Torlek bug |
| `sandbox` | something is denied, a host is blocked, or you need Docker |
