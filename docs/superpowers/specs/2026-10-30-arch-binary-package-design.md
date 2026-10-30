# First Arch binary package: isolated staging

## Intent and decisions

Theo selected an Arch binary package and delegated the remaining preparation choices.
The first target is Linux x86_64 on Theo's Arch workstation, not generic Linux.
The package must install prebuilt runtime artifacts; installation must not compile Rust
or Go. AUR publication, release publication and host activation are separate approvals.

Implementation choice: test the package separately before replacing the running checkout.
Do not install into the real home, register providers, start services or alter PATH during
staging. Preserve current checkout operation while adding package support incrementally.

## What the repository currently requires

- `config/bin/.local/bin/osd` and the other Rust wrappers find binaries relative to the
  resolved wrapper at `apps/target/release`. Preserve that relationship inside the private
  package tree rather than rewriting every wrapper merely to flatten the layout.
- `config/bin/.local/bin/cockpit` separately defaults OSD and remote binaries to
  `$HOME/agent-cockpit/apps/target/release`; those paths need explicit package support.
- `config/bin/.local/bin/greywall-agents/codex` finds its bridge in
  `$HOME/agent-cockpit/config/codex/.codex/cockpit-bridge.mjs`.
- `cockpit-health-check` defaults to a source repository. Component checks under
  `packages/greywall` and `packages/greyproxy` parse source build scripts for expected pins.
- Current systemd units start commands through `%h/.local/bin`; enabled-unit symlinks
  under `config/systemd` express Theo's intent and are not universal package defaults.
- No root license file or PKGBUILD was found. A license must not be assigned silently.

These observations rule out archiving the checkout or simply packaging release binaries
and claiming the whole Cockpit works without its source tree.

## Intended layout and ownership

Proposed implementation choice, pending the complete payload inventory:

- `/usr/lib/agent-cockpit/`: package-owned private runtime tree, retaining wrapper-relative
  directories and the `apps/target/release` binary location within that tree.
- `/usr/bin/`: explicit public command links, not every development helper.
- `/usr/share/agent-cockpit/`: inert integration examples and release identity information.
- Existing user configuration, credentials, databases and runtime paths remain user-owned.

The inventory must classify tracked paths individually before any copying is implemented.
Required helpers must be shipped; optional provider/desktop integration must not overwrite
user files. Learned sandbox grants, enabled-unit symlinks, test/build tools and untracked
files are excluded. A missing necessary resource is an error, not an optional omission.

Greywall and Greyproxy retain their exact reviewed fork revisions. Do not substitute
upstream packages or silently bundle their executables before composition and redistribution
review. Preserve admission, authentication and host approval boundaries.

## Incremental delivery

1. Finish a reviewable payload and dependency inventory, including source-layout blockers.
2. Add package layout support with real entry-point tests and unchanged checkout behavior.
3. Assemble and inspect a private Arch package from exact prebuilt artifacts; prove startup
   without a source checkout, compilers or accidental access to Theo's configuration.
4. After exact-source CI and package smoke tests pass, obtain separate approval for release
   publication and AUR registration. Select the license before public redistribution.
5. Design and approve migration/activation separately; installing package-owned files must
   not itself restart services or migrate mutable state.

Only step 1 is sufficiently specified for implementation now. Later steps need their own
bounded plans based on its evidence; do not invent component artifacts or release URLs.

## Verification and open gates

The first deliverable is documentation, not an installable artifact. No package success is
claimed until an isolated native Arch test proves it. Running Rust executables can contact
live services; smoke tests must have isolated home/runtime/state and fake host commands.

PR 14 was merged by Theo as `681effeb7b19d3f64ff29dd92911ab3cb5cfcc26`.
Its head `f84ba9f5d8b36a982737c0505890f6d00659ef6e` CI run 41047226643 failed:
hosted admission succeeded, then Rust tests stopped at 156 passed and 3 failed;
3,110 tests were not run. The failing model-preference tests report an empty OpenCode
catalog. This is not verification of the merge commit and not a release-ready baseline.

No dependency installation, manifest change, release workflow activation, sandbox policy
change, host deployment or AUR publication is included in this design.
