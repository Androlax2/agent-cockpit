# Torlek user-facing rename implementation plan

> **For agentic workers:** Use `executing-plans` for inline implementation of the already-approved PHASE2 brief.

**Goal:** Ship the seven Torlek commands and their runtime/install namespace atomically for #560.

**Architecture:** Keep the existing capability crates and call sites. Change the product names at their producers and consumers, while keeping legacy detection independent of the new default profile.

**Tech Stack:** Rust workspace, Python release tools, shell installers, Markdown documentation.

**Spec:** #560's approved Alpha 1 scope and `.handoff/torlek-approved-transfer.md`; remaining internal/repository names belong to #570 (Beta 2: cutover).

## Global constraints

- Seven binaries: `torlek`, `torlek-daemon`, `torlek-tui`, `torlek-claude-hook`, `torlek-codex-hook`, `torlek-codex-session`, `torlek-opencode-bridge`.
- Cargo manifest authorization covers binary targets only; preserve package names, dependencies and the lockfile.
- Preserve flag → command name → environment → build profile precedence and development default `dev`.
- No compatibility environment aliases, state adoption/migration/deletion, legacy application edits, repository rename, tags or publication.
- Alpha 1: Linux + WSL2, archive-first. Packaging remains Alpha 2.
- #571 documentation is integrated at `6a35822ab452fe42aa092a109f58b30f181e7975` through approved host request `3d223e0da680`.
- #523 merges first. Hold release scripts/workflows/tests until its exact reviewed handoff; preserve its candidate mechanics and existing review ownership.
- Builds and host operations require their explicit approval; coordinate a single full greenfield gate.

## Review focus

- A legacy `cockpit.service` or foreign `cockpit` on PATH must still prevent default-profile installation despite the new namespace.
- Old `cockpit`/`cockpit-next` files must remain byte-for-byte untouched; only a Torlek-marked same-profile unit may be replaced.
- Inherited environment variables must reach every provider bridge, agent command and sandbox without retaining old aliases.
- Binary paths in tests, archives, helper discovery and version checks must agree with Cargo targets; internal crate references must still resolve.
- Release manifest discovery must distinguish an unpacked release bundle from a development binary, including symlinks.

## Atomic implementation

- [x] Update `crates/cockpit-config/tests/profiles.rs` expectations and add CLI entry-point regressions in `crates/cockpit-cli/tests/profiles.rs` for historical state and foreign units. Run the focused tests before implementation; record actual red results.
- [x] Change profile/layout constants in `crates/cockpit-config`, service ownership and read-only legacy recognition in `crates/cockpit-cli`, with the same-profile replacement and precedence tests retained.
- [ ] Rename only the binary targets in the six owning Cargo manifests (CLI, daemon, TUI, Claude, Codex, OpenCode). Update runtime executable lookup, help, provider bridge/version checks and `CARGO_BIN_EXE_*` consumers together.
- [ ] Change greenfield agent/user-facing environment producers and consumers to `TORLEK_*`, including sessions, daemon, CLI, diagnostics, sandbox and provider integrations; keep internal fixture-only names and persisted protocol fields.
- [ ] After #523's handoff, integrate its exact reviewed main revision through the required host approval. Update archive/install naming and fixtures in release tools, installer and workflows; keep upstream engine/repository identifiers intact.
- [x] Update current README/site/user guidance against the implemented commands, retaining #568's Linux/WSL2 scope and historical evidence.
- [ ] Run focused profile/install, hook/provider, archive/install and consumer checks. Run formatting, lint and one coordinated full greenfield gate; record failures and skips verbatim.
- [ ] Obtain the actual required medium code-review on the full #560 delta, address findings, then deliver a feature PR through permitted Git/host routes. Keep #570 linked and make no full-codebase-rename claim.

## Independent verification before release integration

Native Codex invoked the installed `$code-review medium` workflow on the complete current
rename delta and finished through host request `252120b75294`, continuing the interrupted
original request rather than launching another reviewer. Its four findings were addressed:
MCP profile forwarding, two daemon fixture expectations, and the daemon usage command.
The two daemon assertions still await the held shared-helper integration for execution.

Host request `910a648129c8` passed `111 tests run: 111 passed, 0 skipped`, covering Codex
launch and all six consumers of the isolated CLI fixture. The daemon's invalid-argument
entry point reports `torlek-daemon` and exits 2. CLI/Codex all-target Clippy and daemon
production-target Clippy passed. Earlier production workspace Clippy/Rustdoc and the
complete documentation gate passed. These checks do not replace the combined final gate.

The remaining integration order is #523 protected merge, #382's separate environment
patch, then coordinator release of the shared files. The final archive proof, all-target
daemon checks, combined gate and protected feature-PR merge remain open.
