# Documentation generator proposal

Issue #102. **Fumadocs visual direction selected by Theo; implementation and dependency approval pending.** No generator, dependency, manifest or workflow has been installed or changed.

## Existing tooling

The repository has Markdown source, `make docs-ci` (markdownlint-cli2, offline lychee and typos), and no documentation-site generator configuration or Node package manifest.
The current environment has Node, but no `mdbook`, `mkdocs` or `hugo` command.
`DESIGN.md` describes approved terminal surfaces, not a website identity. Product naming is still #339.

## Smallest justified options

| Option | Search, dark mode and editing | Versioning and checks | Cost |
| --- | --- | --- | --- |
| Fumadocs (Theo's selected visual reference) | Standard documentation layout, light/dark themes, search dialog and source links | Configure static output and browser-side search; build version trees and retain source/rendered link checks | React framework, Fumadocs packages, CSS build tooling and a documentation-only manifest/lockfile |
| mdBook, standard theme (rejected by Theo) | Built-in browser search, light/dark themes and edit links | Build separate `latest/` and release-tag trees; retain existing Markdown/link/spelling checks and check rendered links | Small build toolchain, but its UI does not meet Theo's preference |
| Starlight | Built-in Pagefind search, theme switching and edit links | Same version-tree publishing requirement; same source checks plus site build | Astro/Node package manifest and dependency lockfile; more frontend tooling than this Alpha guide needs |
| Plain static generator | Small HTML output; search, dark mode and edit links would need implementation or extra tooling | Same version-tree publishing requirement; retain existing source checks | Less external tooling can mean more custom renderer/navigation/search code to maintain |

Theo rejected mdBook's UI and selected [Fumadocs](https://www.fumadocs.dev/docs) as the visual reference. Do not install mdBook or add its configuration.
Retain the published Agent Cockpit name while #339 is undecided.
The worker recommends implementing that direction with Fumadocs UI itself, rather than recreating it in another generator. This recommendation does not treat the visual preference as approval for a dependency graph or a finished layout.

## Proposed technical choice

Use Fumadocs UI and MDX with Next.js static export under `site/`, isolated from the Rust product manifests. This follows Fumadocs' documented Next.js integration and static build path. Search indexes are emitted at build time and queried in the browser; Pages must not require a Node server or search service.

The other supported path considered is Astro with React islands. Its current Fumadocs adapter is `@fumadocs/astro` 0.1.1; the worker prefers the direct Next.js integration for this first site, while keeping the published output fully static. This is a recommendation, not a measured comparison of bundles or framework performance.

Candidate direct packages, queried read-only from the npm registry on 2026-11-02:

| Purpose | Proposed exact versions |
| --- | --- |
| Docs and content | `fumadocs-core` 16.17.9, `fumadocs-ui` 16.17.9, `fumadocs-mdx` 15.4.10 |
| Framework | `next` 16.4.3, `react` 19.3.1, `react-dom` 19.3.1 |
| Local search and content schema | `@orama/core` 1.2.19, `zod` 4.6.7 |
| CSS build | `tailwindcss` 4.3.3, `@tailwindcss/postcss` 4.3.3 |
| Type checks | `typescript` 7.0.2, `@types/node` 26.6.6, `@types/react` 19.3.0, `@types/react-dom` 19.3.0, `@types/mdx` 2.0.14 |

The installed Node 26.12.1 and npm 12.2.1 satisfy the inspected framework engine requirements. The full dependency lockfile has not been resolved and no build has run. Resolve it only after approval, with exact direct pins and `npm --prefix site install --ignore-scripts --no-audit --no-fund`; do not install a global generator or run an automatic project scaffold.

## Design handoff

A Designer has been recruited for the Fumadocs layout handoff under #499. Use standard Fumadocs navigation, document body, table of contents, version selector, search and theme controls. Retain source provenance and pending-reference labels in the content. No new logo, product palette or marketing landing page is requested.
The approved FigCraft frame revision and Impeccable guidance actually used must be recorded before implementing a new layout. A preferred reference is not an approved frame. Missing design capabilities must be reported rather than silently replaced with invented UI.

## Intended implementation after approval

- Reuse canonical repository Markdown for the user guide, quickstart, design reports and decision records. Generate a staging tree rather than maintaining duplicate copies.
- Derive the CLI reference from the built `cockpit` help and effective configuration output from a binary run with an isolated empty home, no daemon, and a clean environment. Label effective defaults as defaults, not a complete schema reference. #295's protocol schema remains explicitly planned until implemented.
- Render the current shipped agent instruction bundle from its source. Add search and source-specific edit links; generated references link to their owning source and state their revision.
- Keep `make docs-ci` on canonical source; add a deterministic site-build entry point and rendered local-link checks, including project subpath and release-version URLs.
- Build on PRs without deploying. Deploy `latest/` from `main` and immutable versioned copies only for released tags matching the release lane's tag grammar. Coordinate preservation of older version outputs and release-event wiring with the release lane; do not edit its workflows or `docs/RELEASING.md` concurrently.
- Pages settings/deployment, workflow pushes and the repository website field use reviewed `ask-to-run` host approval. No custom domain, tag or release creation.
- Keep install/session acceptance open until the merged public installer is used successfully by a reader following the site alone. #98's pending volunteer result is not site acceptance.

Approve the proposed documentation-only dependencies, manifest and lockfile before package resolution or installation. The dependency install stays in this sandbox checkout; workflow pushes, Pages settings and deployments retain their separate reviewed host approval.

## Sources

- [mdBook HTML renderer configuration](https://rust-lang.github.io/mdBook/format/configuration/renderers.html): search, themes, edit links and site subpath.
- [Starlight configuration](https://starlight.astro.build/reference/configuration/): search and edit links.
- [Fumadocs Next.js integration](https://www.fumadocs.dev/docs/manual-installation/next), [static build and search](https://www.fumadocs.dev/docs/deploying/static), and [Astro integration](https://www.fumadocs.dev/docs/manual-installation/astro).
- Package versions and engine/peer declarations were read from the public [npm registry](https://registry.npmjs.org/), without executing package code.
- Repository ownership and release policy: [Releasing](../RELEASING.md), [user README acceptance](../acceptance/98-user-readme.md).
