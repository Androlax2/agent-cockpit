# Documentation layout proposal

Issue #102. **Proposed; awaiting Theo's review of these exact frames.**
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

## Manual critique and remaining verification

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

All six exported PNG previews were opened and inspected. No independent design critique,
automated accessibility audit, browser interaction check or performance
measurement has run. Dark-theme values exist in the reference variables, but a
dark browser rendering has not been reviewed. Exact-frame approval remains open.

## Impeccable provenance and tooling

Guidance was read manually from upstream revision
`e89e3ada3e875a89ac2c5d33d11e65ebef6fe285`:
[Impeccable](https://github.com/pbakaus/impeccable/tree/e89e3ada3e875a89ac2c5d33d11e65ebef6fe285/.agents/skills/impeccable).
References read: `SKILL.md`, `mode-read.md`, `new-work.md`, `craft-floor.md`,
`critique.md` and `audit.md`. Applicable guidance: honor the chosen visual
direction, use familiar reading navigation, maintain a calm text column, use real
content, inspect responsive states and distinguish empty from failed search.

No discoverable local Impeccable skill or connected FigCraft MCP tool was found in
this session. No Impeccable engine, detector, hooks or automated command workflow
was installed or executed. Reading critique/audit instructions is not a claim that
those workflows ran. The upstream guidance does not broaden this lane's authority.

The already installed `figcraft-cli` created the board headlessly through JSON
commands, saved its native version history, and exported the PNG previews. Font-free
initial exports lacked visible text; the final previews were regenerated using
already installed fonts and inspected. No new tool, dependency, live desktop
operation, product code change, Git commit or push was performed for this proposal.

Review these frames before implementation. Approval should identify the saved
version above; only approved frames may subsequently be marked `readyForDev`.
