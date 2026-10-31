# Gate M0: foundation proof

Issue #27. Each item of the gate in [report 20](../design/greenfield/20-refactoring-plan.md)
links to a test that drives the binary or the protocol, or to a recorded run on named
hardware. A platform counts only where it ran; an item without proof stays open below.

## Proof

| Gate item | Proof | Linux | macOS | WSL2 |
| --- | --- | --- | --- | --- |
| Daemon starts on a clean home | `cockpit-daemon` `startup::a_daemon_starts_on_a_clean_home_with_only_home_in_its_environment` (only `HOME` set, nothing in it) and `startup::a_daemon_starts_on_a_clean_home_and_answers_ping` (XDG variables set) | run 1 | CI job `greenfield-macos` (`macos-26`, arm64), with the rest of `startup` | open: [checklist](m0-wsl2-recorded-run.md) |
| Versioned protocol handshake and version refusal | `cockpit-daemon` `protocol::a_client_sharing_no_version_is_refused_with_the_daemons_versions` over the daemon's socket; `cockpit-protocol` `handshake::*`; `cockpit-client` `handshake::a_refused_handshake_reports_the_daemons_reason` | run 1 | not run | open: checklist |
| `COCKPIT_LOG_LEVEL` sets the daemon's level | `cockpit-daemon` `startup::cockpit_log_level_decides_which_records_the_daemon_writes`, `startup::an_unreadable_log_level_stops_startup` | run 1 | CI job `greenfield-macos` | open: checklist |
| Runtime level override with auto-revert | library: `cockpit-diagnostics` `level::an_override_raises_the_running_filter`, `level::an_override_reverts_when_it_expires` | run 1 (library) | not run | not run |
| Support bundle | library: `cockpit-diagnostics` `support_bundle::the_plan_lists_logs_sections_and_platform_facts_before_writing`, `support_bundle::the_written_archive_holds_exactly_the_listed_entries` | run 1 (library) | not run | not run |
| Crate-graph rule fails on a forbidden edge | `cockpit-workspace-rules` `crate_graph::each_rule_catches_its_forbidden_edge`, `crate_graph::an_edge_through_an_intermediate_crate_is_forbidden`, `crate_graph::the_workspace_has_no_forbidden_dependency_edge` | run 1, and every PR's `greenfield` CI job | not run | not run |

## Recorded runs

**Run 1, Linux, 2026-10-31.** Base `a91c859` plus this branch's tests. AMD Ryzen 7 7800X3D
(16 threads), 30 GiB RAM, Arch Linux, kernel 7.2.11, x86_64, inside Theo's greywall sandbox.
cargo 1.99.1, cargo-nextest 0.9.150.

```sh
cargo nextest run --locked --cargo-quiet --manifest-path Cargo.toml \
  -p cockpit-daemon -p cockpit-protocol -p cockpit-client -p cockpit-diagnostics \
  -p cockpit-workspace-rules \
  -E 'binary(startup) | binary(protocol) | binary(handshake) | binary(level) | binary(support_bundle) | binary(crate_graph)'
```

Outcome: `73 tests run: 73 passed, 0 skipped`.

**macOS.** The `greenfield-macos` CI job runs `cockpit-daemon`'s `startup` tests, both
clean-home tests included, on GitHub's `macos-26` arm64 runner whenever CI selects the
greenfield job (`tools/ci-jobs.py`). Its first run is linked from the PR that added it; the
other gate items have not run on macOS.

## Open items

- **WSL2:** no run yet. Theo fills [the WSL2 checklist](m0-wsl2-recorded-run.md) on his
  machine and links the filled copy here.
- **`cockpit logs level` and `cockpit support-bundle` verbs:** not wired. The library is
  proven above; the verbs need the host-only channel, which #108 assigned to #32 (M1). The
  coverage-map rows for both stay pending until a test drives them through the CLI.
- **macOS beyond `startup`:** the handshake, override, bundle and crate-graph tests run on
  Linux only.
