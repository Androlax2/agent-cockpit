# User README acceptance

Issue #98 remains open until a volunteer with no prior Cockpit context installs and starts a session from the README alone.
This record separates the documentation draft from that live acceptance. No volunteer walk has been recorded.

## Source coverage

| README section | Current source or entry-point fixture |
| --- | --- |
| First-run checklist | `crates/cockpit-cli/src/onboarding.rs`; `crates/cockpit-cli/tests/onboarding.rs` |
| Profile and daemon service | `crates/cockpit-cli/src/profile.rs`, `daemon_install.rs`; `crates/cockpit-cli/tests/profiles.rs` |
| Project and first session | `crates/cockpit-cli/tests/projects.rs`; `crates/cockpit-cli/tests/sessions.rs` |
| Approvals | `crates/cockpit-cli/src/approvals.rs`; `crates/cockpit-cli/tests/ask.rs` |
| Configuration and directories | `crates/cockpit-config/src/lib.rs`, `layout.rs`; `crates/cockpit-cli/tests/config_show.rs` |
| Providers | Adapter READMEs under `crates/cockpit-claude`, `cockpit-codex`, `cockpit-opencode`; capability pins in each adapter's `src/lib.rs` |
| Help and support bundles | `crates/cockpit-cli/src/bug_report.rs`; `crates/cockpit-cli/tests/bug_report.rs` |

These are source pointers, not a claim that every listed test was run for this documentation change.
Documentation checks and actual test outcomes belong in the PR's commands record.

## Release checkpoints

- [ ] Replace the README's install status with #97's merged public archive/installer instructions, including the merged engine lifecycle from #470. Coordinate with their owners; do not edit their release files.
- [ ] Check provider table and platform evidence against the final merged Alpha release. Coordinate live capability claims with the provider lane.
- [ ] Add a screenshot from a greenfield demo fixture once one is available. Existing legacy demo screenshots remain labelled in the legacy guide.
- [ ] Chief arranges a volunteer with no prior context, on a supported platform, using an isolated clean home. Record consent for any live provider usage and host effects through the approval workflow before the walk.
- [ ] Record exact release version/revision, platform, starting environment, README revision, commands followed, deviations and resulting session ID (redact private data). Confirm the volunteer used the README alone.
- [ ] Correct any onboarding failures and repeat the affected steps; link the measured result here and in #98 before closing it.

## Volunteer result

Pending. A fixture run cannot satisfy the volunteer acceptance criterion, nor prove public archive onboarding across all supported platforms.
