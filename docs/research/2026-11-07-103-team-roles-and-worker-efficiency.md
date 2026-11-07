# Team roles, spawning rules, briefs and worker efficiency

Issue [#103](https://github.com/Androlax2/agent-cockpit/issues/103).
**Research and proposal, 2026-11-07; Theo has not decided.** Merging this document accepts
research delivery only. It changes no code and no design report: each decision Theo takes
below is folded into [report 07](../design/greenfield/07-teams-and-chief.md) and into
[#53](https://github.com/Androlax2/agent-cockpit/issues/53) by a separate change. Model, effort
and harness choice belong to [#497](https://github.com/Androlax2/agent-cockpit/issues/497),
whose report another worker is writing; this one links to it and sets no model per role.
Numbers are for `0b6ec2d0` and the dates they name; raw counts and commands are in
[`103/measurements.md`](103/measurements.md).

## Answer

Keep one worker per task as the default and make every other role a workflow step that a
preset asks for, not a standing team. The evidence for Designer / Developer / QA as three
sessions per task is missing; the evidence against fanning out inside one task is strong.

1. **Roles.** Ship `worker` (any step, the default) and an optional `reviewer`: the `check`
   step's independent review, run by a fresh session on a fresh clone at the handoff commit.
   Drop Designer and QA as fixed team roles. Researcher, Integrator and Release are not roles:
   no preset needs a different context or authority for them.
2. **Spawning.** The owner and a coordinator holding `start-sessions` may start a session. A
   worker never does. A reviewer is not "recruited": it is a ready workflow step that
   scheduling places, so no session spawns another.
3. **Caps.** Keep the grant's eight sessions at once and twenty starts an hour. Add three
   limits the code does not have: one active session per task step, three attempts per step
   before the task is blocked for the owner, and a start that names the task it is for.
4. **Briefs.** The task packet is the brief: goal, acceptance, out of scope, binding decisions,
   pointers, authority by reference, expected handoff, stop conditions; at most 8 KiB, no
   transcripts.
5. **Lifecycle.** One session per attempt, fresh context per issue, a 2 KiB evidence-bound
   handoff, and a stop when the step's requirements pass or a stop condition fires.
6. **Efficiency.** No token data exists for any role today, so no role has earned or lost its
   cost on evidence. Run a two-arm trial on about 20 risky issues before the reviewer step
   ships on by default.

- [ ] Theo answers the [five questions](#questions-for-theo).

## Questions for Theo

Each recommendation is the first option.

**Q1. Which roles exist?**

- **A. `worker` plus an optional `reviewer` step (recommended).** Pros: matches how the
  refactor ran (below) and how the presets are written; one new idea, "a step may demand a
  fresh session". Cons: a Designer role waits for a preset that needs it; the legacy QA
  behavior is rebuilt as one step, not a role set.
- **B. Keep Designer / Developer / QA as report 07 says.** Pros: no design change; the
  `Role` type already exists in `cockpit-scheduling`. Cons: the coordinator found the team
  unused ([#423](https://github.com/Androlax2/agent-cockpit/issues/423)); a role per title
  triples the sessions whose context is re-sent every turn.
- **C. A larger standing set** (Researcher, Reviewer, Integrator, Release). Pros: named
  responsibilities. Cons: none of the four has a preset step with different context or
  authority; merge and release are CI and procedures, not agent work.

**Q2. Who may start a session?**

- **A. Owner and a coordinator with `start-sessions`; a worker never; a reviewer is a placed
  step (recommended).** Pros: one mechanism, already built ([#410](https://github.com/Androlax2/agent-cockpit/issues/410)
  decided it stays); depth is 1 because grants are never inherited; every start has an owner
  decision or a grant behind it. Cons: a worker that wants help must ask its coordinator.
- **B. Also a worker, when the owner gave that worker `start-sessions`.** Pros: possible
  today without code. Cons: nested fan-out, the failure the sources below report; per-task caps
  would have to follow the chain.
- **C. Chief starts without confirmation.** Pros: fewer approvals. Cons: the grant already
  allows it with a named cap; making it a default removes the owner's view of spend.

**Q3. Which caps does Torlek add?**

- **A. The three missing limits of the [caps table](#spawning) (recommended).** Pros: each
  replaces a legacy limit with one the new model can state. Cons: three small changes to
  `cockpit-workflows`, `cockpit-scheduling` and the start request.
- **B. A + a per-project concurrent cap as config.** Pros: the owner can hold a project to
  fewer sessions. Cons: duplicates resource admission ([report 10](../design/greenfield/10-resources-scheduling.md)).
- **C. Grant caps only.** Pros: nothing to build. Cons: nothing stops a step looping or a start
  with no task behind it.

**Q4. What does a worker receive?**

- **A. The brief template under [Briefs](#briefs), 8 KiB at most (recommended).** Pros:
  durable across restart; the same text for every provider. Cons: the coordinator writes
  acceptance and out-of-scope lines it may skip today.
- **B. A free-text start prompt as today.** Pros: no work. Cons: the rules repeat in every
  start (2,000 bytes of suffix, measured) and acceptance is whatever the author remembers.

**Q5. Does the reviewer step ship on by default?**

- **A. Ship it off; turn it on only for risky tasks after the trial below (recommended).**
  Pros: an evidence-based default; about 20 risky issues is about 70 issues of normal work, between one day and one week at the recent pace. Cons:
  slower to a decision.
- **B. Ship it on for risky tasks now.** Pros: more independent review now. Cons: the cost is
  unmeasured, and one second review pass in 34 recorded PRs says the in-session review mostly
  suffices.
- **C. Never build it.** Pros: no new step. Cons: report 07 keeps "QA on a fresh copy"; this
  drops a decided behavior without a trial.

## What exists today

| Piece | State | Where |
| --- | --- | --- |
| Presets `research`, `code-change`, `design-led`; a step has a doer (`Agent` or `Owner`), required capabilities and named evidence | Built | `crates/cockpit-workflows/src/presets.rs` |
| A step may name a `Role` (Designer, Developer, QA); a role step goes only to a session holding it | Built, no step names one | `crates/cockpit-scheduling/src/model.rs:51`, `plan.rs:26` |
| Coordinator grants: `observe`, `message`, `start`, `end`; only the owner gives them; none implied | Built | `crates/cockpit-authorization/src/model.rs` |
| Start terms: 20 an hour, 8 sessions started and not ended, each start owner-approved by default | Built | `StartTerms::DEFAULT`, `model.rs:215` |
| A worker's question waits on its starter 15 minutes, then the owner | Built | `docs/decisions/2026-11-04-710-worker-questions-to-coordinator.md` |
| A coordinator ends a worker once the task list says done | Built | `docs/decisions/2026-11-04-713-coordinator-ends-workers.md` |
| `cockpit-teams`, `cockpit-chief` | Not started ([#53](https://github.com/Androlax2/agent-cockpit/issues/53), [#54](https://github.com/Androlax2/agent-cockpit/issues/54)) | none under `crates/` |

The decision record for the coordinator kit says defaults of "10 per hour"; the code says 20.
The code is the authority here; the record is stale on that number.

## Roles

**Test for a role.** A role earns a place only when a preset step needs (a) a different
context (a fresh clone, no view of the author's reasoning), (b) different authority, or (c)
different provider capabilities, and a trial shows it catches more than it costs. A job title
passes none of these.

**How the refactor ran.** The coordinator's own account in
[#423](https://github.com/Androlax2/agent-cockpit/issues/423): "Teams (Designer/Developer/QA)
were not used: one worker per crate area plus CI and review covered QA; design-first applies
only to UI work." The worker log agrees: of 34 recorded PRs, 33 took one review pass, and the
log's notes attribute none of the ten CI fix rounds to a logic defect (causes in the
[appendix](103/measurements.md#worker-trials-log)).

| Candidate | Needs a preset step with different context, authority or capability? | Verdict |
| --- | --- | --- |
| Worker | The default for every `Agent` step | Keep |
| Reviewer | `code-change` `check` requires `review`; a fresh clone at the handoff commit and no view of the author's reasoning is a context difference | Keep as an optional step, default off until the trial |
| QA | Same step as Reviewer, plus running acceptance on the clone | Fold into Reviewer: one step, one fresh session |
| Designer | `design-led` starts with the owner's approval of a design made earlier; no agent step makes it | Not a role yet; design workers are [#499](https://github.com/Androlax2/agent-cockpit/issues/499) |
| Researcher | `research` is three steps by one session; eight research PRs went through one worker each, one CI fix | Not a role: the `research` preset is the specialization |
| Integrator | Merge is the `main` ruleset plus auto-merge, not agent work | Not a role |
| Release | A procedure with owner gates (`docs/RELEASING.md`) | Not a role |
| Coordinator (Chief, orchestrator) | Authority, not a step: a session holding the four coordinator grants | A grant set, not a workflow role |

**What changes in the code, and who owns it.** The `Role` type stops being a closed
three-value list that `cockpit-teams` assigns. A preset step (`cockpit-workflows`) declares a
placement rule: "begins in a session that did not begin an earlier step of this task"
(`reviewer`) or none (`worker`). `cockpit-scheduling` enforces the rule when it places the
step. `cockpit-teams` owns no role field: it keeps the recruit, handoff, findings and
clarification rules over messaging. `worker` and `reviewer` are the two names of that rule's
values. Which values exist is Q1.

## Spawning

**Who.** The owner, always. A coordinator, under `start-sessions`, with the owner's approval of
each start unless the grant says otherwise. Nobody else. Messages stay collaborator content:
they can ask, never authorize ([report 07](../design/greenfield/07-teams-and-chief.md), `Keep`).

**Placed steps are not spawns.** Report 06 says scheduling consumes ready steps and workflows
never schedule. A reviewer therefore appears when `check` becomes ready and scheduling places
it on a new session under admission and the existing caps. Nothing the worker said created it.

**What a recruit may not do.** Widen grants: a started session holds none of its starter's
grants, because only the owner gives a grant and none implies another
(`Permission::COORDINATOR` doc). Spawn further: it holds no `start-sessions`. Choose its own
agent or directory: the host picks them, never the request (report 07, `Keep`).

| Cap | Value | State |
| --- | --- | --- |
| Sessions a coordinator started and not ended | 8 | Built (`max_active`) |
| Starts per coordinator per hour | 20 | Built (`per_hour`) |
| Depth of spawning | 1 | Built by grants never inherited; stated here so a test pins it |
| Active sessions per task step | 1 | Proposed; replaces "at most 1 session per role" |
| Attempts per step | 3, then the task is blocked with the next action "owner decides" | Proposed; replaces "at most 3 QA spawns per task per 10 minutes", which counted time, not outcomes |
| Reviewer sessions per handoff commit | 1 | Proposed. Report 07 keeps only "a newer handoff cancels the unsent review of the older one", which stays |
| A start names its task | Required; refused without one | Proposed; `agent start` carries a directory and launch choices, and I found no task reference in `start` (`crates/cockpit-daemon/src/coordination.rs`). The coordinator skill files the item by script today |
| Per project concurrency | Resource admission | Built elsewhere ([report 10](../design/greenfield/10-resources-scheduling.md)); not a team cap |
| Sessions per crate area | 2 | Coordinator practice ([#208](https://github.com/Androlax2/agent-cockpit/issues/208)); the product does not know areas, so a habit, not a cap |

**Evidence of need.** A start without a task has none. With one, the task's workflow run says
which step is ready and what it requires. That is the evidence; free text is not.

**Why these numbers.** The 8 and 20 are the existing defaults; observed practice held 6 to 8
sessions ([#208](https://github.com/Androlax2/agent-cockpit/issues/208)). The sources report
the same caps for the same reason: Anthropic's research system first spawned "50 subagents for
simple queries" and added effort rules to its prompts; Claude Code's subagents allow three
nested layers by default and a switch to turn nesting off; its agent teams forbid nested
teams. Torlek fixes depth at 1 because grants already do.

## Briefs

The brief is the task packet of [report 06](../design/greenfield/06-tasks-workflows-verification.md),
rebuilt from durable records on every start. Fields, in order, with the budget each may take
of the 8 KiB packet ceiling [#619](2026-11-04-619-agent-token-efficiency.md#proposed-response-and-wake-rules)
proposes:

| Field | Rule | Budget |
| --- | --- | ---: |
| Goal | One paragraph; behavior, not file paths | 600 B |
| Acceptance | Testable lines; each is a check `cockpit-verification` can evaluate | 1.5 KiB |
| Out of scope | Explicit; "nothing else" is not a line | 500 B |
| Binding decisions | One line each with the record's link; owner rulings included | 1 KiB |
| Pointers | Issue, reports and files to read on demand, by name; never pasted | 500 B |
| Authority | The grants and step requirements, by reference; no prose permission | 300 B |
| Expected handoff | The shape under [Lifecycle](#lifecycle) | 300 B |
| Stop conditions | When to ask, when to stop | 300 B |

The rules come from three places: the `AGENT-BRIEF` advice that
[#687](2026-11-04-687-mattpocock-skills.md) found ("durability over precision", behavior not
file paths, testable acceptance, explicit out-of-scope), Anthropic's finding that a vague
delegation ("research the semiconductor shortage") made subagents duplicate work and that a
brief needs an objective, an output format, guidance on tools and sources, and clear task
boundaries, and this repository's own start prompt.

**Must not receive.** Another session's transcript. The previous attempt's reasoning: only its
handoff and findings. The owner's chat. The whole issue thread: its decisions, not its
comments. A full CI log: a pointer and a bounded tail. Secrets.

**Why not share traces.** Cognition argues the opposite: "share full agent traces, not just
individual messages". Its concern is conflicting implicit decisions between parallel agents
on one task. Torlek does not split a task across parallel workers, and puts the decisions
that traces would carry into the brief as explicit records. Sharing traces also costs every
turn: a long context is re-sent with every request
([Claude Code cost guide](https://code.claude.com/docs/en/costs)).

**Role instructions.** Today every start pastes the same 2,000-byte suffix (git rules, test
commands, the CI loop) after one line of task text. These are project rules, not task
text. Proposal for the project: keep them in project instructions and the step's instruction
topic, and let the start carry the task item and one line of intent. This is a project choice,
not a product requirement.

**Measured context per role, bytes, `0b6ec2d0`.**

| Role | Always loaded | On demand | Tokens |
| --- | --- | --- | --- |
| Worker | 1,640 (brief 1,364 + skill entry 276) | manual 12,569; this repository's `AGENTS.md` + `CLAUDE.md` 6,838; start suffix 2,000 | Not measured |
| Coordinator | the same 1,640 | the coordinator skill 12,386 | Not measured |
| Reviewer | does not exist | packet at most 8,192 + a diff pointer | Not measured |

Bytes come from `torlek brief --measure` and `wc -c`. Tokens come from the adapters' per-turn
totals that [#479](https://github.com/Androlax2/agent-cockpit/issues/479) added; a session has
no way to read another session's totals, so the first token numbers per role come from the
trial.

## Lifecycle

- **One session per attempt; fresh context per issue.** Kept from
  [#17](https://github.com/Androlax2/agent-cockpit/issues/17). A restart rebuilds the packet
  from durable records; a summary supplements it (report 06).
- **Handoff, about 2 KiB plus pointers.** Six fields: what changed (all task commits, not the
  last); the exact result checked (full commit id, checkout); what passed or failed, by
  requirement name; open items; what needs the human; pointers to evidence. "Done" alone or a
  review claimed from prose is incomplete (report 06).
- **Reviewer gets a fresh clone** at exactly the handoff commit, the packet and the diff
  pointer, never the author's transcript. Two axes, from
  [#687](2026-11-04-687-mattpocock-skills.md): standards (the project's rules) and spec (the
  acceptance lines). Its findings return as a structured message to the worker's task; a newer
  handoff cancels an unsent review.
- **A worker stops when** its step's named requirements pass and the handoff is recorded
  (for `deliver`, when the result is where the task asks for it); or a stop condition fires:
  a question only the owner can answer (it waits on the coordinator 15 minutes, then the owner,
  [#710](../decisions/2026-11-04-710-worker-questions-to-coordinator.md)); a third failed
  attempt of one step; an idle turn end with no question and no armed wait. Three of 34
  recorded workers stalled mid-task and needed a nudge; the wake rule in
  [#619](2026-11-04-619-agent-token-efficiency.md) (blocking waits, no polling) is the
  mitigation already in the start prompt.
- **A finished worker is ended**, by `end-sessions` once its task item is done
  ([#713](../decisions/2026-11-04-713-coordinator-ends-workers.md)), so no idle session
  keeps a slot or a context.

## Efficiency

**What the data shows.** From the worker trials log (34 PRs) and the last 60 merged worker
PRs (`#981` to `#1117`, 2026-11-06 16:05Z to 2026-11-07 15:13Z):

The two samples are different populations and are not pooled: the log spans 2026-11-04 to
2026-11-07 and PRs `#745` to `#887`; the 60 PRs are the newest day. Open-to-merge times from the
newest day understate the log's own (rows of 5 to 6 hours include reboots and rebases).

- Newest 60 PRs: median open-to-merge 0.2 h, 90th percentile 0.54 h, longest 2.4 h.
- Newest 60 PRs: commits per PR median 2; 12 had one; 21 had three or more.
- CI fix rounds: 10 of 30 logged PRs had at least one (risky 5 of 9, contained 4 of 12,
  research 1 of 8, docs 0 of 1). Attributed causes: five flaky tests, main drift or disk, three
  lint or clippy, one PR opened before its review, one unattributed.
- Review passes: 33 of 34 took one; one took two.
- Stalls: 3 of 34.

**What it does not show.** Tokens, for any session. Findings per review. Defects after merge.
Cost of a second session. The log has no token column, and no PR comment holds a review (the
60 PRs carry 0 review comments and 5 conversation comments; the review runs in the worker's
session). So "which roles earn their cost" cannot be answered from existing records, and a
number for tokens per verified result would be invented. The commit counts are a rework
proxy only: they mix review fixes, CI fixes and plain follow-ups.

**Metrics** (report 06 and 07 already name them; this fixes their meaning per role):

| Metric | Definition | Where it is recorded |
| --- | --- | --- |
| Tokens per verified result | All tokens of every session that worked the task, divided by tasks whose requirements passed on the exact result | Adapter totals (#479) joined to the task |
| Useful events per wake | Wakes that produced a checkpoint advancing a requirement, an accepted handoff, a resolved question or a delivered decision, divided by wakes; availability-only reports count for nothing | Workflow and messaging records |
| Review yield | Reviewer findings the worker accepted, per review | Structured findings |
| Post-merge fixes | A later PR repairing the same issue's behavior within 14 days | Issue and PR links |
| Stalled-task age | Time from last event to now for a task with no named next action | Workflow state |

**Trial for Q5.** Two arms on risky issues, assigned alternately, like the existing model
rotation:

1. **Baseline (today).** One worker, one in-session `/code-review`.
2. **Reviewer.** The same, plus a reviewer session on a fresh clone at the handoff commit.

About 29% of recorded PRs are risky (10 of 34), so 20 risky issues, ten per arm, take about 70
issues of normal work. The two pace figures disagree: the 34 logged PRs took three days
(since 2026-11-04), about 6 days for 70; the last 60 merged worker PRs took 23 hours, about 1
day for 70. Expect one day to one week. Ten per arm detects only a large effect; a small one stays unknown, which is an acceptable result for a step that ships
off. **The owner sets the threshold before the first run.** A suggestion to set against:
the reviewer earns its cost if post-merge fixes fall by a third or more, or it yields an
accepted high-severity finding in one review of five, at no more than 50% more tokens per
verified result. The trial costs no extra issues. **Prerequisite:** each session's token totals
(#479) must be recorded against its task id. No issue I found owns that join:
[#58](https://github.com/Androlax2/agent-cockpit/issues/58) (`cockpit-usage`) is the closest, so
the changes below add it there. Without it the trial can count review yield and post-merge fixes
but not tokens per verified result, and the 50% threshold cannot be evaluated.

**Do not transfer the published multiples.** Anthropic reports multi-agent research at about
15 times a chat's tokens; Claude Code's cost guide says agent teams use about 7 times a
standard session's when teammates run in plan mode. Both describe peers running in parallel.
A serial reviewer is a different shape; its cost is the trial's output.

## Other orchestrators

Primary sources, read on 2026-11-07. The Claude Code pages (agent teams, costs) came back as
page text and the sub-agents nesting and concurrency lines were re-checked against the raw page;
the other pages came through a fetch tool that summarizes, and their quotes are as returned. Four are the vendors' own documentation of their own products, two are papers, one
is a company's design essay. None measured Torlek's shape (one issue, one session, one PR).

| Source | What it does | Transfers | Does not transfer |
| --- | --- | --- | --- |
| [Anthropic, multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system) (2025-06-13) | Lead plus parallel subagents; about 15 times chat tokens; briefs need objective, output format, tools, boundaries; effort rules in prompts; fresh subagents with handoffs and stored references | Brief fields; caps by task size; reference passing | Breadth research that splits cleanly; the lead blocks on subagents |
| [Claude Code subagents](https://code.claude.com/docs/en/sub-agents) | Fresh context, the parent writes the delegation message, only a summary returns; nesting to three layers, switchable to one; 20 concurrent | Fresh context; summary return; an explicit depth cap | Runs under the parent's authority; Torlek sessions hold their own grants |
| [Claude Code agent teams](https://code.claude.com/docs/en/agent-teams) (experimental) | Lead, teammates, shared task list, mailbox; no nested teams; one team per session; 3 to 5 teammates advised; teammates start in the lead's permission mode; "For sequential tasks, same-file edits, or work with many dependencies, a single session or subagents are more effective" | No nesting; messages never approve anything; one owner per file set; start small | Teams inside one session and one permission mode |
| [OpenAI Codex subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents) | Spawned when asked, or when `AGENTS.md` or a skill asks; inherit the sandbox policy and approvals, which a custom agent file cannot replace; "more tokens than comparable single-agent runs" | A recruit never widens authority | Concurrency and depth defaults: I could not confirm them on the official page |
| [Cognition, "Don't Build Multi-Agents"](https://cognition.com/blog/dont-build-multi-agents) (2025-06-12) | "Share context, and share full agent traces"; "conflicting decisions carry bad results"; prefers a single-threaded linear agent | Do not split one task across parallel workers; make decisions explicit | Sharing full traces; Torlek rebuilds from durable records |
| [Cemri et al., "Why Do Multi-Agent LLM Systems Fail?"](https://arxiv.org/abs/2503.13657) (2025, v3 2025-10) | 1,600+ traces, 7 frameworks, 14 failure modes in three groups: system design, inter-agent misalignment, task verification; gains "often minimal" | Spend on verification and handoff checks before adding roles | Taxonomy is of traces from other frameworks |
| [Kim et al., "Towards a Science of Scaling Agent Systems"](https://arxiv.org/abs/2512.08296) (2025-12, v3 2026-04) | 260 configurations, 5 architectures; +80.8% on decomposable work to -70.0% on sequential planning; centralized verification propagates fewer errors | Parallel across decomposable tasks, serial inside one; central verification | Benchmarks, not repositories under review |
| [MetaGPT](https://arxiv.org/abs/2308.00352) (2023, v7 2024-11) | Roles on an assembly line with standardized operating procedures in prompts | A preset is the procedure; roles are steps of it | Role list not confirmed from the page I read; generation benchmarks |

**Disagreement.** Anthropic's research system and Claude Code's agent teams fan out; Cognition
and the scaling paper argue serial work should stay in one thread. The sources agree on the
condition: independent subtasks. Torlek's unit of parallelism is the task, which is independent
by construction (one issue, one clone, one PR), so it fans out across tasks and stays serial
inside one.

## What this report does not know

- Tokens per role, and so tokens per verified result. The trial produces them.
- Whether a fresh reviewer finds what the author's own review misses. The trial measures it.
- The summarized sources' claims beyond the text the fetch tool returned; the Codex default caps in
  third-party posts differ from the official page and are left out.
- The legacy roles' behavior under load: legacy sessions were not run or read.

## Folding into report 07 and #53

After Theo's answers, report 07 changes as follows (text assumes the recommended options):

- `Keep`, first bullet: "Roles Designer / Developer / QA; any provider can hold any role" becomes
  "Roles are workflow steps. `worker` takes any step; `reviewer` takes an independent `check`
  on a fresh clone at the handoff commit. Any provider can hold either."
- `Keep/drop`, first bullet: "Roles, at most 1 session per role" becomes "at most 1 active
  session per task step"; "at most 3 QA spawns per task per 10 min" becomes "at most 3 attempts
  per step, then the task is blocked for the owner".
- `Rules`: add "A start names its task; a worker never starts a session; a recruit holds no
  coordinator grant."
- `Measures`: add the definitions of the table under [Efficiency](#efficiency).

## Changes for each linked issue

Text assumes the recommended answer to every question; the coordinator edits it to match
Theo's answers before posting.

### #53 cockpit-teams on workflows and messaging

> **Owns:** The team rules over workflow steps and messaging: recruit, handoff, findings, clarifications. The `worker` and `reviewer` placement rule is declared by the preset step in `cockpit-workflows` and enforced by `cockpit-scheduling`; this crate has no role field and no tables of its own.
>
> **Scope (decided in #103, 2026-11-07)**
>
> - Roles are workflow steps: `worker` takes any step; `reviewer` takes an independent `check` on a fresh clone at the handoff commit. No Designer or QA role; the `Role` type in `cockpit-scheduling` is replaced by the placement rule "a session that began no earlier step of this task".
> - Keep from report 07: role rules for messages, exact-hash approval, recruit with host-chosen agent and directory, structured handoff and findings, clarifications of at most 3 rounds, uncertain delivery never resent.
> - Caps: 1 active session per task step; 3 attempts per step, then the task is blocked for the owner; 1 reviewer session per handoff commit (new in #103); a newer handoff cancels the unsent review (report 07).
> - Session links become task messages with scheduling limits.
>
> **Acceptance**
>
> - Parity scenario: a task with `check` placed on a second session at the handoff commit; a fourth attempt is refused with the next action "owner decides"; a newer handoff cancels the pending review.

### #54 cockpit-chief

> Add to Scope: the Chief starts sessions only through `start-sessions` (#410), every start names its task, a recruit holds no coordinator grant, and the Chief never starts a worker for a step that scheduling places. Add to Acceptance: a start without a task is refused with a typed reason.

### #208 Orchestrator role

> Add to Scope: the brief template of #103 (goal, acceptance, out of scope, binding decisions, pointers, authority by reference, expected handoff, stop conditions; at most 8 KiB, no transcripts) replaces the free-text start prompt; the caps table of #103 (8 at once, 20 an hour, 2 per crate area as a habit) is the orchestrator's launch limit. Metrics per milestone gain tokens per verified result once #479 totals are joined to tasks.

### #58 cockpit-usage: quota windows and alerts

> Add to Scope: record each session's token totals (the harness totals of #479) against the task it works, so tokens per verified result can be computed across every session of one task (#103 trial prerequisite).

### #160 Teams/Chief legacy lessons

> Record from #103: "at most 3 QA spawns per task per 10 min" is dropped for "3 attempts per step"; "at most 1 session per role" is kept as "1 active session per task step"; the team unused in the refactor is dropped as a standing set of roles.

### Not changed

- #497 (model selection): this report sets no model or effort per role; its report decides them.
- #619 (token efficiency): its budgets and wake rules are used as written.
- #17, #410, #423, #56: unchanged. #423's brainstorm input is cited above.
- `docs/decisions/2026-11-01-410-coordinator-kit.md` says "at most 10 per hour"; the code's
  default is 20 (`StartTerms::DEFAULT`). This report does not edit it; correct it as its own
  docs change.

## Sources

- Issue #103 and its comment; reports [06](../design/greenfield/06-tasks-workflows-verification.md),
  [07](../design/greenfield/07-teams-and-chief.md) and
  [08](../design/greenfield/08-agent-onboarding-context.md); issues 17, 53, 54, 58, 208, 410,
  423, 497 and 160.
- Research: [#619](2026-11-04-619-agent-token-efficiency.md),
  [#687](2026-11-04-687-mattpocock-skills.md).
- Records: `docs/decisions/2026-11-01-410-coordinator-kit.md`,
  `2026-11-04-710-worker-questions-to-coordinator.md`, `2026-11-04-713-coordinator-ends-workers.md`,
  `2026-11-04-739-max-active-workers.md`.
- Code: `crates/cockpit-workflows/src/presets.rs`, `crates/cockpit-scheduling/src/model.rs`,
  `crates/cockpit-authorization/src/model.rs`, `crates/cockpit-daemon/src/coordination.rs`,
  `crates/cockpit-daemon/src/coordinator_workers.rs`.
- Worker practice: `.agents/skills/coordinator/SKILL.md`, `trials-log.md` and `prompts/`.
- The external sources in the table above, fetched 2026-11-07.
