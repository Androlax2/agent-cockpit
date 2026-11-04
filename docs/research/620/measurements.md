# Measurements behind the #620 findings

Appendix to [the findings](../2026-11-04-620-agent-workflow-conventions.md). Taken 2026-11-04 at
Cockpit [`0723de52`](https://github.com/Androlax2/agent-cockpit/tree/0723de5248e04f199af9801df83b39f32902b6b8).
Bytes and words are *measured*; tokens are *est.* (bytes ÷ 4, about ±25%, no tokenizer on this host).

## Instruction files in this repository

```bash
wc -c -w AGENTS.md CLAUDE.md CONTRIBUTING.md docs/DEVELOPMENT.md .agents/rules/*.md \
  .agents/skills/repo-workflow/SKILL.md .agents/skills/repo-workflow/references/scenarios.md
wc -c crates/*/README.md
```

| File | Bytes | Words | Est. tokens |
| --- | ---: | ---: | ---: |
| `CLAUDE.md` | 2,159 | 322 | 540 |
| `AGENTS.md` | 4,372 | 587 | 1,090 |
| `.agents/rules/rust.md` | 1,313 | 186 | 330 |
| `.agents/rules/ci.md` | 1,308 | 177 | 330 |
| `.agents/rules/scripts.md` | 1,071 | 151 | 270 |
| `.agents/rules/protocol.md` | 992 | 141 | 250 |
| `.agents/skills/repo-workflow/SKILL.md` | 6,255 | 865 | 1,560 |
| `.agents/skills/repo-workflow/references/scenarios.md` | 2,640 | 397 | 660 |
| `CONTRIBUTING.md` | 6,517 | 949 | 1,630 |
| `docs/DEVELOPMENT.md` | 10,001 | 1,319 | 2,500 |
| 30 crate `README.md` files | 182,366 | | 45,600 |

Chain for a Claude Code worker: `CLAUDE.md` (2,159) + `AGENTS.md` (4,372) + `rust.md` (1,313) +
`repo-workflow` (6,255) + `CONTRIBUTING.md` (6,517) = **20,616 bytes**; with `DEVELOPMENT.md`,
30,617. The `repo-workflow` description line is 295 characters, under the specification's 1,024.

## The same measure in other repositories

Read with `gh api -H "Accept: application/vnd.github.raw" repos/<repo>/contents/<file>` at the
commit shown.

| Repository | File | Commit | Bytes | Lines |
| --- | --- | --- | ---: | ---: |
| astral-sh/uv | `AGENTS.md` | `33f003cb85d7` | 3,746 | 49 |
| rust-lang/rust-analyzer | `AGENTS.md` | `d385d3dc167c` | 3,503 | 60 |
| biomejs/biome | `AGENTS.md` | `fca5c35007fc` | 5,178 | 88 |
| microsoft/vscode | `AGENTS.md` (pointer to `.github/copilot-instructions.md`) | `f92b8f3adcd9` | 271 | 5 |
| denoland/deno | `CLAUDE.md` | `e7b39ebd55c8` | 11,866 | 433 |
| zed-industries/zed | `AGENTS.md` | `ead5672e169f` | 12,461 | 190 |
| vercel/next.js | `AGENTS.md` | `9d7422f68b9e` | 31,539 | 538 |

Codex's `project_doc_max_bytes` default is 32 KiB (32,768 bytes); Next.js's file is 96% of it.
Biome and Next.js also keep a skill catalog under `.agents/skills/` (Biome: 12 skills; Next.js:
20) with a README that says when to add one.

## Issues and pull requests

```bash
gh issue list --repo Androlax2/agent-cockpit --state all --limit 400 \
  --json number,title,state,comments,milestone,assignees,labels,createdAt,closedAt,author > issues.json
gh pr list --repo Androlax2/agent-cockpit --state merged --limit 200 \
  --json number,title,body,additions,deletions,comments,createdAt,mergedAt,headRefName > prs.json
gh api repos/Androlax2/agent-cockpit/assignees --jq '.[].login'   # Androlax2
```

| Measure | Result |
| --- | --- |
| Issues read | 400 (#61 to #739): 204 closed, 196 open |
| Issues with no assignee | 399 of 400 |
| Issues created since 2026-11-02 | 88; all with a milestone, 77 with a label, 1 assigned |
| Comments | 432, all by `Androlax2` |
| Issues with no comment | 162 |
| Comment length | median 415 characters, p90 1,704, max 13,580; 92 under 200 |
| Comments starting with an acknowledgement phrase (`ack`, `on it`, `working on`, `starting`, `picking up`, `will do`, `thanks`, `got it`, `noted`, `looking`) and under 200 characters | 0 |
| Comments repeated within one issue | 0 (five bodies repeat across issues: cross-posted triage notes) |
| Issues titled "Research…" | 15 (13 open, 2 closed) |
| Merged PRs read | 200 (#82 to #735); 192 on a `greenfield/` branch |
| Conventional-commit title | 120 of 200; oldest 60: 20; newest 60: 57 |
| Body median / p90 | 3,726 / 6,305 characters |
| PR comments | 44 in total |
| Closing keyword in body | oldest 60: 52; newest 60: 30; every newest-60 body references an issue number |

Median open-to-merge was 0.2 hours; it is not used in the findings because auto-merge and CI
timing, not the instructions, set it.

Analysis was a short `python3 -I` script over the two JSON files: `json`, `re`, `statistics` and
`collections` only, one regular expression per row above. The conventional-title pattern was
`^\w+(\([^)]+\))?!?:` followed by one space.

## Instruction-file history

```bash
git log --since=2026-10-31 --oneline -- AGENTS.md CLAUDE.md CONTRIBUTING.md .agents
```

Nine commits: #404, #396, #355, #132, #113, #131, #517, #574, #720. Those that add or change a
workflow rule: #132, #355, #517, #720.

## Contradiction checks

```bash
grep -rn -i -E "force|rebase" AGENTS.md CLAUDE.md CONTRIBUTING.md docs/DEVELOPMENT.md .agents
grep -n -E "make |agent-build|apply-cockpit|verify-cockpit" AGENTS.md CLAUDE.md CONTRIBUTING.md \
  .agents/skills/repo-workflow/SKILL.md .agents/rules/*.md .github/pull_request_template.md
gh api 'repos/Androlax2/agent-cockpit/milestones?state=open&per_page=50' --jq '.[] | .title, .description'
```

C1: `CONTRIBUTING.md:50` against `CLAUDE.md:12`. C2: `make greenfield-ci` in
`.github/pull_request_template.md:29`, `make` targets in `CONTRIBUTING.md` and
`docs/DEVELOPMENT.md:55-66`, "shared full-test entry point" in `AGENTS.md`. C3:
`CONTRIBUTING.md#milestones` against the descriptions of milestones 21 to 23. C4:
`repo-workflow/SKILL.md:24` against the one assignable login.

## Launch-prompt coding (n=1)

The prompt for this issue holds 13 instructions. Coded by hand against the repository files and
the Torlek manual:

| Class | Count | Instructions |
| --- | ---: | --- |
| Already in `CLAUDE.md` | 3 | follow its Git authorization and Tests; one medium `/code-review`; focused tests, PR to `main`, arm auto-merge |
| Already in the Torlek manual | 2 | `--needs-input` then end the turn; keep `--doing` current |
| Task-specific | 4 | cite #687; research only; findings under `docs/research/`; list follow-ups, do not file |
| One-off workaround | 1 | do not run `torlek` owner verbs (#729) |
| Written nowhere | 3 | done only when merged; wait for CI in a blocking loop; match the research documents' form |

This is one prompt, coded by its reader; it shows what a launch prompt can carry, not how often.
