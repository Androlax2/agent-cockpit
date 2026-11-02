# User README acceptance

Issue #98 remains open until a volunteer with no prior Cockpit context installs and starts a session from the README alone.
This record separates the documentation draft from that live acceptance. No volunteer walk has been recorded.

## Source coverage

| README section | Current source or entry-point fixture |
| --- | --- |
| First-run checklist | `crates/cockpit-cli/src/onboarding.rs`; `crates/cockpit-cli/tests/onboarding.rs` |
| Profile and daemon service | `crates/cockpit-cli/src/profile.rs`, `daemon_install.rs`; `crates/cockpit-cli/tests/profiles.rs` |
| Managed engine/proxy setup and status | `crates/cockpit-cli/src/sandbox_install.rs`, `crates/cockpit-cli/tests/sandbox_install.rs`; `crates/cockpit-sandbox/README.md`; `crates/cockpit-sandbox/tests/managed_proxy.rs` |
| Project and first session | `crates/cockpit-cli/tests/projects.rs`; `crates/cockpit-cli/tests/sessions.rs` |
| Approvals | `crates/cockpit-cli/src/approvals.rs`; `crates/cockpit-cli/tests/ask.rs` |
| Configuration and directories | `crates/cockpit-config/src/lib.rs`, `layout.rs`; `crates/cockpit-cli/tests/config_show.rs` |
| Providers | Adapter READMEs under `crates/cockpit-claude`, `cockpit-codex`, `cockpit-opencode`; capability pins in each adapter's `src/lib.rs` |
| Help and support bundles | `crates/cockpit-cli/src/bug_report.rs`; `crates/cockpit-cli/tests/bug_report.rs` |

These are source pointers, not a claim that every listed test was run for this documentation change.
Documentation checks and actual test outcomes belong in the PR's commands record.

## Release checkpoints

- [ ] Replace the README's install status with #97's final public archive instructions. The merged #470 installer and #495 proxy setup are documented from source; public release and platform acceptance still need the release owners' evidence. Do not edit their release files.
- [ ] Check provider table and platform evidence against the final merged Alpha release. Coordinate live capability claims with the provider lane.
- [x] Add a greenfield fixture render with fictional data. [Capture evidence](98-demo-screenshot.json) identifies source hashes, real renderer, isolated capture and inspection; legacy screenshots stay in the legacy guide.
- [ ] Chief arranges a volunteer with no prior context, on a supported platform, using an isolated clean home. Record consent for any live provider usage and host effects through the approval workflow before the walk.
- [ ] Record exact release version/revision, platform, starting environment, README revision, commands followed, deviations and resulting session ID (redact private data). Confirm the volunteer used the README alone.
- [ ] Correct any onboarding failures and repeat the affected steps; link the measured result here and in #98 before closing it.

## Volunteer result

Pending. A fixture run cannot satisfy the volunteer acceptance criterion, nor prove public archive onboarding across all supported platforms.

## Merged setup documentation

The README follows #470's installer (`2553ebff48d8c812c8464996fe9a50e27d97fc1d`)
and #495's managed proxy wiring (`d1f7b0a08f36f0c67c8fb8cbf6da4e9c66900f95`),
both present in this branch's base `2ebc7f13c10bb0fb6b3fea125cd17a097ebb4d46`.
The command downloads and verifies engines, then configures and starts the profile's
proxy before the daemon. The README records curl, service-manager prerequisites,
fixed-port collision refusal, state locations and the current macOS public-CA
capability blocker (#527). It does not suggest changing the sandbox policy to bypass
that blocker.

Provider wording follows the current adapter READMEs. Codex's local fake-model
approval-routing result on 0.166.0 is separate from its 0.161.0 capability pin and
the unmeasured signed-in and resume/fork checks. No provider conformance was rerun
for this documentation update.

The published guide already contains generated CLI help and effective configuration
defaults; the protocol schema remains planned under #295. `make site-ci` checks
the actual generated pages and version-prefixed links. None of these source or
build checks measures public-archive installation or a volunteer session.

## Greenfield demo image

The README image comes from the existing `two-projects.feed`, existing fake
approval helpers and the same `cockpit_tui::draw` function used by the binary.
`crates/cockpit-tui/examples/readme_demo.rs` renders an 80 × 24 cell frame through
Ratatui's test backend, using only fictional data. It completes a fake focus
reply and advances fixture time past the TUI's reading grace; no returned command is executed.
The SVG projection retains the current default-color/bold/reverse cell styling
and rejects unsupported styles. ImageMagick rasterizes it with a temporary home
and font cache. This is a fixture render, not a live terminal/provider capture.

Regenerate with the pinned Rust toolchain and an already installed ImageMagick:

```sh
make readme-screenshot
```

The target installs nothing and changes only the generated image. Its self-checks
exercise the actual renderer and require the expected projects, request and keys.
The image is labeled as fictional data. This completes the image checkpoint,
while public installation, final provider evidence and volunteer acceptance stay
open. No new product layout, design tokens or dependencies were introduced.
