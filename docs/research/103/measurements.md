# Measurements behind the #103 findings

Appendix to [the findings](../2026-11-07-103-team-roles-and-worker-efficiency.md). Taken 2026-11-07 at
[`0b6ec2d0`](https://github.com/Androlax2/agent-cockpit/tree/0b6ec2d0). Counts and bytes are
*measured*. No token count appears: none was available, and the findings say so.

## Context sizes

```bash
torlek brief --measure
wc -c CLAUDE.md AGENTS.md .agents/skills/coordinator/SKILL.md .agents/skills/coordinator/prompts/*
```

| Item | Bytes |
| --- | ---: |
| Launch brief | 1,364 |
| Skill catalog entry | 276 |
| Whole manual, on demand | 12,569 |
| `CLAUDE.md` | 1,116 |
| `AGENTS.md` | 5,722 |
| Coordinator skill | 12,386 |
| Worker start suffix | 2,000 |
| Decision-issue start prompt | 1,223 |

`torlek brief --measure` counts what Torlek adds, not the harness's own context, which is why the
brief is 246 bytes larger than the 1,118 that [#619](../2026-11-04-619-agent-token-efficiency.md)
measured at `fb683e93`.

## Worker trials log

Source: `.agents/skills/coordinator/trials-log.md`, 34 rows (kind: contained 15, risky 10,
research 8, docs 1). The log has no token column.

| Kind | Rows with a known CI count | With at least one CI fix round |
| --- | ---: | ---: |
| risky | 9 | 5 |
| contained | 12 | 4 |
| research | 8 | 1 |
| docs | 1 | 0 |
| all | 30 | 10 |

Causes of the ten, from the log's notes: flaky tests, main drift or the disk floor (#800, #766, #825,
and #406 parts 3 and 5); lint or clippy (#136, #845, #860); a PR opened before its review
(#754); not stated (#713). Review passes: 33 rows one, one row two (#767). Stalls without a question
that needed a nudge: #777, #766, #406 part 4.

## Merged worker pull requests

```bash
gh pr list --state merged --limit 100 --json number,headRefName,additions,deletions,createdAt,mergedAt
gh api repos/Androlax2/agent-cockpit/pulls/N --jq '[.number,.commits,.review_comments,.comments]|@tsv'
```

The first call returned 98 merged PRs on `greenfield/` branches. `commits` was fetched for the
60 newest (`#981` to `#1117`, created from 2026-11-06 16:05Z, merged up to 2026-11-07 15:13Z);
the one query asking for commits and PRs together exceeds GitHub's node limit.

| Measure | Value |
| --- | --- |
| Open to merge, median / 90th percentile / longest | 0.2 h / 0.54 h / 2.4 h |
| Changed lines (additions + deletions), median / 90th percentile | 339 / 1,585 |
| Commits per PR, median / mean / most | 2 / 2.3 / 7 |
| PRs with one commit / with three or more | 12 / 21 |
| Review comments / conversation comments, all 60 | 0 / 5 |

Commits mix review fixes, CI fixes and follow-ups, so they are a rework proxy, not a count of
rework. Reviews run in the worker's session, which is why no PR holds one.

## External sources

Fetched 2026-11-07 with a tool that returns a model's summary of the page, not the page. Quotes in
the findings are as that tool returned them and were not checked against the full text.

| Source | Date on the page |
| --- | --- |
| Anthropic Engineering, "How we built our multi-agent research system" | 2025-06-13 |
| Claude Code documentation: sub-agents, agent teams, costs | undated, read 2026-11-07 |
| OpenAI Codex documentation: subagents | undated, read 2026-11-07; redirects to `learn.chatgpt.com` |
| Cognition, "Don't Build Multi-Agents" | 2025-06-12 |
| arXiv 2503.13657, Cemri et al. | submitted 2025-03-17, v3 2025-10-26 |
| arXiv 2512.08296, Kim et al. | submitted 2025-12-09, v3 2026-04-08 |
| arXiv 2308.00352, Hong et al. (MetaGPT) | submitted 2023-08-01, v7 2024-11-01 |

A search snippet and two third-party posts gave `agents.max_depth` and `agents.max_threads`
defaults for Codex. The official page I then fetched did not document them, so the findings omit
them.
