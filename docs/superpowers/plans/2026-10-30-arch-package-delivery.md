# Arch Binary Package Delivery Implementation Plan

> **For agentic workers:** Use executing-plans inline, with test-driven-development and verify-cockpit for each executable change. Publication, security changes and live activation retain their separate approval gates.

**Goal:** Complete the selected Arch binary-package delivery, beginning with a green CI baseline and relocatable runtime resources.

**Architecture:** Preserve checkout behavior and installed command names. Stage an explicit runtime payload outside user configuration, using the private layout described in the [design](../specs/2026-10-30-arch-binary-package-design.md). Never turn package installation into a source build or an unreviewed service/configuration migration.

**Tech Stack:** Existing Rust, shell and Python; native Arch packaging tools after approval; existing CI and fake-command fixtures.

**Spec:** [Design and safety boundaries](../specs/2026-10-30-arch-binary-package-design.md), supplemented by Theo's request to complete all five delivery stages. The earlier inventory-only restriction no longer limits preparation/implementation; scoped permission gates still apply.

## Global constraints

- Target Linux x86_64 Arch first; do not claim other platforms work.
- Keep user state, credentials, grants and service enablement outside package-owned files.
- No dependency manifest change, installation, policy migration, publication or host activation without its required approval.
- Keep existing workflow/component owners and unrelated untracked/worker work intact.
- Source identity and successful verification must match the actual published artifacts.

## Task 1: Repair the POSIX model-catalog fixture

**Files:** `apps/cockpit/src/tests.rs`, `apps/cockpit/src/tests/menus.rs`.

**Observed failure:** GitHub run 41047226643 stopped after three catalog-preference tests failed with EOF JSON. The shared shim declares `/bin/sh`, while model_catalog uses `[[` and `==`; ShellCheck reproduces SC3010/SC3014. Host `/bin/sh` accepts that extension, so local passing tests did not prove portability.

- [ ] Add a regression that validates the actual generated model-catalog shim with the existing ShellCheck prerequisite, then parses its actual command output.
- [ ] Run the focused regression through agent-build; confirm RED on the declared shell dialect.
- [ ] Change only that fixture condition to quoted POSIX `[ "$1" = opencode-models ]`.
- [ ] Run the regression and affected menu tests; then run the canonical full gate once when the source settles. Preserve failures and skipped/unrun evidence.
- [ ] Commit the isolated repair, and publish through the reviewed host workflow when authorized; verify exact-head GitHub results rather than assuming local success.

## Task 2: Make required collectors resolve the private installed layout

**Files:** `apps/crates/cockpit-core/src/paths.rs`, `usage.rs`, `subscription_fallback/native.rs`; existing fixture tests for their real callers.

**Lookup contract:** Keep an existing home Waybar collector first, including a broken symlink
or an unreadable existing path so an explicit user installation is not silently replaced.
Only a genuinely absent user path falls back to the same filename under
`/usr/lib/agent-cockpit/config/waybar/.config/waybar/scripts`. This native Arch prefix is fixed;
no new environment override or upward filesystem search is introduced. The four directly
called collectors are claude-usage.sh, codex-usage.sh, codex-session.py and
native-subscription.py; codex-quota.py remains their adjacent shared dependency.

**Interfaces:** Preserve usage_script() and codex_session_script() returning PathBuf; add
codex_usage_script() and native_subscription_script() with that same return contract.
One private collector_script(name: &str) -> PathBuf owns the lookup. Consumers retain their
existing validation and subprocess behavior; no credential/state changes.

- [ ] Inspect existing user-path compatibility and explicit override behavior before choosing the smallest lookup contract.
- [ ] Write failing entry-point tests for installed layout without a checkout or home Waybar scripts; retain existing user-path tests and errors for malformed/missing resources.
- [ ] Implement explicit installed resource lookup without a general configuration/service abstraction or new dependencies.
- [ ] Keep the five collectors together with their required siblings; verify fake-provider behavior and no access to live user state.
- [ ] Run affected core/client tests and checks; commit separately from the CI repair.

## Subsequent delivery gates

These stages require exact file lists and executable tests from the preceding evidence before implementation, not speculative scaffolding:

1. Resolve command/launcher/hook paths, missing external assets, component artifact composition and service templates. Obtain specific approval before sandbox-policy migration; never broaden grants merely to make relocation work.
2. Verify native Arch dependencies using approved host inspection because the sandbox cannot read its pacman database. Ask before creating/changing the dependency-bearing PKGBUILD or installing missing tools.
3. Assemble the explicit prebuilt payload and native package; run isolated fresh-install/startup/upgrade/uninstall tests with no source checkout/compiler or live service access. Do not call a partial package complete.
4. Obtain Theo's license decision and component redistribution review. Coordinate release ownership; publish versioned verified artifacts and AUR only through reviewed host actions, with real checksums and no invented release URLs.
5. Preview migration, preserve active sessions/state/approvals and enabled-service intent, then obtain host approval for installation/activation and verify health/rollback boundaries.

## Review focus

Failing/non-Bash shell fixtures; shadowing user overrides; hidden checkout fallback; missing sibling modules; package-prefix paths admitted to sandboxes without review; mutating commands mislabeled as checks; native ABI compatibility inferred from an unrelated binary; merged sources mistaken for green release sources.
