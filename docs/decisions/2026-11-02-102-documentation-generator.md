# Documentation generator proposal

Issue #102. **Proposed by the documentation worker; awaiting Theo's decision.** No generator, dependency, manifest or workflow has been installed or changed.

## Existing tooling

The repository has Markdown source, `make docs-ci` (markdownlint-cli2, offline lychee and typos), and no documentation-site generator configuration or Node package manifest.
The current environment has Node, but no `mdbook`, `mkdocs` or `hugo` command.
`DESIGN.md` describes approved terminal surfaces, not a website identity. Product naming is still #339.

## Smallest justified options

| Option | Search, dark mode and editing | Versioning and checks | Cost |
| --- | --- | --- | --- |
| mdBook, standard theme (recommended) | Built-in browser search, light/dark themes and edit links | Build separate `latest/` and release-tag trees; retain existing Markdown/link/spelling checks and check rendered links | One pinned documentation build tool and `book.toml`; no product Cargo dependency or Node application |
| Starlight | Built-in Pagefind search, theme switching and edit links | Same version-tree publishing requirement; same source checks plus site build | Astro/Node package manifest and dependency lockfile; more frontend tooling than this Alpha guide needs |
| Plain static generator | Small HTML output; search, dark mode and edit links would need implementation or extra tooling | Same version-tree publishing requirement; retain existing source checks | Less external tooling can mean more custom renderer/navigation/search code to maintain |

The recommendation is mdBook's standard theme and the published Agent Cockpit name, without a new logo, palette or branded landing page.
Any custom visual design requires the approved Designer/FigCraft/Impeccable workflow separately; this proposal does not authorize one.

## Intended implementation after approval

- Reuse canonical repository Markdown for the user guide, quickstart, design reports and decision records. Generate a staging tree rather than maintaining duplicate copies.
- Derive the CLI reference from the built `cockpit` help and effective configuration output from a binary run with an isolated empty home, no daemon, and a clean environment. Label effective defaults as defaults, not a complete schema reference. #295's protocol schema remains explicitly planned until implemented.
- Render the current shipped agent instruction bundle from its source. Add search and source-specific edit links; generated references link to their owning source and state their revision.
- Keep `make docs-ci` on canonical source; add a deterministic site-build entry point and rendered local-link checks, including project subpath and release-version URLs.
- Build on PRs without deploying. Deploy `latest/` from `main` and immutable versioned copies only for released tags matching the release lane's tag grammar. Coordinate preservation of older version outputs and release-event wiring with the release lane; do not edit its workflows or `docs/RELEASING.md` concurrently.
- Pages settings/deployment, workflow pushes and the repository website field use reviewed `ask-to-run` host approval. No custom domain, tag or release creation.
- Keep install/session acceptance open until the merged public installer is used successfully by a reader following the site alone. #98's pending volunteer result is not site acceptance.

Approve the generator and documentation-only configuration first. The specific pinned tool install command and any workflow push will be prepared for host review separately, with usage and effects stated.

## Sources

- [mdBook HTML renderer configuration](https://rust-lang.github.io/mdBook/format/configuration/renderers.html): search, themes, edit links and site subpath.
- [Starlight configuration](https://starlight.astro.build/reference/configuration/): search and edit links.
- Repository ownership and release policy: [Releasing](../RELEASING.md), [user README acceptance](../acceptance/98-user-readme.md).
