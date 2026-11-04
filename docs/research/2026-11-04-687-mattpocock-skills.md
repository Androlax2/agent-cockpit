# Which mattpocock/skills fit Torlek and its workers

Issue [#687](https://github.com/Androlax2/agent-cockpit/issues/687).
**Research and proposal, 2026-11-04; Theo has not decided.** Merging this document
accepts research delivery only. It does not authorize installing any skill, adding a
dependency, fetching the repository from a product code path, or changing an instruction,
workflow, role prompt or release. The skills were read as data. None was installed and no
script from them was run.

## Recommendation and decision still needed

Ship no skill from this repository by default. Offer **a small optional pack, installed by
the owner, pinned to one commit**, and start it with three skills:
[`diagnosing-bugs`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/diagnosing-bugs/SKILL.md),
[`tdd`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/tdd/SKILL.md)
and [`writing-for-agents`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/writing-for-agents/SKILL.md).
They fit the `code-change` workflow's implement and check steps and the people who write
Torlek's instruction text, need no change to run under Torlek's rules, and add 504 bytes
of frontmatter to a session's always-loaded context, counted the way `torlek brief --measure`
counts (Torlek's own is 1389 bytes).

Take the **ideas** of `chief-of-staff`, `handoff`, `retro` and `research` into Torlek-owned
instructions (the Chief, compaction [#686](https://github.com/Androlax2/agent-cockpit/issues/686),
the `research` workflow preset), and ship none of the four. Keep `pr`, `codebase-design` and
`wait-what` in the pack but off until the owner asks. Skip the other 29. The largest groups of
skips are the issue-tracker flow (`triage`, `to-spec`, `to-tickets`, `wayfinder`,
`setup-matt-pocock-skills`), the build flow (`implement`, `implement-spec`, `prototype`) and
`code-review`.

The owner stays in charge by construction: the pack is off until the owner turns it on, each
skill is on or off by name, an owner's own skill or a project's skill with the same name wins,
and the pack never edits an owner file. The pack is a default the owner can see, replace or
remove, not a rule (Theo's constraint, 2026-11-03).

- [ ] Theo records whether an optional pack is wanted, and which starter set.

Until that decision is recorded, do not create implementation work from this proposal. The
follow-up issues below are proposed, not filed.

## Evidence snapshot and provenance

Source examined: [`mattpocock/skills@4ae2317`](https://github.com/mattpocock/skills/tree/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d)
(`main` on 2026-11-04, committed 2026-10-31), MIT, version 1.3.2 in its
[plugin manifest](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/.claude-plugin/plugin.json).
It holds the same 39 skills the issue lists on 2026-11-03: 20 `engineering`, 8 `in-progress`,
4 `misc`, 7 `productivity`. Every `SKILL.md`, every file beside them, `README.md`, `SCOPE.md`,
`CLAUDE.md`, `.agents/invocation.md`, `.out-of-scope/` and ADR 0002 were downloaded from that
commit and read in a scratch folder. A `docs/engineering/resolving-merge-conflicts.md` page
exists with no skill behind it; it is not counted.

Harness behaviour comes from each harness's own documentation, fetched 2026-11-04:
[Claude Code skills](https://code.claude.com/docs/en/skills),
[Codex skills](https://learn.chatgpt.com/docs/build-skills) (the old
`developers.openai.com/codex/skills` URL redirects there) and
[OpenCode skills](https://opencode.ai/docs/skills/). Torlek's side comes from this checkout
(`HEAD` `2c89c7bd`), cited by path.

How the repository sorts its own skills matters here. Its
[`CLAUDE.md`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/CLAUDE.md)
says only `engineering/` and `productivity/` are promoted and ship in the plugin (27 skills);
`in-progress/` is beta that "can change or disappear without warning"
([README](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/in-progress/README.md));
`misc/` is "frozen and unmaintained"
([`.out-of-scope/frozen-misc-skills.md`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/.out-of-scope/frozen-misc-skills.md)).
New skills are not accepted from outside ([`new-skills.md`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/.out-of-scope/new-skills.md)),
so the owner of this catalog is one person and a change reaches us only as a new commit.

## What Torlek needs from a skill

Four rules from Torlek's own design decide most rows below.

1. **Skills grant nothing.** [Report 01](../design/greenfield/01-scope-and-principles.md) and the
   [launch brief](../../crates/cockpit-agent-context/bundle/SKILL.md) say prose requests and
   Torlek's grants decide. A skill that tells an agent to commit, push, merge, install, open a
   process outside its session or edit the owner's files is asking for something the grants may
   refuse. The skill text is then wrong for that session, not the grant.
2. **A skill can also grant.** Claude Code's `allowed-tools` frontmatter pre-approves tools for
   the turn that invokes the skill, and "workspace trust doesn't gate this field"
   ([docs](https://code.claude.com/docs/en/skills)). No skill in the pack uses `allowed-tools` or
   `hooks` (a search of every file at the pinned commit found neither), so the pack as pinned
   grants nothing. A later commit could. This is why the pin matters.
3. **Project skills load only for trusted projects, and name collisions get visible diagnostics.**
   [Report 08](../design/greenfield/08-agent-onboarding-context.md) and
   [report 09](../design/greenfield/09-project-setup-readiness.md).
4. **Always-loaded context is counted** (`torlek brief --measure`, [#419](https://github.com/Androlax2/agent-cockpit/issues/419),
   [#619](https://github.com/Androlax2/agent-cockpit/issues/619)). A skill's description is
   paid for in every session whether or not the skill fires.

Torlek already delivers its own skill this way. The daemon writes the `torlek` skill into
`agent-context` under its state folder, and writes a second copy into `claude-skills`, laid out
as `.claude/skills/torlek`; the Claude adapter adds that folder with `--add-dir` so Claude Code
lists it (`crates/cockpit-claude/src/lib.rs`, `crates/cockpit-daemon/src/agent_context.rs`).
The OpenCode adapter adds a folder to its `skills.paths` (`crates/cockpit-opencode/src/lib.rs`;
its code comment still calls the skill `cockpit`, the bundle's name is `torlek`). A pack would
reuse the same two routes, written in each adapter's layout, so no new delivery channel is
needed. Not run: a launch of either adapter to confirm what the harness then lists.

## Measured: what each skill costs in every session

Method: for each `SKILL.md`, the bytes of its YAML frontmatter, the same quantity
`torlek brief --measure` reports as the catalog entry
(`context_budget` in `crates/cockpit-agent-context/src/brief.rs`), and the bytes of
`name` plus `description`, which is what a harness lists. Compare Torlek's figure with the
frontmatter column; the name-plus-description column is the lower bound a harness lists. Bytes are measured. Tokens are not:
no tokenizer was run, and a count from a provider's tokenizer would need a live call. Where this
page says "about N tokens" it divides bytes by 4, an estimate for English prose that differs by
provider.

Torlek today, from `torlek brief --measure` in this session:
**1389 bytes always loaded** (brief 1113, `torlek` catalog entry 276), 9862 bytes on demand.

| Set of pack skills | Skills | Frontmatter bytes | Name + description bytes | About tokens (estimate) |
| --- | --- | --- | --- | --- |
| Recommended starter (`diagnosing-bugs`, `tdd`, `writing-for-agents`) | 3 | 504 | 444 | 110 |
| The 11 promoted skills the model may invoke itself | 11 | 2676 | 2260 | 565 |
| All 27 promoted skills (the Claude Code plugin) | 27 | 5515 | 4161 | 1040 |
| All 39 skills in the repository | 39 | 7952 | 5984 | 1500 |

Three results follow. The first rests on documentation, not a run; the other two are measured
bytes or quoted limits.

- **User-invoked skills cost nothing on Claude Code and Codex, and full price on OpenCode.**
  16 of the 27 promoted skills set `disable-model-invocation: true`. For those, Claude Code's
  table says "Description not in context, full skill loads when invoked"
  ([docs](https://code.claude.com/docs/en/skills)); Codex honours the pack's
  `agents/openai.yaml` `policy.allow_implicit_invocation: false`, which "won't invoke the skill
  from prompt content alone" ([docs](https://learn.chatgpt.com/docs/build-skills)). OpenCode's
  documentation lists only `name`, `description`, `license`, `compatibility` and `metadata`,
  says "unknown frontmatter fields are ignored" and "OpenCode lists available skills in the
  `skill` tool description" ([docs](https://opencode.ai/docs/skills/)). So on OpenCode the
  16 user-invoked skills add their 1901 name-plus-description bytes (about 475 tokens) to the
  2260 of the model-invoked ones. Not run: a real OpenCode session to confirm this.
- **Bodies cost only when a skill fires, then stay.** The starter's three bodies are 8464
  (`diagnosing-bugs`), 3448 (`tdd`) and 10736 (`writing-for-agents`) bytes, plus Markdown
  references read on demand: 3695 for `tdd`, 2629 for `writing-for-agents`, none for
  `diagnosing-bugs`. Claude Code keeps an invoked skill in context across turns and, after
  auto-compaction, re-attaches "the first 5,000 tokens of each" within "a combined budget of
  25,000 tokens" ([docs](https://code.claude.com/docs/en/skills)). By the 4-bytes-per-token
  estimate all three bodies fit under the per-skill cap.
- **The pack's largest descriptions go to skills Torlek would not use.** `code-review` (428
  bytes) and `wizard` (319) are the two largest model-invoked entries in the promoted set, and
  both are skips below.

Not measured: whether a skill improves a worker's result. That needs a with/without trial on
fixture tasks and live provider calls, which need Theo's authorization
([#619](https://github.com/Androlax2/agent-cockpit/issues/619) sets the same bar). Follow-up 3
proposes it. Also not measured: whether a Torlek launch loads the owner's personal
`~/.claude/skills` beside the pack; this session's own skill list suggests it does, but no
launch test was written.

## Skill by skill

**Keep** = in the starter pack, used as pinned. **Optional** = in the pack, off until the owner
turns it on. **Adapt** = ship nothing; take the idea into a Torlek-owned instruction.
**Skip** = leave out. Links go to the pinned commit.

### Developer workers (the `code-change` and `research` workflows)

| Skill | Verdict | Why |
| --- | --- | --- |
| [`diagnosing-bugs`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/diagnosing-bugs/SKILL.md) | Keep | Builds a failing, fast, repeatable check before any theory, then a regression test. That is the `check` step's evidence rule in other words. It asks the user only when no loop can be built ("stop and say so"), which is a `--needs-input` report under Torlek's worker rule. It redacts secrets before showing output. Its `scripts/hitl-loop.template.sh` is a template the agent writes; nothing runs it. |
| [`tdd`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/tdd/SKILL.md) | Keep | Tests through public seams, red before green, one vertical slice at a time. Matches the repo's rule that tests drive real entry points. It says to "confirm [seams] with the user"; a worker does that with `torlek agent report --needs-input`, so no edit is needed. It names `codebase-design` and `code-review` only as references, and works without them. |
| [`pr`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/pr/SKILL.md) | Optional | A PR-body template (summary visual, before/after evidence, one-way or two-way door). This repository already has [a PR template](../../.github/pull_request_template.md) and the skill does not defer to one, so both would apply here. Useful in a project that has none. The smallest model-invoked description in the pack (29 name-plus-description bytes). |
| [`codebase-design`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/codebase-design/SKILL.md) | Optional | Deep-module vocabulary that `tdd` consults. 280 bytes always loaded for a vocabulary a worker mostly does not need. |
| [`research`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/research/SKILL.md) | Adapt | Its three rules (primary sources, one cited Markdown file, match the repo's convention) are what the `research` workflow preset's steps already ask. It also tells the agent to start a background agent, which a Torlek session may not be able to do, and its name equals the preset's. Fold the rules into the preset step text; ship no skill. |
| [`code-review`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/code-review/SKILL.md) | Skip | The name is the harness's own `/code-review`, which this repository's review gate names. The pack's author [declines to rename](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/.out-of-scope/harness-name-collisions.md) and tells users to reach theirs as `/mattpocock-skills:code-review`. [Report 08](../design/greenfield/08-agent-onboarding-context.md) records a gate by the resolved command identity and version; a same-named skill makes that identity ambiguous. It also needs the tracker setup that `setup-matt-pocock-skills` writes. The two-axis idea (Standards, Spec) is worth lifting into a review prompt; see follow-up 4. |
| [`implement`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/implement/SKILL.md), [`implement-spec`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/implement-spec/SKILL.md) | Skip | `implement` ends "Commit your work to the current branch". `implement-spec` creates an integration branch and worktrees, opens a draft PR, runs merger subagents and cleans worktrees. Each is a git or fan-out action that Torlek's grants, scheduling and placement own (reports 07 and 10), and each overlaps the `code-change` preset. |
| [`prototype`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/prototype/SKILL.md) | Skip | Tells the agent to commit the prototype "to a throwaway branch". A git write with no grant. |
| [`improve-codebase-architecture`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/improve-codebase-architecture/SKILL.md) | Skip | A whole-repository survey for the owner to run now and then. [#152](https://github.com/Androlax2/agent-cockpit/issues/152) already plans a per-crate `/simplify` pass before the first stable release. |
| [`domain-modeling`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/domain-modeling/SKILL.md), [`grill-with-docs`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/grill-with-docs/SKILL.md) | Skip | They build `GLOSSARY.md` and `docs/adr/` files. This repository records decisions as one file each under `docs/decisions/` and has no glossary, so the skills would start a second decision log. |
| [`wizard`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/wizard/SKILL.md) | Skip | Generates a bash script that opens URLs, reads secrets and runs `gh secret set`. Steps only a human can do already go through `torlek ask guide`, and secrets stay with the owner. Its description is the second-largest model-invoked entry (319 bytes). |

### The issue and planning flow

| Skill | Verdict | Why |
| --- | --- | --- |
| [`triage`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/triage/SKILL.md), [`to-spec`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/to-spec/SKILL.md), [`to-tickets`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/to-tickets/SKILL.md), [`wayfinder`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/wayfinder/SKILL.md) | Skip | All four post to an issue tracker (labels, comments with an AI disclaimer, native blocking links, `wayfinder:*` labels, closing issues) and read the tracker settings that `setup-matt-pocock-skills` writes. This repository's own flow (milestones, an owner per issue, `Closes #`, [repo-workflow](../../.agents/skills/repo-workflow/SKILL.md) and [#620](https://github.com/Androlax2/agent-cockpit/issues/620)) already covers it, and four of `triage`'s five state labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`) do not exist here. Torlek's own task list and workflow runs are the work queue; a second queue in GitHub labels would disagree with them. The one piece worth keeping is [`AGENT-BRIEF.md`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/triage/AGENT-BRIEF.md): "durability over precision", behaviour not file paths, testable acceptance, explicit out-of-scope. It is the same advice `writing-for-agents` gives for briefs, and it belongs in the task-packet text ([#103](https://github.com/Androlax2/agent-cockpit/issues/103)). |
| [`setup-matt-pocock-skills`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/setup-matt-pocock-skills/SKILL.md) | Skip | It runs `git init` and `git remote add origin` when no remote exists, runs `gh label create`, and edits `CLAUDE.md` or `AGENTS.md`. All three are host or repository writes that the issue forbids a skill from instructing. It is also the prerequisite of every skill in the row above. |
| [`ask-matt`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/ask-matt/SKILL.md) | Skip | A router over the skills in this repository. With a subset installed it routes to skills that are not there. |

### The Chief and the coordinator

| Skill | Verdict | Why |
| --- | --- | --- |
| [`chief-of-staff`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/in-progress/chief-of-staff/SKILL.md) | Adapt | See the next section. |
| [`retro`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/engineering/retro/SKILL.md) | Adapt | Suggests changes to the agent's environment (navigation pointers, automated checks, steering files, tool economy), most severe first. Its sources are "session logs on this machine", transcripts that [#619](https://github.com/Androlax2/agent-cockpit/issues/619) puts behind separate authorization. A Torlek retro would read Torlek's own records (events, receipts, handoffs). Keep the categories. |

### Briefs, handoffs and teaching

| Skill | Verdict | Why |
| --- | --- | --- |
| [`writing-for-agents`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/writing-for-agents/SKILL.md) | Keep | Every rule it states is one Torlek is already working to: pay for always-loaded words, push reference behind a pointer, one source of truth, hunt no-ops, state the target instead of banning. It reads like the criteria of [#619](https://github.com/Androlax2/agent-cockpit/issues/619) and [#620](https://github.com/Androlax2/agent-cockpit/issues/620). It fits whoever writes the instruction bundle, a role prompt or a project's `CLAUDE.md`. It does not apply to a worker doing ordinary code changes. |
| [`handoff`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/handoff/SKILL.md) | Adapt | The model writes a summary to the OS temp folder. Torlek already has the stronger form: a workflow handoff of at most 2 KiB with evidence references, built from durable records (`crates/cockpit-agent-context/bundle/references/workflow.md`). Three habits carry over into [#686](https://github.com/Androlax2/agent-cockpit/issues/686): reference artifacts instead of copying them, redact secrets, and name the skills the next agent should load. |
| [`claude-handoff`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/in-progress/claude-handoff/SKILL.md) | Skip | Runs `claude --bg ...`, which starts an agent outside Torlek's session, sandbox, grants and outbox. The one pattern that sidesteps every rule in this document. |
| [`teach`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/teach/SKILL.md) | Skip | A multi-session course workspace (`MISSION.md`, `lessons/*.html`, `learning-records/`) written into the current folder. The quickstart's pasted [explain-it-to-me prompt](../QUICKSTART.md#let-your-agent-explain-it-to-you) is deliberately read-only ("Do not change anything") and already asks a check question after each idea, which is the part of `teach` that applies. |
| [`wait-what`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/wait-what/SKILL.md) | Optional | Four lines asking the agent to re-pitch the last message in plain English. User-invoked, 59 name-plus-description bytes. An owner who wants that gets it for free; it reads `GLOSSARY.md` only if there is one. |
| [`to-questionnaire`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/to-questionnaire/SKILL.md) | Skip | Writes a Markdown questionnaire for a third person. A worker's question to the owner goes through `torlek agent report --needs-input`. |
| [`grilling`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/grilling/SKILL.md), [`grill-me`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/productivity/grill-me/SKILL.md) | Skip | `grilling` asks "the whole frontier in one round", numbered. Theo's own instructions say one question at a time, which makes this the clearest case of a shipped default disagreeing with an owner. An owner who likes it installs it themselves. A worker must not run an interview; it reports one question and ends its turn. |

### In-progress and misc buckets

| Skills | Verdict | Why |
| --- | --- | --- |
| `loop-me`, `setup-ts-deep-modules`, `writing-beats`, `writing-fragments`, `writing-shape` | Skip | Workflow design, a TypeScript `dependency-cruiser` setup, and article writing. Torlek is Rust and none serves a worker. All are beta and "can change or disappear without warning". |
| [`software-factory`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/in-progress/software-factory/SKILL.md) | Skip | A glossary for designing a factory of agents. It says to use its terms "and only these" and marks "session", "sandbox", "workflow" and "server" as words to avoid, which are Torlek's words. Reading it as a reference when the coordinator is designed ([#208](https://github.com/Androlax2/agent-cockpit/issues/208)) is fine; loading it into a session is not. |
| [`git-guardrails-claude-code`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/misc/git-guardrails-claude-code/SKILL.md) | Skip | A hook that blocks every `git push` by pattern match. Torlek's sandbox and grants already decide git writes, per project, and this one would block an authorized push too. The frozen-skills note records two closed reports, #301 (`git -C <dir> push` bypasses it) and #898 (it fails open without `jq`), because `misc/` is not maintained. |
| `migrate-to-shoehorn`, `scaffold-exercises`, `setup-pre-commit` | Skip | A TypeScript test library, course exercises, Husky. Out of scope and frozen. |

## The Chief: what `chief-of-staff` is worth

[`chief-of-staff`](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/skills/in-progress/chief-of-staff/SKILL.md)
is the skill the issue names for the Chief. Read against
[report 07](../design/greenfield/07-teams-and-chief.md), it cannot be dropped in. Three of its
instructions contradict the Chief's design:

| `chief-of-staff` says | Torlek's Chief |
| --- | --- |
| "All work should be done in subagents." | The Chief "decides"; scheduling places work and authorization permits it. The Chief "cannot also be a product worker". |
| "Harness-permitting, you will suggest recurring schedules." | A Chief wake "records why it happened and whether it produced a useful action"; the Chief never polls ([chief supervision](../chief-supervision.md), [#619](https://github.com/Androlax2/agent-cockpit/issues/619)). |
| Keep a wiki of Markdown pages and write into "a shared wiki ... the user ... already keep". | A Chief that writes files into a project repo is a worker-like write. State belongs in Torlek's records. |

Four ideas do fit, and each is an input to a Chief instruction, not a skill:

- **Context pointers.** Talk to workers by pointing at the issue, the note or the commit instead
  of copying it. Same direction as the 2 KiB handoff cap.
- **Write history, not the present.** Date every claim and cite the commit it was true at, so a
  stale memory is visible. This suits any durable note the Chief keeps.
- **Sample retros, worst first.** "A retro on about one in five" subagent sessions, "the ones
  where something went wrong". Torlek has the records to pick them without reading transcripts.
- **Improve the environment, not just the task.** Its "no workarounds" and "pit of success"
  framing matches the repository's own preference for one named opt-out over scattered bypasses.

The Chief is designed under [#54](https://github.com/Androlax2/agent-cockpit/issues/54) and
[#423](https://github.com/Androlax2/agent-cockpit/issues/423); the coordinator under
[#208](https://github.com/Androlax2/agent-cockpit/issues/208). Follow-up 4 hands them the four ideas.

## Fit with this repository's own skills and #620

- **`.agents/skills/repo-workflow`** already covers the task-to-merge path the mattpocock flow
  (`to-spec`, `to-tickets`, `implement`, `code-review`, `pr`) covers, with this repository's
  authorization rules in the text and "a skill ... grants no Git, dependency, host, merge or
  publication authority" as its first paragraph. It is the reason `triage`, `to-*`, `implement*`
  and `pr` are skips or optional rather than keeps. `verify-cockpit` and `apply-cockpit` have no
  counterpart in the pack.
- **[#620](https://github.com/Androlax2/agent-cockpit/issues/620)** asks for few, justified
  skills ("avoid a skill per crate", "reuse existing skills where they fit"). The three kept
  skills each have a recurring task behind them: a failing check before a fix, test-first slices,
  and writing instruction text. That research can adopt or reject them with its own evidence.
- **The owner's own skills.** This session lists `test-driven-development`,
  `systematic-debugging`, `requesting-code-review` and `code-review` among Theo's skills.
  A pack's `tdd` and `diagnosing-bugs` have different names, so no collision is reported, and
  both would load and compete for the same prompts. A name check cannot see an overlap of
  purpose. That is why the pack is opt-in per skill and lists the owner's installed skills
  beside its own when the owner chooses.

## Delivery options

| | A. Bundled in the binary | B. Optional pack, owner-installed, pinned (recommended) | C. The vendor's Claude Code plugin |
| --- | --- | --- | --- |
| What ships | A copy of chosen skills inside Torlek's release | A pinned snapshot the owner installs into Torlek's state folder; Torlek writes it where the adapters already look | `claude plugin install mattpocock-skills@claude-plugins-official`, run by the owner |
| Update | Our release; we carry the maintenance | Only when the owner moves the pin to a reviewed commit | The vendor's marketplace entry pins a sha, so "a release reaches installed users when that pin moves" ([ADR 0002](https://github.com/mattpocock/skills/blob/4ae2317ddb22b046e26455e4f9c9b3aaaf62212d/.agents/adr/0002-ship-as-a-claude-code-plugin.md)); the README says it "updates itself by default" |
| Owner control | Hard: a default in the binary | Per skill by name, off by default, removable in one step | `/plugin` only: "`skillOverrides` doesn't apply to plugin skills" ([docs](https://code.claude.com/docs/en/skills)) |
| Other harnesses | Ours to build | Ours to build, once, in the adapters | Claude Code only; others use the vendor's other routes |
| Reproducible | Yes | Yes (commit and tree hash) | No: a moving listing |
| Licence | We carry the notice | The pack carries the MIT text and copyright line beside the files | The vendor's |
| Verdict | Rejected: ships a default the owner did not choose, and every update is our review burden | Recommended | Rejected as Torlek's route: auto-update bypasses the pin, and the owner cannot turn a single skill off with the control Claude Code gives its other skills |

Rules for option B, each with the reason:

1. **Pin by commit and tree hash, never a branch.** The upstream keeps changing (its
   changelog is 66 KB and it is at version 1.3.2), and a skill can gain `allowed-tools` in a
   later commit. Moving the pin is an owner decision that shows the diff of the skill text.
2. **The pack holds only the keep and optional skills.** It is a selection of the pinned tree
   (the three starter skills plus `pr`, `codebase-design` and `wait-what`), not the tree. A skill
   this document skips never reaches a folder a harness reads, so OpenCode's missing per-skill
   flag and Claude Code's `skillOverrides` gap for plugin skills cannot expose one. The licence
   text (`LICENSE`, "Copyright (c) 2026 Matt Pocock") stays in the pack folder; MIT only
   requires the notice to travel with copies.
3. **Never modify a skill.** A changed skill is a fork nobody reviews. A needed change is a
   Torlek-owned instruction in the role prompt or brief. Product invariants are reserved and
   cannot be shadowed by a skill ([report 08](../design/greenfield/08-agent-onboarding-context.md)).
4. **Per harness, include only what that harness hides.** Claude Code and Codex hide a
   user-invoked skill from the model; OpenCode has no flag, so Torlek must omit user-invoked
   skills from the folder it gives OpenCode or deny them in its `permission.skill`.
5. **Show what loaded.** Report the pack's frontmatter bytes next to Torlek's own in
   `torlek brief --measure`, and list a name collision or a purpose overlap with the owner's
   skills instead of picking one silently. A name check cannot see two skills that answer the
   same prompts under different names, so the listing shows the owner's installed skills beside
   the pack's and the owner decides which to keep.
6. **The owner's and the project's skill wins on a name.** Design 08 and 09 already say project
   skills load only for trusted projects and collisions are shown.
7. **The pack never edits files in the project or the owner's home.** This excludes
   `setup-matt-pocock-skills` and its edits to `CLAUDE.md`.

Per-harness off switches, from the harnesses' documentation, for the owner's own use:
Claude Code `skillOverrides` (`off`, `name-only`, `user-invocable-only`) for non-plugin skills;
Codex `[[skills.config]]` entries in `~/.codex/config.toml`; OpenCode `permission.skill` with
`deny`.

## Proposed follow-up issues (not filed)

None starts until Theo records the decision above. Milestone for each is for Theo to set.

1. **An optional skill pack, owner-installed and pinned.** Scope: manifest with source URL,
   commit, tree hash and licence file; per-skill on/off by name in owner config; written to the
   folder the adapters already load; a listing that shows the owner's installed skills beside the pack's, with name collisions marked; refuse a pin
   whose skills use `allowed-tools` or `hooks` without the owner's confirmation. Acceptance:
   a launch test per adapter shows only enabled skills; a test shows an owner skill with the same
   name wins; `torlek brief --measure` reports the pack's bytes separately. Purpose overlap (for
   example `tdd` beside the owner's `test-driven-development`) is shown, not detected. Owner: the
   agent-context crate; depends on [#419](https://github.com/Androlax2/agent-cockpit/issues/419).
2. **Hide user-invoked skills from the model on every adapter.** Claude Code and Codex honour
   the pack's own flags; OpenCode does not. Acceptance: a fixture per adapter proves a
   user-invoked skill's description is absent from what the harness lists. Includes confirming
   the OpenCode behaviour that this research read only from its documentation.
3. **A measured with/without trial of the three starter skills.** Fixture tasks in the
   `code-change` workflow, same model, with and without the pack, recording turns, tokens (from
   provider reports, [#479](https://github.com/Androlax2/agent-cockpit/issues/479)) and check
   results. Needs Theo's authorization for provider spend. Output: keep, drop or change each
   skill. Coordinates with [#620](https://github.com/Androlax2/agent-cockpit/issues/620) and
   [#619](https://github.com/Androlax2/agent-cockpit/issues/619).
4. **Hand four ideas to the Chief, compaction and review designs.** Context pointers, dated
   history, sampled retros and the two-axis review (Standards, Spec) go to
   [#54](https://github.com/Androlax2/agent-cockpit/issues/54)/[#423](https://github.com/Androlax2/agent-cockpit/issues/423),
   [#208](https://github.com/Androlax2/agent-cockpit/issues/208) and
   [#686](https://github.com/Androlax2/agent-cockpit/issues/686) as comments with this
   document's anchors, not as new issues.
5. **Fold the `research` skill's three rules into the `research` preset step text.**
   Small: primary sources, one cited file, match the repo's convention. Test:
   the preset's packet text carries them (`crates/cockpit-workflows`).

## Research delivery versus acceptance

This document supplies a per-skill list with sources, byte measurements, a delivery
comparison and proposed follow-ups. It changes no executable behavior and installs
nothing.

Measured: frontmatter and name-plus-description bytes of every `SKILL.md` at the pinned
commit, and Torlek's own always-loaded bytes from `torlek brief --measure`. Read from
documentation only: how Codex and OpenCode treat user-invoked skills, the Claude Code
listing budget (1% of the context window, 1,536 characters per entry) and compaction
re-attachment. Not run: any tokenizer, any live harness session, any with/without quality
trial, any launch of a pack. Judgments about fit (the "Why" column) are reasoned from the
skill text and Torlek's rules, not from watching a worker use the skill.

Verification for this pull request is the repository's docs gate (`markdown`, `links` and
`typos` of `tools/quality.py`) and one `/code-review` at `medium`; their actual results are
in the pull request description. Broad Rust suites are not proof for prose and were not run.
Theo's decision remains open.
