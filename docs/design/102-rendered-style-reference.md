# Rendered Fumadocs style reference

Issue #102. These images capture the installed Fumadocs styles in an isolated fixture for the proposed documentation design. They do not show the product site or establish layout approval.

- [Light sample](../../design-plans/previews/102-style-reference-light.png)
- [Dark sample](../../design-plans/previews/102-style-reference-dark.png)
- [Computed styles and execution evidence](evidence/102-fumadocs-rendered-styles.json)

## Capture and scope

A temporary copy of the existing Astro build-validation fixture rendered real TOML and shell examples with the approved Fumadocs packages. Its page was shown in headless Chromium through the already installed Python Playwright environment. A temporary 70ch body container, margin and padding kept the isolated sample readable; those wrapper values are not a new product layout or design token.

The light/dark class switch and code-token colours came from the installed CSS and rendered DOM. The neutral scheme is intentionally restrained. The source files are `fumadocs-ui/css/neutral.css`, `css/lib/default-colors.css` and `css/lib/shiki.css`; package versions and computed values are recorded in the evidence file. No font or dependency was installed.

The style build exited 0. Existing CSS optimization emitted the same two warnings about `::highlight(fd-search)` already recorded in the dependency validation. Capture exited 0; both images were opened and inspected. The server helper stopped the sole local fixture server afterward. No user desktop or provider session was captured.

A diagnostic `impeccable detect <temporary fixture>/dist/index.html --json` exited 0 with stdout `[]` and no stderr. This result applies to that HTML style fixture. It does not retroactively validate the FigCraft artifact, and it does not establish product browser, accessibility or interaction acceptance.

## Desktop viewing

At Theo's explicit request, host-approved request `cf8a6d7eaa2d` ran the existing checkout-isolated FigCraft launcher. It started the app on workspace 7 and the verified control endpoint acknowledged opening `design-plans/documentation-site.figcraft` (exit 0). Other windows and documents were left open; no session restart was requested.

The next design revision uses a separate board so headless edits cannot overwrite a document Theo is viewing or editing. Opening a proposed board is not frame approval. Workflow pushes, Pages settings and deployment retain their separate host review.
