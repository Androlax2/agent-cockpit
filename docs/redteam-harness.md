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
| 249 | `AgentReports` | `agent reports` |
| 273 | `BugReportNew`, `BugReportBundlePlan`, `BugReportList`, `BugReportShow`, `BugReportEdit`, `BugReportOpened`, `BugReportSent`, `BugReportDiscard` | `bug-report new\|list\|show\|edit\|send\|discard` |
| 285 | `VersionReport` | `version --all` |
| 294 | `UsageQuota` | `usage quota` |
| 314 | `SkillPack` | the owner's own skill listing |
| 330 | `StorageReport`, `StoragePrunePreview`, `StoragePruneApply`, `StoragePruneAbandon`, `StoragePruneStatus`, `StorageOutputs`, `StorageDatabase` | `storage`, `storage outputs [prune]`, `storage database [prune]`, `storage prune` |
| 347 | `ProjectAdd`, `ProjectList`, `ProjectRemove`, `CheckoutList`, `CheckoutClone`, `CheckoutReview`, `CheckoutClaim`, `CheckoutRelease`, `CheckoutSweep`, `CheckoutPush` | `project add\|list\|remove`, `checkout list\|clone\|review\|claim\|release\|sweep\|push` |
| 389 | `ApprovalQueue`, `ApprovalShow`, `ApprovalApprove`, `ApprovalDecline`, `ApprovalEdit`, `ApprovalInput`, `GuideShow`, `GuideAnswer`, `GuideStuck`, `GuideClose` | `approval list\|show\|approve\|decline\|edit\|input`, `guide show\|answer\|stuck\|close` |
| 406 | `NetworkList`, `NetworkAllow`, `NetworkDeny`, `NetworkGrants`, `NetworkRevoke` | `network list\|allow\|deny\|grants\|revoke` |
| 415 | `AccountSignIn`, `AccountSignedIn`, `AccountSignInDiscard`, `AccountGrant`, `AccountGrants`, `AccountRevoke` | `account sign-in\|grant\|grants\|revoke` |
| 441 | `SessionGrant`, `SessionRevoke`, `SessionGrants` | `session grant\|revoke\|grants` |
| 465 | `SessionNew`, `SessionList`, `SessionRestart`, `SessionRestartAll`, `SessionSleep`, `SessionWake`, `SessionResume`, `SessionSend`, `SessionCancel`, `SessionEnd`, `SessionEndFinished`, `SessionLink`, `SessionSandbox`, `SessionSandboxRecord`, `SandboxShow` | `session new\|list\|restart\|sleep\|wake\|resume\|send\|cancel\|end\|link\|sandbox\|sandbox-record`, `sandbox show` |
| 518 | `TaskPriority`, `TaskQueue`, `TaskApprove`, `TaskDecline`, `TaskReopen`, `TaskWait`, `TaskUnwait`, `TaskImageAdd`, `TaskImage`, `TaskDraftSave`, `TaskDrafts`, `TaskDraftDiscard` | `todo priority\|queue\|approve\|decline\|reopen\|after\|unwait\|image add\|image get\|draft save\|drafts\|draft discard` |
| 556 | `WorkflowStart`, `WorkflowCancel` | `workflow start\|cancel` |
| 571 | `ResourceStatus`, `ResourceProfile`, `ResourceQueue`, `ResourceCheck`, `ResourceCheckpoint`, `ResourceStop` | `resources status\|profile\|pause-queue\|resume-queue\|check\|checkpoint\|stop` |
| 582 | `QueueExplain`, `QueueDispatch`, `TaskHandoff` | `queue explain\|dispatch`, `todo handoff` |
| 597 | `ManagerOpen`, `ManagerFocus`, `ManagerFit`, `SessionShow`, `SessionType`, `SessionPasteImage` | the manager/TUI protocol behind `tui` |
| 612 | `ProviderList` | `provider list` |
| 622 | `Readiness` | the readiness report behind `init`/`doctor` |
| 628 | `DesktopSessionImport` | `desktop-session import` |
| 640 | `CheckRun`, `CheckReceipt`, `CheckPending`, `CheckResolvePending` | `check run`, `check pending`, `check resolve-pending` |
| 662 | `LogsLevelStatus`, `LogsLevelSet`, `LogsLevelRevert` | `logs level` |
| 676 | `ConfigReload` | `config reload` |

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
`status` (`automated`: the harness runs it in the guest; `manual`: needs a human running a
live session, not generated here) and, when automated, the `command`, the author's `expect`ed
outcome, and a `caller` naming who runs it (see the next section).

`tools/redteam-harness.py --list` prints it without running anything.

## How each check runs, and as whom

Issue #1158: every automated check runs as the caller its entry names, never as the guest's
owner shell. Who that is decides what the result proves, so the caller is part of the record.

- **`caller: "sandboxed-session"` (section 1, escape the sandbox).** The attack runs inside a
  real sandboxed agent session in the guest, under `agents.sandbox = "required"`. The harness
  starts a session (`torlek session new`) in a checkout it owns from outside the sandbox, tells
  the agent to run the command verbatim and write the command's combined output followed by a
  distinctive `__REDTEAM_EXIT__=<code>` trailer to a drop file in that checkout, then reads the
  drop file back and classifies it. A model drives the session, so these need a provider credential
  (below). The result records the
  session's sandbox evidence (`torlek session sandbox`): the backend that enforced it and the
  subject, so a reader can tell a real sandbox block from a Unix permission error.
