# Known gaps

This is an alpha. Data may be reset between alpha releases. Each gap links the open issue that tracks it.

- **Linux and WSL2 only, from the release archive.** There are no macOS builds (#568). Arch, Debian/Ubuntu and Scoop packages are prepared but not published yet (#562, #563, #565).
- **The native sandbox is off by default.** Turn it on with `sandbox = "required"` under `[agents]` in the owner configuration (the path `torlek config show` prints). Where no sandbox backend exists, every launch is then refused (#833).
