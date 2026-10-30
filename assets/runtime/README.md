# Runtime desktop defaults

These are byte-preserved snapshots of Theo's existing Cockpit CSS and seven OSD SVG icons,
approved for package preparation. User desktop overrides remain outside the package.
The generated CSS comments identify their original dotfiles palette templates; the palette
generator is not a runtime dependency and the package does not modify those templates.

The two CSS files are included under the project's Apache-2.0 license with Theo's explicit
ownership/redistribution approval. Font family names are preferences with sans-serif fallback;
no font binaries are bundled or licensed by this repository.

The icons retain Lucide's ISC and Feather's MIT notices, not the project's Apache license.
They include existing size, color, stroke, geometry serialization and notification-dot changes.
They are not asserted to be byte-identical to an upstream SVG release.

Upstream evidence and notices:

- [Lucide license at fcebb5c](https://github.com/lucide-icons/lucide/blob/fcebb5c4dbd7ab6c26943a2aa009cf57eddf8893/LICENSE), copied as `LICENSE-LUCIDE`.
- [Historic volume geometry at 944e428](https://github.com/lucide-icons/lucide/blob/944e4286304c049ade0adaba8174253f512cff35/icons/volume-2.svg): the polygon vertices and volume arcs match the existing optimized paths.
- [Lucide bell geometry](https://github.com/lucide-icons/lucide/blob/fcebb5c4dbd7ab6c26943a2aa009cf57eddf8893/icons/bell.svg) and [sun geometry](https://github.com/lucide-icons/lucide/blob/fcebb5c4dbd7ab6c26943a2aa009cf57eddf8893/icons/sun.svg) match the existing vector elements.
- [Feather license at 1b00239](https://github.com/feathericons/feather/blob/1b002399e8758fb2bccea4d07312dc9e0f43dcb8/LICENSE), copied as `LICENSE-FEATHER`.

Release assembly must include these notices alongside the component and compiled-dependency
notices. This directory is package input, not permission to overwrite a user's desktop files.
