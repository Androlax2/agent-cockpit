# Documentation layout r4

**Proposed; exact-frame approval remains open.** This separate
[r4 board](../../design-plans/documentation-site-r4.figcraft) preserves the original
r3 file, historical evidence and the selected Fumadocs identity. No frame is
marked `readyForDev` or completed. Product naming remains unchanged pending #339.

Saved version: `102 documentation proposal r4`, ID `1a19a1c1456`.
Artifact SHA-256:
`f6e3a8580caaf7c77432d89273af3daaf1648f88995cf125a44d1057aa2d4a3d`.

| Frame | State | Preview |
| --- | --- | --- |
| `11` | Desktop light, 1280 × 1280 | [Light](../../design-plans/previews/102-r4-desktop-light.png) |
| `211` | Desktop dark, 1280 × 1280 | [Dark](../../design-plans/previews/102-r4-desktop-dark.png) |
| `83` | Mobile reading light, 390 × 1460 | [Light](../../design-plans/previews/102-r4-mobile-light.png) |
| `283` | Mobile reading dark, 390 × 1460 | [Dark](../../design-plans/previews/102-r4-mobile-dark.png) |
| `108` | Mobile navigation light, 390 × 1040 | [Navigation](../../design-plans/previews/102-r4-mobile-navigation.png) |
| `153` | Desktop search results | [Results](../../design-plans/previews/102-r4-search-results.png) |
| `168` | Desktop search empty | [Empty](../../design-plans/previews/102-r4-search-empty.png) |
| `178` | Desktop search unavailable | [Unavailable](../../design-plans/previews/102-r4-search-unavailable.png) |
| `308` | Mobile search light, 390 × 700 | [Light](../../design-plans/previews/102-r4-mobile-search-light.png) |
| `325` | Mobile search dark, 390 × 700 | [Dark](../../design-plans/previews/102-r4-mobile-search-dark.png) |

## Changes and findings

The existing [r3 critique](../../.impeccable/critique/2026-11-02T02-18-33Z__design-plans-documentation-site-figcraft.md)
identified two P2 concerns. Desktop prose is now 672 px instead of 832 px: about
65.96ch using the already installed 16 px DejaVu Sans zero-glyph width of 10.1875 px.
This is a font-relative measure, not a claim that every line has 66 characters.
Use an equivalent readable prose limit in the actual upstream layout; wide code
and tables may scroll independently when needed.

All drawn search states now include an explicit 80 × 44 reference Close button,
and mobile search is shown in both themes. This maps to installed upstream
`SearchDialogClose`, a button with `aria-label="Close Search"` and
`onClick: () => onOpenChange(false)`. Its default child is `ESC`; use its existing
children/className props to display a clear Close label and suitable touch target.
The drawings specify intent; they do not prove pointer, touch or keyboard behavior.

## Style evidence

These are **headless FigCraft approximations**, not screenshots of rendered
Fumadocs. The separate [rendered style reference](102-rendered-style-reference.md)
contains actual installed-stack captures and computed values; its scope is an
isolated style fixture rather than the documentation product layout.

The installed `fumadocs-ui` version is `16.17.9`. Source paths are relative to
`site/node_modules/fumadocs-ui/`:

- `css/style.css` imports `css/neutral.css`, which imports
  `css/lib/default-colors.css`. These establish the light/dark palette below;
  8-bit drawing colors round the source HSL values and retain alpha.
- `dist/layouts/docs/slots/sidebar.js` uses `bg-fd-card` and active
  `bg-fd-primary/10`; the dark sidebar muted-text override is in `neutral.css`.
- `dist/components/codeblock.js` and `dist/components/callout.js` supply the
  card background, border and rounded-xl treatment represented in the frames.
- `dist/components/dialog/search.js` supplies the popover colors and actual
  `SearchDialogClose` behavior; `search-default.js` includes that close component.

| Role | Light | Dark |
| --- | --- | --- |
| Background / foreground | `#f5f5f5` / `#0a0a0a` | `#121212` / `#ebebeb` |
| Card / sidebar | `#f1f1f1` | `#191919` |
| Muted text | `#737373` | `#b3b3b3cc` |
| Popover / foreground | `#fafafa` / `#272727` | `#1e1e1e` / `#dedede` |
| Border | `#cccccc80` | `#66666633` |

`site/node_modules/fumadocs-core/dist/shiki-DL5DIaX-.js` declares the default
`github-light` / `github-dark` pair. The installed `shiki.codeToTokens` was run
locally on the displayed shell/TOML excerpts, without writing product files.
Command names and the TOML section use `#6f42c1` / `#b392f0`; arguments and strings
use `#032f62` / `#9ecbff`; ordinary code uses `#24292e` / `#e1e4e8`.
These also agree with the parent's
[computed fixture evidence](evidence/102-fumadocs-rendered-styles.json).
They are syntax colors, not new brand accents or terminal-palette tokens.

## Scope, guidance and verification limits

The component mapping and source-content rules remain those of the
[original proposal](102-documentation-layout.md): standard `DocsLayout`,
`DocsPage`, Markdown components and local search. Preserve the current public
installation limitation, `Latest (main)`, planned protocol label and source edit
links. Preview excerpts do not replace the complete canonical quickstart in the
eventual site. DejaVu Sans/Mono are already installed preview fonts; they do not
establish the website's actual font stack.

For this refinement, the Designer directly read installed Impeccable 4.8.0
`SKILL.md`, `reference/craft-floor.md` and `reference/mode-read.md` from
`/home/theo/.agents/skills/impeccable/`. The existing critique records these three
as matching upstream revision `e89e3ada3e875a89ac2c5d33d11e65ebef6fe285`; this does
not assert that the whole installed distribution matches that commit. Root's
context load had already run and was not repeated. Guidance informed the scoped
refinement; the two P2 findings came from the prior critique.

All ten final PNGs were opened and inspected. Headless FigCraft generation,
native archive/JSON integrity, frame sizes and absence of ready/completed flags
are artifact checks. No Impeccable executable, detector, independent re-review or
browser interaction audit ran in this Designer continuation. The parent's
separate rendered fixture/detector evidence does not constitute a pass for r4 or
the future site. Remaining checks include actual dismissal/focus restoration,
keyboard search, 320 px layout, 200% zoom, long content and contrast in rendered
components. No deployment, package install, live desktop operation, Git mutation
or product implementation was performed by this Designer continuation.
