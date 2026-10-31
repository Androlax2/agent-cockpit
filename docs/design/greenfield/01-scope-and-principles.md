# 01. Scope and principles

Status: decided

## Decision

- Cockpit is a **personal installation**: one human owner, one daemon per OS user,
  any number of clients (TUI, CLI, phone/web). No shared multi-human control.
- Teams are teams of **agents**, working inside the owner's grants and budgets.
- Target platforms: **Linux and macOS native; Windows through WSL2 only**, accessed from
  Windows Terminal. No native Windows daemon or desktop app.
- **Terminal-first**: the TUI is the interactive cockpit; CLI verbs serve agents and
  automation; every feature exists as a typed command before it has a screen.
- Designed **greenfield**: migration convenience of the current code does not shape the
  architecture. Current behavior is evidence of needs, not a constraint.
- Other users must be able to install Cockpit without Theo's dotfiles, instructions or
  machine-specific paths. Product guidance ships with the product; user policy is separate.

## Why

- The audit found a repository that is hard to understand and extend, with delivery,
  responsiveness and coordination costs rooted in structure, not in file size.
- Shared human control needs accounts, roles and conflict rules that a first release
  does not need; personal installation is the smaller correct product.
- WSL2 reuses the Linux backend; a native Windows app would add a second execution
  model (job objects, AppContainer) with no user asking for it.

## Rules carried into every report

- Every crate states what it owns and what depends on it in one sentence.
- Fail loud at boundaries; a hidden default that masks state is worse than noise.
- Evidence beats claims: "received" is not "done"; "installed" is not "running".
- Prose never grants authority. Instructions, skills, team messages and plugins request;
  `cockpit-authorization` decides.
- Measure before optimizing. Latency and context budgets stay provisional until measured.

## Rejected

- Shared collaboration (multiple humans on one Cockpit): deferred, not designed.
- Native Windows execution, Tauri/web desktop app, Iced native UI: not needed for a
  terminal product; WSL2 covers Windows developers.
- Self-written terminal multiplexer: tmux already solves it (report 05).

## Sources

- WSL filesystems and lifecycle: <https://learn.microsoft.com/en-us/windows/wsl/filesystems>,
  <https://learn.microsoft.com/en-us/windows/wsl/systemd>
