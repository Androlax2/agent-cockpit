# Documentation layout proposal

Issue #102. **Proposed; awaiting Theo's review of these exact frames.**
The current proposal is the separate [r4 handoff](102-documentation-layout-r4.md); this file retains the r3 history and verification evidence.
Theo's preference for Fumadocs establishes the visual direction. It does not approve
these layouts, implementation, deployment or a new product identity. Theo separately approved
the exact site-only dependencies and build validation in decision commit `fd9194c`.

## Review artifact

[FigCraft proposal](../../design-plans/documentation-site.figcraft), saved version
`102 documentation proposal r3`, version ID `1a199ed781f`.
Artifact SHA-256:
`b0c22549fbddf9592a1fa3adeb7fa25f232f42ccb6c0f1b5b393610eee03e17e`.

| Frame ID | Proposed state | Size | Preview |
| --- | --- | --- | --- |
| `11` | Desktop quickstart | 1440 × 1200 | [Desktop](../../design-plans/previews/102-d1.png) |
| `83` | Mobile reading | 390 × 1460 | [Mobile reading](../../design-plans/previews/102-m1.png) |
| `108` | Mobile navigation | 390 × 1040 | [Mobile navigation](../../design-plans/previews/102-m2.png) |
| `153` | Search results | 640 × 520 | [Results](../../design-plans/previews/102-s1.png) |
| `168` | Search with no results | 640 × 520 | [Empty](../../design-plans/previews/102-s2.png) |
| `178` | Search index unavailable | 640 × 520 | [Unavailable](../../design-plans/previews/102-s3.png) |

These are one compact proposal, with component-reference and notes pages. There is
no approved wireframe or visual stage; every frame remains unapproved and
`inspect.readyForDev` returns an empty list. Existing product boards are unchanged.

## Layout and source content

Use the standard Fumadocs documentation shell: sidebar navigation, a reading
column, and desktop contents. Mobile uses its normal header, navigation and
contents disclosure. Keep Agent Cockpit as the name while #339 is undecided.
Use upstream light/dark theme roles and ordinary sans-serif article typography;
reserve monospace for commands and configuration. The terminal-specific
`DESIGN.md` does not supply website tokens.

The drawings use excerpts from [README.md](../../README.md): install status,
prerequisites and the first project/session instructions. Preserve the warning
that public installation is still being prepared. The previews abbreviate the
article to show layout; the published page must retain the complete canonical
instructions, including project registration and session launch. Displayed
commands were not executed.

Use the sidebar groups shown in the desktop frame: Get started, User guide,
Reference, and Project. Keep the current page visible in the navigation. Clearly
label the protocol schema as planned until #295 implements it. Generated CLI and
configuration pages must identify their revision and owning source; effective
defaults are not a complete configuration schema.

`Latest (main)` distinguishes current development documentation from release
tags. Search uses the current version's local index. Version switching, source
edit links and onward links must resolve under the project subpath and selected
version. Generated pages link to their owning source rather than generated output.

## Component mapping

| Drawing role | Intended implementation |
| --- | --- |
| Sidebar, mobile navigation and header | Standard `DocsLayout` and its page tree |
| Title, article, contents and page footer | Standard `DocsPage` composition |
| Version selector | Standard navigation control wired to available version paths |
| Search input and result rows | Official local-search integration and standard search UI |
| Code, callout and reference tables | Existing Fumadocs Markdown components |
| Theme control | Standard Fumadocs theme switch |

The board's components are reference diagrams of upstream roles, not a new custom
component library. Use upstream controls, icons and theme values in the actual
site. The preview uses already installed DejaVu Sans and DejaVu Sans Mono because
Inter was unavailable; this is not a new website font decision. Do not recreate
preview control shapes or copy spacing by eye when the upstream component already
provides them. The framework choice belongs to the
[generator proposal](../decisions/2026-11-02-102-documentation-generator.md).

