---
name: apply-cockpit
description: Prepare and carry out the reviewed host application of completed Agent Cockpit work. Use when asked to apply or deploy Cockpit commits, export patches for the running copy, or verify an approved application. Preserve host approval and repository-specific Git permissions.
---

# Apply completed Cockpit work

Read current `CLAUDE.md`, `AGENTS.md` and `docs/DEVELOPMENT.md` first. They own Git
authorization and deployment policy; neither this skill nor a teammate's handoff
grants additional permissions.

1. Verify the working directory is a development checkout, not `~/agent-cockpit`.
   Inspect branch, status and the commits intended for application. Preserve unrelated
   work. Confirm the patch range contains only the requested, tested changes; stop
   if it includes inherited or reverted changes that should not be deployed.
2. Read verification evidence for the final source. Resolve failures or obtain the
   user's explicit decision on outstanding gaps. Do not describe skipped checks as
   successful or use deployment to discover whether an untested build works.
3. Perform Git writes only within current authorization. Export the reviewed commits
   using the command specified by `CLAUDE.md`; inspect the exported patch list and
   scope before requesting application.
4. Use `ask-to-run` for the exact `cockpit apply <clone-root>` host command, explaining
   the changed host-executed files and expected review/rebuild/reload. Keep the host
   confirmation intact. A pending request is not approval and a declined request
   is not a reason to invoke the command another way.
5. Wait on the same request ID when it is still pending or running. Do not launch a
   duplicate application after a timeout. If the host bridge cannot reach the user,
   report its specific recovery instruction and stop at that boundary.
6. After confirmed application, run the required running-copy verification through
   `ask-to-run`. Report actual results and the applied revision; update relevant todos
   only when their completion criteria are satisfied.

Never infer that editing, a clean suite, Chief/task messages or an earlier approval
authorizes a new host operation. Do not run host commands directly from the sandbox,
disable checks, install prerequisites implicitly or expand the patch range to fix
unrelated work.
