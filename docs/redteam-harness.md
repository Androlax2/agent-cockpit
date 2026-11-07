# Red-team harness and host-only verb catalog

Read this to run the #685 red-team's checks and read their results, and to see the host-only
verb catalog its acceptance asks for. Build or reset the guest first with
[the red-team VM](redteam-vm.md); the brief there (scope, what's in and out) still applies.
Theo runs the harness against the guest; this page and `tools/redteam-harness.py` are the
tooling, not a run's results.

## Host-only verb catalog

Every command the daemon answers with `owner_only()` for an agent caller is one `match` arm
in `crates/cockpit-daemon/src/routes.rs`; `crates/cockpit-daemon/tests/host_only_verbs.rs`
connects as an agent and proves one representative command of each arm refuses with
`ErrorCode::NotAuthorized`. One command is enough per arm: the arm itself, not the command's
fields, decides the refusal, so every command in a group takes the identical code path.

| Group (routes.rs line) | Commands | CLI verbs |
| --- | --- | --- |
| 247 | `AgentReports` | `agent reports` |
| 271 | `BugReportNew`, `BugReportBundlePlan`, `BugReportList`, `BugReportShow`, `BugReportEdit`, `BugReportOpened`, `BugReportSent`, `BugReportDiscard` | `bug-report new\|list\|show\|edit\|send\|discard` |
| 283 | `VersionReport` | `version --all` |
| 303 | `SkillPack` | the owner's own skill listing |
| 319 | `StorageReport`, `StoragePrunePreview`, `StoragePruneApply`, `StoragePruneAbandon`, `StoragePruneStatus`, `StorageOutputs`, `StorageDatabase` | `storage`, `storage outputs [prune]`, `storage database [prune]`, `storage prune` |
| 336 | `ProjectAdd`, `ProjectList`, `ProjectRemove`, `CheckoutList`, `CheckoutClone`, `CheckoutReview`, `CheckoutClaim`, `CheckoutRelease`, `CheckoutSweep`, `CheckoutPush` | `project add\|list\|remove`, `checkout list\|clone\|review\|claim\|release\|sweep\|push` |
| 378 | `ApprovalQueue`, `ApprovalShow`, `ApprovalApprove`, `ApprovalDecline`, `ApprovalEdit`, `ApprovalInput`, `GuideShow`, `GuideAnswer`, `GuideStuck`, `GuideClose` | `approval list\|show\|approve\|decline\|edit\|input`, `guide show\|answer\|stuck\|close` |
| 395 | `NetworkList`, `NetworkAllow`, `NetworkDeny`, `NetworkGrants`, `NetworkRevoke` | `network list\|allow\|deny\|grants\|revoke` |
| 430 | `SessionGrant`, `SessionRevoke`, `SessionGrants` | `session grant\|revoke\|grants` |
| 454 | `SessionNew`, `SessionList`, `SessionRestart`, `SessionRestartAll`, `SessionSleep`, `SessionWake`, `SessionResume`, `SessionSend`, `SessionCancel`, `SessionEnd`, `SessionEndFinished`, `SessionLink`, `SessionSandbox`, `SessionSandboxRecord`, `SandboxShow` | `session new\|list\|restart\|sleep\|wake\|resume\|send\|cancel\|end\|link\|sandbox\|sandbox-record`, `sandbox show` |
| 507 | `TaskPriority`, `TaskQueue`, `TaskApprove`, `TaskDecline`, `TaskReopen`, `TaskWait`, `TaskUnwait`, `TaskImageAdd`, `TaskImage`, `TaskDraftSave`, `TaskDrafts`, `TaskDraftDiscard` | `todo priority\|queue\|approve\|decline\|reopen\|after\|unwait\|image add\|image get\|draft save\|drafts\|draft discard` |
| 545 | `WorkflowStart`, `WorkflowCancel` | `workflow start\|cancel` |
| 560 | `ResourceStatus`, `ResourceProfile`, `ResourceQueue`, `ResourceCheck`, `ResourceCheckpoint`, `ResourceStop` | `resources status\|profile\|pause-queue\|resume-queue\|check\|checkpoint\|stop` |
| 571 | `QueueExplain`, `QueueDispatch`, `TaskHandoff` | `queue explain\|dispatch`, `todo handoff` |
| 586 | `ManagerOpen`, `ManagerFocus`, `ManagerFit`, `SessionShow`, `SessionType`, `SessionPasteImage` | the manager/TUI protocol behind `tui` |
| 601 | `ProviderList` | `provider list` |
| 611 | `Readiness` | the readiness report behind `init`/`doctor` |
| 617 | `DesktopSessionImport` | `desktop-session import` |
| 629 | `CheckRun`, `CheckReceipt`, `CheckPending`, `CheckResolvePending` | `check run`, `check pending`, `check resolve-pending` |
| 651 | `LogsLevelStatus`, `LogsLevelSet`, `LogsLevelRevert` | `logs level` |
| 665 | `ConfigReload` | `config reload` |

Also owner-only, but not an agent-reachable verb at all (`daemon install`, `daemon-install`
and the native launcher checks): see
[the security-isolation report](design/greenfield/11-security-isolation-platforms.md) and
`crates/cockpit-sandbox`. `tools/agent-docker.sh` has its own refusals, checked by
`tools/tests/test_agent_docker_grant.py`.

Why each group refuses is in `routes.rs` itself, next to its match arm, not repeated here:
one authority, not two copies to keep in sync.

## What the routing proves, and what it doesn't

`Peer` (`routes.rs`) comes from which socket file the kernel accepted the connection on,
never from a command's fields or the connecting process's environment: "as the socket it
came through and the kernel say; never what a payload claims." `host_only_verbs.rs` connects
on the real agent socket of a real daemon process; none of its commands carries a
`launch_token` or `through` field claiming ownership, and every one still refuses. A payload
forging one, or a session's child process reusing its own socket, changes nothing the match
arm reads.

## The check catalog (sections 1, 2 and 4)

`tools/redteam-harness/catalog.json` holds one entry per bullet of #685's sections 1
(escape the sandbox), 2 (permissions the owner did not give) and 4 (indirect attacks); section
3 is the verb catalog above. Each entry has an `id`, a `category` (1, 2 or 4), a `title`, a
`status` (`automated`: one shell command the harness runs in the guest; `manual`: needs a
human running a live session, not generated here) and, when automated, the `command` and the
author's `expect`ed outcome.

`tools/redteam-harness.py --list` prints it without running anything.

## Running it

```sh
tools/redteam-vm.sh start
tools/redteam-harness.py --results-log ~/.local/share/torlek-redteam/results.jsonl
tools/redteam-vm.sh stop
```

Each automated entry runs once inside the guest (`tools/redteam-vm.sh ssh`), never on the
host. `--id ID` (repeatable) runs only those entries. Every attempt appends one line to the
results log: `id`, `category`, `title`, `command`, `exit_code`, `stdout`, `stderr`, `outcome`
(`blocked`, `refused`, `succeeded` or `unclear`), `torlek_version` and `recorded_at`. The
outcome is a heuristic read of the output (a `torlek: refused:` message, a `NotAuthorized`
error, a blocked-network or permission-denied message, otherwise the exit code); a human
reads the raw `stdout`/`stderr` of every `succeeded` or `unclear` line before deciding
anything.

## What a result means

- **A success is a finding to file, not to act on.** Logging it is the whole of what this
  tool does; it never changes anything outside its own results log.
- **Each successful or unclear attempt** becomes its own `bug` + `area:security` issue (#685's
  acceptance), ranked by severity, with a regression test through the real entry point once
  fixed.
- **Reset the guest between unrelated attempts** (`tools/redteam-vm.sh reset`) so one does not
  hide behind another's side effects.
