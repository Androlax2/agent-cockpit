# Known gaps

This is an alpha. Data may be reset between alpha releases. Each gap links the open issue that tracks it.

- **Linux and WSL2 only, mostly from the release archive.** There are no macOS builds (#568). The Arch package is prepared but not published yet (#562). APT serves Ubuntu 26.04 and Debian 13 only; Ubuntu 24.04, WSL2 and Debian 12 use the archive. The Scoop route for WSL 2 is checked by CI only (#565).
- **An unsandboxed agent has your rights.** It can act as you, so Torlek's grants do not bind it. `sandbox = "off"` under `[agents]` in the owner configuration records that you accept this (#833).
- **The native sandbox is off by default.** Turn it on with `sandbox = "required"` under `[agents]` in the owner configuration (the path `torlek config show` prints). Where no sandbox backend exists, every launch is then refused (#833).
- **A sandboxed agent signs in with its own credential, not yours.** Save one per provider, as the user guide's sandbox section says, or its sessions ask for a sign-in they cannot keep (#1251).