## Author preview review and remaining verification

- The article starts with the reader's task and visible installation limits;
  source truth takes precedence over attractive onboarding claims.
- Sidebar groups and current-page emphasis provide ordinary documentation
  wayfinding. The mobile navigation's duplicate product heading was removed
  during the manual preview review.
- Code and configuration remain readable in the inspected 390 px frame. Verify
  320 px layout, long commands, wide reference tables and 200% zoom in the actual
  browser; these static drawings do not prove those behaviors.
- Search results, no matches and index failure are distinct. Failure recovery
  copy is proposed intent: verify how the official adapter exposes failure before
  implementation. A search failure must leave ordinary navigation usable.
- Verify keyboard opening/closing and focus restoration, arrow-key result
  selection, accessible control names, theme contrast and touch target sizes in
  rendered upstream components. Focus outlines in drawings are not browser tests.

During the original proposal preparation, all six exported PNG previews were opened and inspected. No independent design critique,
automated accessibility audit, browser interaction check or performance
measurement has run. Dark-theme values exist in the reference variables, but a
dark browser rendering has not been reviewed. Exact-frame approval remains open.

## Verified Impeccable use for revision 1a199ed781f

**No Impeccable command, automated Impeccable check or independent critique ran for this revision.** The Designer manually read and applied portions of upstream guidance. Reading the critique and audit references did not execute either workflow. There is no Impeccable report, score, detector output or independent reviewer verdict.

After Theo requested verification, the original Designer checked the original session transcript and embedded FigCraft snapshots. The parent worker independently checked the original retrieval calls, verified the archive hash/version index and confirmed the upstream commit through GitHub's API. This is retrospective evidence verification, not a new design review or a new Impeccable run.

### Upstream revision and original reads

