# Repository-local skills and agent workflow conventions

Issue [#620](https://github.com/Androlax2/agent-cockpit/issues/620).
**Research and proposal, 2026-11-04; Theo has not decided.** Merging this document accepts
research delivery only. It changes no instruction, skill, policy or dependency; each plan item
below waits for Theo's approval, item by item. The follow-up issues it lists are not filed.

## Answer

Add no new skill. The workflow is mostly written down already. Three things stop agents from
finding and following it:

1. **Each client loads half of it.** Claude Code reads `CLAUDE.md` and not `AGENTS.md`; Codex reads
   `AGENTS.md` and not `CLAUDE.md`. Today each file only links to the other.
2. **The documents disagree.** Four contradictions; one is about the `make` commands Theo banned
   today in #720 (see [Contradictions](#contradictions)).
3. **Four rules are written nowhere in the repository.** "Done means merged", waiting on CI, the
   research document form, and how to report on the issue. Three of them reach workers only through
   launch prompts.

Size is not the problem. The always-loaded files are 2.1 KB and 4.3 KB, smaller than
[Biome's](https://github.com/biomejs/biome/blob/fca5c35007fc3ff9c55edb2085fc557a7fbfd16d/AGENTS.md)
(5.2 KB) and far below Next.js's (31.5 KB, 96% of Codex's cap).

Recommendation, in order: fix the contradictions; make loading deterministic; turn `repo-workflow`
into the autonomous issue-to-merge path; add short issue-update and discovered-work rules; add a
`docs/research/README.md`; guard the result with one regression test. Details, examples and
templates follow. The mattpocock skill collection is
[#687](https://github.com/Androlax2/agent-cockpit/issues/687)'s question and is not repeated here.

- [ ] Theo picks the loading option (D1), the rebase rule (D2) and the research README (D3) under
  [Decisions](#decisions-for-theo), and approves or declines each [plan](#rollout-plan) item.

## Method and labels

- **Source examined.** Cockpit at [`0723de52`](https://github.com/Androlax2/agent-cockpit/tree/0723de5248e04f199af9801df83b39f32902b6b8);
  GitHub issues #61 to #739 and the 200 newest merged pull requests (read-only `gh`).
- **Primary sources only** for conventions: client documentation and the repositories' own files,
  pinned to a commit or read on 2026-11-04. Where a source gives no number, none is quoted.
- **Labels.** *Measured*: counted from files or the GitHub API. *Doc*: stated by client
  documentation, not run here. *Observed (n=1)*: one session, this one. *Est.*: tokens as
  bytes ÷ 4, about ±25%, the rate [#619](2026-11-04-619-agent-token-efficiency.md#method-and-labels)
  uses; bytes and tokens stay apart.
- **Not done.** No provider call, no live conversation read, no worker launched, so no measured
  token or adherence numbers. Codex behaviour is from its documentation. The validation that needs
  live runs is a [plan](#how-to-validate-it-without-theo-repeating-it), not a result.
- **Overlap.** Torlek's own channel (brief, manual, status verbs) is
  [#619](2026-11-04-619-agent-token-efficiency.md)'s and
  [#419](https://github.com/Androlax2/agent-cockpit/issues/419)'s. Roles and spawning are
  [#103](https://github.com/Androlax2/agent-cockpit/issues/103)'s, orchestrator behaviour
  [#208](https://github.com/Androlax2/agent-cockpit/issues/208)'s, documentation style
  [#618](https://github.com/Androlax2/agent-cockpit/issues/618)'s. This document uses their results
  and proposes nothing in their lanes.

The commands, scripts and raw tables are in the appendix [620/measurements.md](620/measurements.md).

## What other projects and the clients say

| Practice | Source | Used here |
| --- | --- | --- |
| Always-loaded file holds facts for every session; procedures go to a skill. | [Claude Code memory](https://code.claude.com/docs/en/memory): an entry that is "a multi-step procedure or only matters for one part of the codebase" moves "to a skill or a path-scoped rule" | Yes |
| Target under 200 lines; imports organise but do not cut cost. | Same page | Fits: `CLAUDE.md` 36 lines, `AGENTS.md` 82 |
| Contradicting instructions: the model "may pick one arbitrarily". | Same page | Drives the first plan item |
| With both files present, Claude Code loads `CLAUDE.md` only; `@AGENTS.md` inside it shares one file. | [Same page, AGENTS.md section](https://code.claude.com/docs/en/memory#agents-md) | Drives D1 |
| Codex builds its chain from `AGENTS.md` only, 32 KiB cap, other names need a config entry. | [Codex AGENTS.md guide](https://learn.chatgpt.com/docs/agent-configuration/agents-md) | Drives D1 |
| Skill: descriptions always in context, body on demand, body under 500 lines, key use first. | [Claude Code skills](https://code.claude.com/docs/en/skills), [Agent Skills spec](https://agentskills.io/specification) (body under 5,000 tokens) | Yes |
| Project skills are read from `.claude/skills/` (Claude Code) and `.agents/skills` (Codex). | Claude Code skills page; [Codex skills](https://learn.chatgpt.com/docs/build-skills) | Drives P5 |
| Rules that cannot have exceptions belong in a hook or CI, not prose. | [Claude Code best practices](https://code.claude.com/docs/en/best-practices): "`CLAUDE.md` is for guidelines, and Claude is allowed to decide a guideline doesn't apply" | Drives P6 |
| Context files do not generally raise task success and add over 20% cost; project-specific rules help, overviews do not. | [Gloaguen et al., arXiv 2602.11988](https://arxiv.org/abs/2602.11988) (ICLR 2026) | Keep files to non-obvious, project-specific rules |
| Universal rules once in `AGENTS.md`; skills "do not repeat them"; one catalog with trigger boundaries. | [Biome `AGENTS.md`](https://github.com/biomejs/biome/blob/fca5c35007fc3ff9c55edb2085fc557a7fbfd16d/AGENTS.md), [skills README](https://github.com/biomejs/biome/blob/fca5c35007fc3ff9c55edb2085fc557a7fbfd16d/.agents/skills/README.md) | Yes: no restating across layers |
| Create a skill for detailed, task-specific, self-contained content; keep one-liner rules and content every session needs in `AGENTS.md`. | [Next.js authoring-skills](https://github.com/vercel/next.js/blob/9d7422f68b9e7913e3a23e27ee02e7fbdb4165b1/.agents/skills/README.md) | Yes: the bar for a new skill |
| Short rule files that link guides, plus the narrowest-test-first loop. | [uv](https://github.com/astral-sh/uv/blob/33f003cb85d73fe2aad0079f532ff019540d72e9/AGENTS.md) (3.7 KB), [rust-analyzer](https://github.com/rust-lang/rust-analyzer/blob/d385d3dc167c25239d3d64a1b3d55623fc14c99c/AGENTS.md) (3.5 KB) | Matches `AGENTS.md` today |
| Who may post issues and comments is stated in the file: Next.js lets only org members' agents comment; rust-analyzer: "Do not autonomously open issues or pull requests." | [Next.js](https://github.com/vercel/next.js/blob/9d7422f68b9e7913e3a23e27ee02e7fbdb4165b1/AGENTS.md), rust-analyzer file above | Cockpit's analogue: Torlek grants decide; prose only states the habit |
| Context-saving habits written down: grep before reading large files, save command output once, do not re-run unchanged commands. | Next.js `AGENTS.md`, "Context-Efficient Workflows" | Already in `AGENTS.md` loop step 3; no addition |
| Final independent review by a fresh subagent given only scope and requirements. | Biome `AGENTS.md`, "Final Review" | Already required here (`/code-review`) |

**Demonstrated versus assumed.** Demonstrated by the sources: layering (always-loaded versus
on-demand), deterministic enforcement for hard rules, and removing contradictions. Not
demonstrated by any source: that a particular issue-comment format reduces status spam, or that
a given file size raises adherence. The only quantified result (the arXiv paper) is on SWE-bench
style tasks, not on delivery workflow. The templates below are therefore proposals to
[validate](#how-to-validate-it-without-theo-repeating-it).

## Inventory

### What is loaded, by client

| Layer | Bytes | Words | Est. tokens | Claude Code | Codex |
| --- | ---: | ---: | ---: | --- | --- |
| `CLAUDE.md` (Git authorization, Tests) | 2,159 | 322 | 540 | **Automatic** | Not loaded |
| `AGENTS.md` (loop, rules index, skill index) | 4,372 | 587 | 1,090 | Only if the agent follows the link | **Automatic** |
| One scoped rule, `.agents/rules/*.md` | 992 to 1,313 | 141 to 186 | 250 to 330 | Only if the agent follows the link | Same |
| `repo-workflow/SKILL.md` | 6,255 | 865 | 1,560 | Not listed (see below) | Listed; body on demand |
| `CONTRIBUTING.md` | 6,517 | 949 | 1,630 | Link | Link |
| `docs/DEVELOPMENT.md` | 10,001 | 1,319 | 2,500 | Link | Link |
| 30 crate READMEs, each opens with a one-sentence ownership line | 182,366 total | | | On demand | On demand |

*Measured* for bytes and words; *doc* for who loads what. A Claude Code worker starts with
**2,159 bytes**. Reaching a rule, the skill and CONTRIBUTING takes five reads and about **20.6 KB**
(≈5,200 tokens, est.); adding DEVELOPMENT makes it 30.6 KB. A Codex worker starts with 4,372 bytes
(13% of its cap) and never sees the Git authorization or the cargo-only Tests section unless it
follows a link.

**Skill discovery.** The Claude Code documentation lists `.claude/skills/` for project skills.
In the first 100,000 of its 118,821 characters (the part read) `.agents/skills` does not appear.
This session's skill listing contains `torlek` and no `repo-workflow` (*observed, n=1*). `AGENTS.md` already says to read the skill by link when the
client does not discover it, so the skill reaches a Claude worker only through
`CLAUDE.md` → `AGENTS.md` → `SKILL.md`.

### What agents repeat today

Instruction files changed in nine commits since 2026-10-31 (*measured*, `git log`). Four add or
change a workflow rule: one medium review per PR and no `/simplify`
([#355](https://github.com/Androlax2/agent-cockpit/issues/355)), worker pushes and auto-merge
([#132](https://github.com/Androlax2/agent-cockpit/issues/132)), Codex skill invocation
([#517](https://github.com/Androlax2/agent-cockpit/issues/517)), cargo directly
([#720](https://github.com/Androlax2/agent-cockpit/pull/720)). That rules arrive as corrections,
paragraph by paragraph, is an inference; the log does not record why.

The launch prompt for this issue is a second sample (*observed, n=1*, coded by hand). Of its 13
instructions: 3 repeat `CLAUDE.md`, 2 repeat the Torlek manual, 4 are task-specific (cite #687,
research only, the document location, list follow-ups instead of filing them), 1 is a one-off bug
workaround, and **3 are written nowhere in the repository**: the work is done only when merged (`CLAUDE.md`
stops at arming auto-merge); wait for CI in a blocking loop without ending the turn; match the
research documents' form.

### Contradictions

| # | Statement A | Statement B | Effect |
| --- | --- | --- | --- |
| C1 | [`CONTRIBUTING.md:50`](../../CONTRIBUTING.md): "Never force-push"; once a PR is open, merge `main` into the branch | [`CLAUDE.md:12`](../../CLAUDE.md): push with `--force-with-lease` after a rebase | A worker obeying CONTRIBUTING cannot do what the launch prompt asks |
| C2 | `CLAUDE.md` Tests: no `make` targets, cargo directly | [`CONTRIBUTING.md`](../../CONTRIBUTING.md) "Checks" and [`docs/DEVELOPMENT.md`](../DEVELOPMENT.md) list `make` targets; the [PR template](../../.github/pull_request_template.md) example is `make greenfield-ci  # exit 0`; `AGENTS.md` step 3 says "shared full-test entry point" | #720 fixed three files and left these four |
| C3 | `CONTRIBUTING.md#milestones`: a new issue goes to the release it blocks, "otherwise to `Hardening`" | Milestone descriptions of the closable batches: "New follow-ups enter through Maintenance: hardening" | Two answers to "which milestone" for discovered work |
| C4 | `repo-workflow`: give a new issue "an owner" | GitHub offers one assignable user, `Androlax2` (Theo); 1 of the 88 issues created since 2026-11-02 has an assignee | The rule cannot be followed as written; real ownership lives in sessions and branches |

### Missing guidance

| Gap | Evidence |
| --- | --- |
| How to report on the issue | No instruction file mentions issue comments, progress or blockers (grep of `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`, `.agents`, `docs/DEVELOPMENT.md`) |
| Definition of done | `CLAUDE.md` stops at "arm `gh pr merge --auto`"; `repo-workflow` ends at "verify remote merge state"; neither says to stay until merged |
| Research document form | 15 issues are titled "Research" (13 open); the form exists only by example in `docs/research/` |
| Waiting on CI | Client-specific; written in no file |

### Issue and pull request baseline (measured)

- **Issue comments.** 432 comments on 400 issues (#61 to #739), **all by one login**, because
  workers act under Theo's account: agent and owner cannot be told apart, so this baseline cannot
  measure agent spam. 162 issues (40%) have no comment. Median 415 characters; 92 comments (21%)
  are under 200 characters. None of the 432 starts with an acknowledgement phrase (pattern match;
  12 of the 92 short ones read by hand were triage or decision pointers), and no issue repeats a
  comment. Today's issue traffic is low and informative; the rules below are preventive for more
  autonomous workers, not a response to existing spam.
- **Pull requests.** 192 merged `greenfield/` PRs: body median 3,726 characters (≈930 tokens,
  est.), p90 6,305; 44 PR comments in total. Conventional titles: 20 of the oldest 60, 57 of the
  newest 60, so the rule is learned. The PR body, not the issue, is where delivery is recorded.
- **Ownership.** 17 of 400 issues have a comment matching a session-id pattern; 1 of the newest 88 issues is
  assigned. Milestone: 88 of 88 newest issues have one.

## Recommendation: a layered map

| Layer | Holds | Loaded | Owning file | Change |
| --- | --- | --- | --- | --- |
| 1. Always | Facts every session needs: authorization, how to test, where rules live, the four-line [issue-update rule](#issue-updates) | Automatic in both clients | `AGENTS.md` (shared); `CLAUDE.md` keeps `## Git authorization` | D1 below; trim, never grow |
| 2. Scoped rules | Area rules for Rust, scripts, protocol, CI | Linked from the index | `.agents/rules/*.md` | None (4 files, 4.7 KB; the explicit index is right, since clients differ on path-scoped metadata) |
| 3. On-demand skill | The autonomous delivery path and its templates | Skill trigger, or link | `repo-workflow` and its `references/` | Rewrite, no new skill |
| 4. Ordinary docs | Crate ownership, research form, decisions | When the agent opens them | crate `README.md`; `docs/research/README.md` (new, 20 lines); `docs/decisions/` | One new file |
| 5. Enforcement | Rules with no exceptions | Always, by the machine | Torlek grants, `main` ruleset, one workspace-rules test | One test (P6) |

**Why no new skill.** Each candidate fails the Next.js and Anthropic tests (recurring task,
too long for the always-loaded file, self-contained):

| Candidate | Recurring task or failure | Verdict |
| --- | --- | --- |
| Issue updates / follow-up filing | Every issue | A section of `repo-workflow`, not a skill: it is a step of the delivery path, and a separate trigger would fire on the same tasks |
| Research deliverable | 13 open research issues | Ordinary document next to the files: it applies to a path |
| Per crate | None found; 30 of 30 crates already have a README opening with an ownership sentence | No |
| Verify / apply | Deleted by #720 | No |
| Third-party packs | n/a | #687 decides; skills there are a default the owner can replace, never a rule |
| Torlek verbs | Covered by the shipped `torlek` skill | Reference it, never restate it |

`repo-workflow` (865 words) mixes five jobs and is written for a coordinator. A worker needs the
path from one issue to a merged PR. Coordination material (assigning workers, briefs, handoff
registry) moves to `references/coordination.md`, read only when coordinating; that is how the
[specification](https://agentskills.io/specification) intends bodies to shrink.

### Where each fact lives (one place each)

| Fact | Authoritative file | Others |
| --- | --- | --- |
| What the agent may do with Git, `gh`, merging | `CLAUDE.md` `## Git authorization` | Link only |
| How to run checks | `AGENTS.md` Verification (moved from `CLAUDE.md`) | `.github/pull_request_template.md` shows the same commands |
| Labels, milestones, branch, PR title | `CONTRIBUTING.md` | Skill links |
| Procedure, states, templates | `repo-workflow` | None |
| Status, approvals, questions to the owner | Shipped `torlek` skill | `repo-workflow` links |
| Research form | `docs/research/README.md` | None |

## Workflow design

### The path from issue to merge

States are distinct and each is reported as what it is:

| State | Means | Say |
| --- | --- | --- |
| Local | Commits on the branch, checks run locally | Nothing on the issue |
| Open PR | Pushed, review run, auto-merge armed | The PR is the record; `Closes #<n>` links it to the issue |
| Merged source | `gh pr view` shows `MERGED` and a merge SHA on `main` | Done for the source; the PR closes the issue |
| Accepted live | The owner's walk or release step passed | Only the owner records it; never inferred from merge |

1. Read the issue and its linked issues, then check `gh issue list --search`, open PRs and
   `greenfield/*` branches for an owner (see [overlap](#existing-owner-overlap)).
2. Branch `greenfield/<n>-<slug>` in your own clone; `git status` first; keep others' changes.
3. Smallest complete change through the real entry point; focused cargo test while iterating.
4. One `/code-review` at medium on the branch delta; fix; run `--workspace` tests once.
5. Push the branch, open the PR from the template, arm auto-merge (`CLAUDE.md` authorizes this).
6. **Stay until merged**: wait on CI in a blocking loop, fix failures, rebase with
   `--force-with-lease` when `main` moves and GitHub reports a conflict.
7. Report each state as the table above says. A skipped check is named, never counted as a pass.

### Issue updates

Three places, three jobs:

| Channel | For | Never for |
| --- | --- | --- |
| `torlek agent report --doing` | What you do now (2 to 12 words); the owner's live view | History |
| `--needs-input` | A question only the owner can answer, then end the turn | Status |
| Issue comment | A durable fact that changes what the next reader does | Acknowledgement, "started", repeating the PR body, credentials, transcripts, token counts |

Comment on the issue only for: **blocked** (exact blocker, owner, resume condition), **scope or
acceptance changed**, **follow-up filed**, or **delivery** when the PR cannot close the issue
(acceptance remains, or a check was skipped). Otherwise the PR is the delivery record. Never post
the same state twice; if nothing changed, post nothing.

```text
Blocked: <what cannot proceed, one line>
Evidence: <command and exact result, or link>
Owner: <Theo | lane/session | external> to <action>
Resume when: <observable condition>
```

```text
Changed: <how this differs from the issue text>
Evidence: <link or output>
Effect: <acceptance item, milestone or dependency touched>
Decision needed: <none | question, with the recommended option>
```

```text
Delivered in #<PR> (<open | merged <sha>>).
Acceptance: <item>: <test name or run>
Not done: <skipped check or open acceptance, and why>
Follow-ups: <#n or "none">
```

### Discovered work

1. **Search first**, open and closed: `gh issue list --state all --search "<keywords>"`, then PR
   titles and `greenfield/*` branches. Found and active: comment there only if you hold new
   evidence; do not file, do not start.
2. **Needed for this issue's acceptance, same owner, same crate**: fix it in the PR, as its own
   commit, and say so in the PR body.
3. **Otherwise file a follow-up** (when the task allows filing): one problem, a `Depends on:`
   line, labels from `CONTRIBUTING`, and the milestone by one rule: the release it blocks,
   else `Maintenance: hardening`, which triage moves (resolves C3). Leave it **unassigned**
   (only Theo is assignable) and name the owning crate or lane in the body (resolves C4).
4. **Escalate** only what policy cannot settle: new dependency, token scope, security,
   spending, anything irreversible, a conflict with a `decided` report. Open a `decision-needed`
   issue or use `--needs-input`; do not guess.
5. Never widen scope silently; never duplicate an active lane; never invent an assignee.

Research issues are the exception that this one is: list proposed follow-ups in the PR and the
issue, and Theo files them.

### Existing-owner overlap

Read the other issue's latest comments, its branch and PR. Active branch or open PR: do not start,
comment once with the overlap and what you would add, and `--needs-input` if the scope conflicts.
No activity: ask Theo; takeover is his call, not an inference from silence.

### Worked examples

| Case | What the agent does | What it does not do |
| --- | --- | --- |
| Ordinary task | Steps 1 to 7; no issue comment; PR body carries commands and results | Post "started" or "PR opened" on the issue |
| Blocked | Blocked template once; `--needs-input` if Theo must answer; end the turn | Retry the denied action another way; repeat the comment each wake |
| Newly discovered | Search; none found; file a follow-up with milestone by the rule; link it in the PR | Fix an unrelated crate in this PR |
| Existing-owner overlap | Find the open PR on the same files; comment once; wait for Theo | Edit the same files in a parallel branch |
| Completed delivery | PR merged, SHA recorded; Delivered template only if acceptance remains | Close a broader issue because the PR merged |

### Hygiene, preserved boundaries

Own clone and branch per issue; check `git status` before editing; never revert changes you did
not make; one broad `--workspace` run before pushing and no duplicates across clones (already in
`CLAUDE.md`); keep the evidence in the PR body; update the coverage-map row and decision file the
issue names; never delete branches (CONTRIBUTING). Autonomy is not authority: the skill states
the path, and Torlek grants, the `main` ruleset and `CLAUDE.md` decide what is allowed. Nothing
here widens Git, dependency, host, publication or live-operation approval.

## Before and after

**1. Loading (D1, option A).** Before, `CLAUDE.md` opens:

```markdown
# Agent Cockpit

Engineering guidance: [AGENTS.md](AGENTS.md). Read its scoped rules before editing.
```

After:

```markdown
# Agent Cockpit

@AGENTS.md

## Git authorization
```

and the `## Tests` section moves into `AGENTS.md`, so Codex also sees it. Effect: a Claude worker
starts with 6.5 KB (≈1,600 tokens, est.) instead of 2.2 KB, but no longer depends on a link; Codex
gains the cargo-only rule. The import does not reduce cost ([doc](https://code.claude.com/docs/en/memory#import-additional-files)).

**2. Closing (`repo-workflow`).** Before, "Close against acceptance" is three paragraphs (about 190
words) that mix source delivery, host application, release and reporting. After, the
[states table](#the-path-from-issue-to-merge) (4 rows, about 60 words) plus one sentence: "A merged
PR can leave a broader issue open." The host-application text goes, because #720 removed that flow.

**3. Reporting.** Before: nothing. After, in `AGENTS.md`, four lines:

```markdown
## Issue updates

Status goes to `torlek agent report`; the PR is the delivery record. Comment on an issue only when
blocked (blocker, owner, resume condition), when scope or acceptance changed, when a follow-up was
filed, or when acceptance remains after the PR. Never acknowledge, repeat or paste transcripts.
Templates and discovered-work rules: [repo-workflow](.agents/skills/repo-workflow/SKILL.md).
```

## Decisions for Theo

- **D1. How both clients load the rules.**
  - **A (recommended):** `CLAUDE.md` imports `@AGENTS.md`; Tests move to `AGENTS.md`;
    `## Git authorization` stays in `CLAUDE.md`, which your global rule reads, with one link line in
    `AGENTS.md`. Pros: small, documented, deterministic for Claude. Cons: Codex still reaches the
    authorization by link.
  - **B:** everything shared in `AGENTS.md`, `CLAUDE.md` only the import. Pros: one file for both.
    Cons: the authorization is no longer text in `CLAUDE.md`, which your global Git rule looks for.
  - **C:** leave as is and rely on the links. Pros: no edit. Cons: Codex never loads `CLAUDE.md`;
    the next banned command repeats #720's pattern.
- **D2. Rebase rule (C1).** Recommend: rebase and `--force-with-lease` on your own branch only when
  GitHub reports a conflict (this is what `CLAUDE.md` and the launch prompt say); delete the
  "never force-push, merge `main` in" lines from `CONTRIBUTING`.
- **D3. `docs/research/README.md`.** Recommend yes: form (answer, labels, decision checkbox,
  sources pinned, follow-ups listed not filed), 20 lines.

## Rollout plan

Times are for one agent with the focused checks. Each item is its own PR; none starts before
approval. Savings are reduced contradiction and re-explaining, not a token claim.

| # | Change | Files | Time | Depends | Check |
| --- | --- | --- | --- | --- | --- |
| P1 | Remove C1 to C3: `make` references in `CONTRIBUTING`, `DEVELOPMENT`, PR template, `AGENTS.md`; rebase rule; milestone intake | those five files | 1 h | D2 | `python3 tools/quality.py` `markdown`, `links`, `typos`; grep finds no banned command |
| P2 | Loading per D1 | `CLAUDE.md`, `AGENTS.md` | 30 min | D1, P1 | A fresh session lists both files as loaded |
| P3 | Rewrite `repo-workflow`: delivery path first, coordination into `references/` | skill, `scenarios.md` | 2 to 3 h | P1 | Scenarios below |
| P4 | Issue-update rule in `AGENTS.md`; templates and discovered-work in the skill | `AGENTS.md`, skill | 1 h | P3 | Scenarios below |
| P5 | `docs/research/README.md`; check whether Claude Code lists `.agents/skills` on the current version, and if not decide a `.claude/skills` link | new file; `.claude/` | 1 h | D3 | The skill appears in a Claude session's listing |
| P6 | Regression test in `cockpit-workspace-rules`: instruction files name no command that `CLAUDE.md` bans; skill description within the cap. Expires with the cutover (#66) | one test file | 2 h | P1 | `cargo nextest run --locked -p cockpit-workspace-rules` |

Order: P1, then P2 and P5 in parallel, then P3, P4, P6. About an afternoon in total.

### Proposed follow-up issues (not filed)

All proposed for `Maintenance: hardening (no release)`; P1 and P2 may justify `v0.1.0-alpha.3`.
Labels `greenfield`, `documentation`, `area:build`. Unassigned.

1. docs: remove contradictions between `CLAUDE.md` and CONTRIBUTING, DEVELOPMENT, the PR template and `AGENTS.md` (P1; blocked by D2).
2. docs: load `AGENTS.md` and the cargo-only Tests rule in both clients (P2; blocked by D1).
3. docs(workflow): turn `repo-workflow` into the autonomous issue-to-merge path (P3, P4).
4. docs(research): `docs/research/README.md` and Claude Code skill discovery for `.agents/skills` (P5).
5. test(workspace-rules): guard instruction files against banned commands (P6).
6. Reconcile with [#687](https://github.com/Androlax2/agent-cockpit/issues/687) once its list lands: which shipped skills overlap the delivery path (`triage`, `to-tickets`, `pr` appear in its candidate list; no judgement made here).

## How to validate it without Theo repeating it

Not run here (it needs live provider sessions and their authorization).

- **Cold-start test.** For each of five scenarios (the three in
  [`scenarios.md`](../../.agents/skills/repo-workflow/references/scenarios.md) plus *blocked* and
  *discovered work*), start a fresh worker with only the issue text, no workflow instructions.
  Pass when it (a) reaches the right rule, (b) stays within the scenario's boundary, (c) posts the
  expected number of issue comments (0 for ordinary, 1 for blocked), and (d) reaches merged state
  without being told.
- **Record per run, kept apart:** instruction bytes loaded before the first edit (Cockpit-added
  only; harness totals are #479's), tool turns to first edit, issue comments posted, and which
  files were read. Compare with today's baseline above before and after P2 to P4.
- **Pass criteria come from the baseline**, not a target set here: no more issue comments than
  today's mean of 1.08 per issue, no repeated comment, no acknowledgement-only comment, and no
  more start-up bytes than the 6.5 KB D1 states.
- Run the same scenarios on Claude Code and Codex.

## What this does not settle

Token cost in provider terms, Codex's real adherence, whether `.claude/skills` discovery can read
`.agents/skills` on the installed version, and any judgement of the mattpocock skills (#687).

## Research delivery versus acceptance

This document supplies the inventory, contradictions, layered map, workflow rules, templates,
rollout plan and proposed issues. It changes no executable behaviour. Verification for this PR is
`tools/quality.py` markdown, links and typos on this file and its appendix, the focused
`cockpit-workspace-rules` tests, and one `/code-review` at medium; the exact commands and outcomes
are in the PR. Implementation and Theo's decisions D1 to D3 remain open.
