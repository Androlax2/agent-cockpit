# Documentation site code review

Issue #102 / PR #507. No simplify workflow ran.

## Invoked Codex workflow

The parent invoked **`$code-review`**, default **medium**, by loading
`/home/theo/.codex/skills/code-review/SKILL.md` and executing its Angles recipe.
Skill SHA-256: `dd5bb20c63d55b3dac6f3b8b4295456c8af9bcc271b18081577176d84b419b35`.
This is the installed Codex port of the workflow, not a slash-command execution
or a substituted fresh-agent review.

Reviewed scope: the PR branch delta from merge base
`6f827521e16a23bca33261bd53779de83f5fd8ed` to HEAD
`bf73b73693ae78525b8791421c3267df9d1f730b`, plus its current working changes and
new lane files. The 86-file source snapshot digest, computed from sorted paths
and SHA-256 content hashes, was
`3b9efed6737b490592f8b64ac4951a3df249ccf88f0f454f0b4b81f691c79f47`.
Unrelated untracked local configuration was excluded.

All eight angles ran inline in the parent's context: line-by-line correctness,
removed behavior, cross-file callers/callees, reuse, simplification, efficiency,
altitude and applicable conventions. Candidates were pooled and deduplicated.
No verifier agent, automated test or fix ran during that review. It was a
**self-review**, and does not count as the required independent final review.

The earlier statement that the workflow was unavailable was incorrect. Searching
the actual Codex/shared skill directories found it outside the advertised list.
The original independent design critiques and #98 review remain their own evidence;
none is relabeled as this code-review invocation.

## Findings and handling

Line numbers refer to the reviewed snapshot.

| File and line | Finding | Handling |
| --- | --- | --- |
| `docs/acceptance/102-documentation-site.md:15` | Two linked evidence files were not created yet | Persist browser and Impeccable evidence before the final documentation gate |
| `site/scripts/content.mjs:36` | Fence-like content with trailing text ended protection early | Require a whitespace-only closing suffix (including CRLF); regression failed before the fix and passes afterward |
| `site/scripts/content.mjs:72` | Fragment-only README links bypassed split-section mapping | Map known source anchors and retain ordinary local fragments; regression failed before the fix and passes afterward |
| `site/scripts/content.mjs:84` | Raw-image query followed its fragment | Put the query before the fragment; regression failed before the fix and passes afterward |
| `site/README.md:95` | Incorrect unavailable-workflow statement | Replaced with actual installed workflow and this record |
| `site/src/pages/[...slug].astro:10` | Repeated complete-tree serialization | Export the serialized navigation once from the source module |

After the review, all seven focused content tests passed with zero skips.
The final site gate passed 13 Node tests, both type checks and rendered links.
The staged documentation gate passed after correcting two manual-source H1 headings.
The independent full-delta review is recorded below. No approval, merge, workflow push or deployment is inferred.

## Fresh independent native workflow

Reviewer `/root/native_review_102` independently loaded the same installed
`$code-review` skill and ran its medium Angles recipe in its own context. All
eight angles completed inline, without finder/verifier agents, verification,
fixes, posting, installs or Git/host actions. The reviewer froze candidates before
reading the parent's self-review record. No generic fallback was substituted.

Reviewed committed range:
`6f827521e16a23bca33261bd53779de83f5fd8ed` through
`0e2308fc055072d74ac90ddabfb1be282b0140f0`.
The independent result had two findings and no P0/P1 identified:

| Reviewed location | Independent finding | Fix |
| --- | --- | --- |
| `site/src/components/Search.tsx:52` | P2: custom no-result content dropped upstream `role="status"` | Restore the status role; browser regression checks its presence. This is not a claim of physical screen-reader testing |
| `.github/workflows/docs.yml:106` | P3: released-version check commands bypassed Make ownership | Both latest and released versions now call `make site-ci`; the same target passes at the fixture prerelease prefix |

The raw independent JSON and scope are in
[independent review evidence](../design/evidence/102-code-review-independent.json).
These are small review fixes; no second independent review was run. Final local
checks and browser results are recorded in the linked acceptance evidence. Host
workflow push, CI and Pages publication retain their separate gates.
