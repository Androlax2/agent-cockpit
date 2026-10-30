# Arch package staging

**Status:** tested staging machinery, not a published release or AUR package.
See the [runtime inventory](arch-runtime-inventory.md) for the remaining runtime and activation
work. Do not describe this intermediate recipe as a working `yay` installation route.

## What exists

- `packaging/arch/PKGBUILD` consumes a local, prebuilt runtime archive. It has no compilation
  function, dependency downloader or installation/activation hook.
- `packaging/arch/package.py` verifies the declared source/version/target, unchanged fork
  revisions, exact file inventory, SHA256 values and canonical Apache-2.0 project license.
  It copies only allowlisted files and promotes a fully staged directory atomically.
- `runtime-files.txt` owns the runtime source inventory. `integration-files.txt` owns inert
  provider/desktop/service examples. Neither includes learned grants or enabled-unit links.
- `preview_policy.py` produces a new 0600 policy proposal. It changes only exact checkout
  prefixes in existing filesystem rule lists; it never activates or overwrites a policy.

The native runtime package owners were inspected through reviewed host commands, including
GTK4/layer-shell, the `libgcc` runtime and `bind` for `dig`. Package ownership is evidence
for this machine, not an ABI minimum or proof that an Ubuntu CI binary runs on Arch.

## Prebuilt input contract

The archive must contain a `runtime/` directory with `release.json` and all required data.
Its exact inventory consists of both source lists, the 17 named Rust binaries, both exact
fork binaries, required desktop assets and third-party license notices. The executable
paths remain `apps/target/release/<name>` inside the private tree, preserving wrapper lookup.

`release.json` has exactly these fields:

| Field | Meaning |
| --- | --- |
| `version` | Selected package version; must match the recipe input |
| `source` | Exact 40-character source commit; must match the recipe input |
| `target` | `x86_64-unknown-linux-gnu` |
| `components` | Greywall and Greyproxy source revisions matching the existing reviewed pins |
| `files` | Exact source-relative payload paths mapped to lowercase SHA256 values |

This verifies **integrity and declared identity**, not provenance by itself. A caller cannot
prove a binary's source merely by writing its commit into JSON. Release production must
establish the clean exact-source build, dependency notices, native compatibility and successful
required checks separately. Unit tests use fake executable contents; they do not prove startup.

The staging recipe requires `COCKPIT_PACKAGE_VERSION`, `COCKPIT_SOURCE_COMMIT` and
`COCKPIT_PAYLOAD_SHA256`. Its local archive name is derived from the version, with no invented
download URL and no `SKIP` checksum. Auxiliary recipe inputs have pinned hashes too.
Generate `.SRCINFO` only with the actual selected archive identity; freeze public release
inputs before publishing an AUR recipe rather than requiring users to invent environment values.

## Ownership and policy safety

Runtime files go under `/usr/lib/agent-cockpit`; explicit public commands are linked in
`/usr/bin`. Provider binaries and the host clipboard command are not replaced. Integration
examples go under `/usr/share/agent-cockpit/integrations`, outside live systemd/provider config.
Licenses go under `/usr/share/licenses/agent-cockpit-bin`.

The policy preview tool supports strict JSON plus the existing learned profiles' leading
`//` comment headers, which it preserves. Other JSONC syntax fails instead of being guessed
or silently rewritten. Duplicate keys, malformed rule lists, parent traversal and whole-root
grants require review; existing preview files are never overwritten. Generate a migration
from the actual host policy and bind activation to its reviewed content, not to a repository
template that might discard newer user grants.

No preview or checksum grants authority to apply it. Live configuration migration, package
installation, service restart and release/AUR publication retain their separate host approvals.
Installing package-owned files must not itself enable services, replace credentials, or
silently turn source-oriented `cockpit apply` into package activation.

## Verification

The staging and policy-preview regressions run in the existing `tools/tests` Python suite.
They cover integrity failures, missing inputs, source symlinks, unexpected metadata,
component mismatches, existing-output preservation and exact-policy translation. The
canonical full check remains `make ci`, with its normal resource admission and test lock.

Native installation/startup/upgrade/uninstall, complete launcher/hook resolution, artifact
provenance and redistribution notices remain release gates. Nothing in this document claims
those checks have already passed.