- **`caller: "agent-session"` (section 2, permissions the owner did not give).** The `torlek`
  command runs on the guest daemon's **agent socket**, with `TORLEK_SESSION` set, so the daemon
  routes it as `Peer::Agent` and refuses it with the agent socket's `owner_only()` — the same
  path `crates/cockpit-daemon/tests/host_only_verbs.rs` proves. This is the fix for the earlier
  bug where the harness ran these on the owner shell (empty `TORLEK_SESSION`), which reaches the
  **owner** socket, so the daemon treated the attack as the owner and the "refusal" was really
  the owner being allowed. These need no provider credential.

## The provider credential for section 1 (option B)

Section 1 runs a real model-driven session, which needs a provider credential. The guest has
the `claude` program (pinned, [the red-team VM](redteam-vm.md)) but no credential. The owner
creates a **separate** credential for the guest only and drops it into the guest at run time.
Two routes, one file each:

```sh
# A: a spend-capped API key (the default variable is ANTHROPIC_API_KEY)
tools/redteam-vm.sh put-credential /path/to/keyfile

# B: the owner's subscription token, made with `claude setup-token`
tools/redteam-vm.sh put-credential /path/to/tokenfile --var CLAUDE_CODE_OAUTH_TOKEN
```

The file holds the one-line value and nothing else. The trade-off: the API key has a spend cap
you set; the subscription token spends the owner's plan and has no cap of its own, only the
plan's usage limits, and lasts a year unless revoked. Either way, revoke it when the red-team
is done. `put-credential` clears the provider's credential folder first, so the guest holds
exactly one.

The credential is copied into the guest only; it never enters this repo, the results log, the
host's own Torlek state, or any result line. The harness **refuses to run section 1, naming
why**, when the guest's sandbox is not `required` or has no backend, the daemon does not list the
agent (`torlek doctor` in the guest: no `claude`, so a guest built before #1171 must be rebuilt
with `create`), or no credential is present, and each case names itself. `tools/redteam-vm.sh
check` reports the agent and the credential on separate lines. Drop the credential again when
done:

```sh
tools/redteam-vm.sh drop-credential
```

Section 2 needs none of this, so it runs even when section 1 is refused.

## Running it

```sh
tools/redteam-vm.sh start
# section 2 only (no key needed):
tools/redteam-harness.py --id 2.1-self-grant --results-log ~/.local/share/torlek-redteam/results.jsonl
# everything automated (section 1 needs the credential dropped first):
tools/redteam-vm.sh put-credential /path/to/keyfile
tools/redteam-harness.py --results-log ~/.local/share/torlek-redteam/results.jsonl
tools/redteam-vm.sh drop-credential
tools/redteam-vm.sh stop
```

Each automated entry runs inside the guest (`tools/redteam-vm.sh ssh`), never on the host.
`--id ID` (repeatable) runs only those entries. Every attempt appends one line to the results
log: `id`, `category`, `title`, `command`, `caller`, `caller_detail`, `exit_code`, `stdout`,
`stderr`, `outcome` (`blocked`, `refused`, `succeeded`, `unclear` or `not-run`), `sandbox` (the
session's sandbox evidence for section 1, `null` for section 2), `session` and
`not_run_reason` (section 1 only),
`torlek_version` and `recorded_at`. The outcome is a heuristic read of the output (a
`torlek: not-authorized:`/`refused:` message, a blocked-network 403 or permission-denied
message, otherwise the exit code); a human reads the raw `stdout`/`stderr` of every
`succeeded` or `unclear` line before deciding anything.

## What a result means

- **For a section-1 escape check, the only breach verdict is `succeeded`** (the attack ran and
  came back with what it should not have). Every other verdict means the attack was stopped:
  `blocked` (the sandbox denied it — a 403 on egress, a permission error), `refused` (Torlek
  declined it with a reason), or `unclear`. An attack on a path the sandbox simply does not
  mount (the owner's home, another project) fails with "No such file or directory", which is
  `unclear`, not `blocked` — both are non-breaches, and the recorded `sandbox` evidence (the
  backend and subject that enforced the session) is what tells a real sandbox stop from a
  benign miss. A human reads the raw output and the evidence of every `succeeded` or `unclear`
  section-1 line before deciding anything; the harness does no host-side confirmation of its
  own.
- **`not-run` tested nothing** (#1175): the section-1 attack never ran in the sandbox, because
  the session did not start, wrote no result, or wrote one without its exit line.
  `not_run_reason` says which. It is never a pass: the summary counts it apart, and a run with
  any `not-run` line exits 1.
- **A success is a finding to file, not to act on.** Logging it is the whole of what this
  tool does; it never changes anything outside its own results log.
- **Each successful or unclear attempt** becomes its own `bug` + `area:security` issue (#685's
  acceptance), ranked by severity, with a regression test through the real entry point once
  fixed.
- **Reset the guest between unrelated attempts** (`tools/redteam-vm.sh reset`) so one does not
  hide behind another's side effects.