Repository: `pbakaus/impeccable`, exact revision
`e89e3ada3e875a89ac2c5d33d11e65ebef6fe285`:
[pinned upstream guidance](https://github.com/pbakaus/impeccable/tree/e89e3ada3e875a89ac2c5d33d11e65ebef6fe285/.agents/skills/impeccable).
All paths in this table are relative to `.agents/skills/impeccable/`.

| Source file | Original retrieval and visibility | Original transcript line |
| --- | --- | --- |
| `SKILL.md` | Direct fetch; displayed guidance | 56 |
| `reference/mode-read.md` | Direct fetch; displayed in full | 66 |
| `reference/new-work.md` | Direct fetch in the same call; combined output was truncated, so only partially visible | 66 |
| `reference/critique.md` | Direct fetch piped through `head -105` | 78 |
| `reference/audit.md` | Direct fetch piped through `head -85` | 78 |
| `reference/craft-floor.md` | Direct fetch piped through `head -115`, before artifact generation; not merely a transitive reference | 137 |

The retrieval command pattern was the following, substituting each exact source path above for `PATH`:

```sh
gh api repos/pbakaus/impeccable/contents/PATH?ref=e89e3ada3e875a89ac2c5d33d11e65ebef6fe285 --jq .content | base64 -d
```

The `head` limits in the table were applied after decoding. No Impeccable skill command/workflow, executable, engine, detector, hook or live mode was invoked or installed. In particular, there was no executed Impeccable critique, audit or polish action. No claim of a full read of the partially displayed references is made.

### Manual application and concrete changes

The guidance informed familiar navigation, a restrained reading column, real source content, respecting Theo's selected Fumadocs aesthetic, and separate search-results, empty and failure states. These are author design choices, not findings emitted by Impeccable automation. The component appearance comes from the chosen Fumadocs reference; it is not a new Impeccable-generated product identity.

| Process that found the issue | Finding | Resulting change and evidence |
| --- | --- | --- |
| Designer's manual inspection of initial FigCraft previews, followed by installed-font discovery | Initial exports had no visible text; Inter was unavailable | r1 to r2 used already installed DejaVu preview fonts and regenerated the images; embedded snapshots record font/style and layout changes |
| Designer's manual inspection of the mobile navigation preview | Product name appeared twice | r2 to r3 hid node `113`, removing the duplicate heading; snapshots also record seven resulting transform changes |
| Reading the repository README | Public installation remained incomplete | Proposal copy retained that limitation and did not claim successful public onboarding |

Those original author checks were not an independent critique. Reading `critique.md` or `audit.md` did not produce a formal finding list. Keyboard behavior, browser accessibility, dark rendering, performance and search behavior remain unverified.

### Evidence locations and verification boundaries

- Original Designer transcript, retained locally: `/home/theo/.codex/sessions/2026/11/02/rollout-2026-11-02T02-34-28-01a199e4-c0cb-7511-b1be-16fdbe112925.jsonl`. Original retrieval calls: lines 56, 66, 78 and 137. FigCraft generation/saved revisions: lines 176, 199 and 232. Preview inspections and final artifact checks: lines 183, 204 and 239. The full transcript is not copied into this repository.
- The committed [FigCraft archive](../../design-plans/documentation-site.figcraft) contains `versions/index.json` and snapshots `versions/1a199ebf63e.json` (r1), `versions/1a199ec6e4b.json` (r2) and `versions/1a199ed781f.json` (r3). The Designer verified that the current saved document matches r3. Its SHA-256 remains the value at the top of this handoff.
- The six committed PNG previews are linked in the frame table above. The Designer opened all six; the parent also inspected the desktop and mobile-reading previews.
- Installed `figcraft-cli` generated, saved and rendered the proposal headlessly. Archive/JSON validity, PNG dimensions, local-link existence and empty `readyForDev` checks were artifact verification. They were not Impeccable automated checks, browser tests or independent design review.

The Designer's inspected discovery paths did not expose a local Impeccable skill or a connected FigCraft MCP tool. Reading pinned upstream guidance granted no execution authority. No dependency, host action, product-code change or Git mutation was performed by the Designer; the parent subsequently committed the proposal artifacts and separately performed the approved dependency/build validation.

Review these frames before implementation. Approval should identify the saved
version above; only approved frames may subsequently be marked `readyForDev`.

## Fresh installed Impeccable critique

After Theo installed Impeccable and explicitly authorized a new run, the parent invoked the installed engine and followed the critique workflow with two fresh isolated assessments. The [persisted critique snapshot](../../.impeccable/critique/2026-11-02T02-18-33Z__design-plans-documentation-site-figcraft.md) contains the findings and static-proposal score. The [execution evidence](evidence/102-impeccable-run-2026-11-02.json) records engine/skill versions, exact commands, outcomes, raw context and detector output, target hash and coverage limits.

The installed engine ran (`impeccable-engine 0.1.18`; skill 4.8.0). Active skill/critique guidance matches upstream `e89e3ada3e875a89ac2c5d33d11e65ebef6fe285`; this is not a claim that every installed file matches that revision. The scoped context command succeeded. The actual detector command exited 1 with an `ENOENT` read error for the exact FigCraft path despite a subsequent file listing confirming it exists. One target was requested; none was successfully scanned. Stdout `[]` is not a clean result and the cause is not established.

The independent visual assessment inspected all six PNG previews and found two P2 concerns: the broad desktop prose measure and absence of a visible pointer/touch dismissal control or mobile example in the search drawings. Its Nielsen assessment is 26/36 for static design intent, with interaction efficiency unscored. That score is not a production audit or shipping approval. No design changes were applied, no frames were approved or marked ready for development, and the FigCraft archive hash is unchanged.

No product browser route was available; no overlay, keyboard/accessibility check, dark browser rendering or performance measurement ran. The dependency probe was not substituted for the product. This fresh review is separate from the original revision's manual-guidance-only history above and from the final independent full-delta PR review, which remains pending.
