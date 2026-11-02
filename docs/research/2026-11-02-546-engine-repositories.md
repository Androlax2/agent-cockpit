# Greywall and Greyproxy repository placement

Issue [#546](https://github.com/Androlax2/agent-cockpit/issues/546).
**Research and proposal, 2026-11-02; Theo has not decided.** Merging this document
accepts research delivery only. It does not authorize migration, engine publication,
new dependencies, workflow changes, host installation or a change to Alpha 1.

## Recommendation and decision still needed

Retain the separate forks for Alpha 1. For Alpha 2, recommend first-class Go packages
under `engines/greywall/` and `engines/greyproxy/`, conditional on Theo accepting the
maintenance and standalone-distribution commitments below. Keep their executables,
Go modules, persistent state and review ownership separate from Cockpit's Rust daemon.
Do not introduce FFI or put enforcement inside the daemon to obtain a common checkout.

This would allow one reviewed source change to update an engine, its Cockpit adapter
and their contract fixtures. It would remove the intermediate fork release required
for integrated candidate testing. It would **not** remove platform validation,
publication approval, installation, secret ownership or database compatibility work.
There are no measured latency, throughput, memory or CI savings in this research.
If upstreaming retires the patches or engine maintenance cannot be staffed, keeping
separate repositories with coordinated contract CI is the better outcome.

- [ ] Theo records the repository choice, support commitments and migration authority.

Until that decision is recorded, do not create implementation/migration work from this
proposal or treat the design reports' general `proposed` convention as permission.
The existing [fork-and-pin decision](../design/greenfield/README.md#decision-register-short-form)
and [#470](https://github.com/Androlax2/agent-cockpit/issues/470) remain operative.
Alpha 1 is Linux and WSL2 only; macOS distribution and acceptance are deferred to
[#568](https://github.com/Androlax2/agent-cockpit/issues/568), not passed.

## Evidence snapshot and provenance

Cockpit source examined: [`de91c826c6101ad9f5bbfa8c6ffdebb1ab977037`](https://github.com/Androlax2/agent-cockpit/tree/de91c826c6101ad9f5bbfa8c6ffdebb1ab977037).
[Greywall fork metadata](https://api.github.com/repos/Androlax2/greywall) and
[Greyproxy fork metadata](https://api.github.com/repos/Androlax2/greyproxy) identify
GreyhavenHQ/greywall and GreyhavenHQ/greyproxy as parents. The forks' default `main` branches are not their Cockpit `dotfiles` release
branches. Importing default-branch HEAD would therefore be the wrong provenance.

| Component | Cockpit consumption | Additional source examined, not installed |
| --- | --- | --- |
| Greywall | `v0.3.7-cockpit.1`, `a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf` | `dotfiles` observed at `f44efb8296fe3d6802f08d15e7c13e1c48287849`; public-CA work remains separately owned |
| Greyproxy | `v0.4.5-cockpit.1`, `1d1e518c59f130198eb57b3310e2db16f3353901` | PR #8 merged at `50e2fff5020a232afb903155931aebfa13653797`; PR #9 and `dotfiles` at `15ad5fbfd582d3d85fce79fec09f5a26bb18323e` |

The [pin manifest](../../crates/cockpit-sandbox/engine-pins.json) records six explicit
URLs/digests for Linux x86_64/arm64 and macOS arm64. Archive names differ between
engines; consumers must not synthesize them from one shared naming convention.
The releases API listed only `.1` for each engine at inspection. Greyproxy's existing
[`.2` tag](https://github.com/Androlax2/greyproxy/tree/v0.4.5-cockpit.2) points at
`50e2fff`; a tag is not evidence of successfully published assets or a consumed pin.
PR #9's Linux release repair is merged; a new `.3` tag/publication remains separately
unapproved. Preserve `.2`, old releases and installed engines.

The legacy [Greywall](../../packages/greywall/build.sh) and
[Greyproxy](../../packages/greyproxy/build.sh) workstation scripts have different,
older source pins and install into `~/.local/bin`. They are evidence of fork history,
not the greenfield installation route. Do not repin them during research or remove
them before their existing Beta 2 cutover owner does so.

### Fork changes that a future import must retain

[Greywall fork PR #24](https://github.com/Androlax2/greywall/pull/24) added validated
per-session Linux `--mount` support, used for the provider socket folder. Earlier
fork changes include final deny-read masks, private proxy-state protection, per-run
proxy secrets/API authentication, startup/write protection, fail-closed helper
readiness and large-launch argument handling. The legacy script lists their commits;
import selection must compare the actual `dotfiles` history, not just copy that list.
The [pinned history](https://github.com/Androlax2/greywall/commits/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/)
also records the socat rollback, private devices and reviewed-profile preservation.
Mount support is Linux-specific; it does not establish a macOS capability.

Greyproxy's [fork history through PR #9](https://github.com/Androlax2/greyproxy/compare/v0.4.5...15ad5fbfd582d3d85fce79fec09f5a26bb18323e)
retains listener self-access refusal (`059dda7`), optional management API token
(`00fde68`), client secrets (`7a622a8`), fail-closed plugin startup (`a30e3d4`) and
numeric-IP DNS bypass (`8c98512`), followed by the fork release workflow. These
are part of the maintained fork burden as well as the later session-rule correction.

[Greyproxy PR #8](https://github.com/Androlax2/greyproxy/pull/8) addresses rule ownership
and authenticated session binding. Its [API contract](https://github.com/Androlax2/greyproxy/blob/50e2fff5020a232afb903155931aebfa13653797/docs/api.md)
requires a unique live secret for session grants, distinguishes client labels from
identity, and rechecks expiry, revocation and rotation. Migration 17 separates global
and session rule uniqueness; previously skipped grants cannot be reconstructed and
must be re-registered. Authorization checks do not revoke already established tunnels.
Those are semantic corrections, independent of where the repository lives.

[Greyproxy PR #9](https://github.com/Androlax2/greyproxy/pull/9) repairs release gating
when the Darwin runner is unavailable. At `15ad5fb`, packaging and smoke tests cover
Linux amd64/arm64 only and checksum generation selects the current invocation's
archives. It does not establish publication, macOS readiness or installed behavior.

## Current interfaces and authority owners

| Boundary | Existing owner and contract to preserve |
| --- | --- |
| Cockpit → engines | [`cockpit-sandbox`](../../crates/cockpit-sandbox/src/greywall.rs) checks full commit identity and capability/help output, emits settings, environment and argv; no in-process engine calls |
| Installation | [`install.rs`](../../crates/cockpit-sandbox/src/install.rs) verifies both assets before activation, bounds download/extraction, preserves licenses and safe `greywatch -> greywall`, and retains immutable digest-named engine pairs |
| Launch policy | [`profile.rs`](../../crates/cockpit-sandbox/src/profile.rs) constructs path/network/desktop/secret policy; unsupported enforcement refuses launch; [`launch.rs`](../../crates/cockpit-sandbox/src/launch.rs) creates a fresh client identity per launch |
| Service lifecycle | [`proxy.rs`](../../crates/cockpit-sandbox/src/proxy.rs) owns profile-local configuration, CA/token generation and service-manager startup; Linux/WSL2 use systemd, future macOS uses launchd |
| Endpoint authentication | [`ownership.rs`](../../crates/cockpit-sandbox/src/proxy/ownership.rs) binds service PID/executable and listener ownership before sending the management token; a healthy unrelated port is insufficient |
| Network authorization | Greywall registers proxy sessions; Greyproxy owns its rule/credential database and HTTP/SOCKS authentication. `/api/sessions` supports registration, heartbeat and deletion; management authentication is distinct from per-session secrets |
| Human authority | Cockpit's authenticated sender and host approval paths retain decisions. Engine APIs, agent labels, documentation and a common repository do not grant authority |
| Evidence and persistence | [`observations.rs`](../../crates/cockpit-sandbox/src/observations.rs) validates immutable launch-bound owner attestations; caller retains transaction ownership. Requested policy, fixture results and live observations remain distinct |

The public CA may be readable inside the sandbox; its private key, management token,
credential database and session keys must remain unreadable and unwritable. Do not
combine engine and Cockpit databases or transfer transaction ownership. Keep service
management on the owner side, sandboxed children at their existing isolation boundary,
and management credentials out of agent argv/logs. A common source tree does not justify
adding privileges, changing authenticated identity or widening network rules.

## What actually caused the integration delays

| Example | Evidence and cause | Effect of consolidation |
| --- | --- | --- |
| [#470](https://github.com/Androlax2/agent-cockpit/issues/470) managed installation | Required owned paths, verified archives, safe extraction and fresh-home installation; implementation delivered in #511 | Common builds can simplify artifact selection; installer correctness and clean-host checks remain necessary |
| [#495](https://github.com/Androlax2/agent-cockpit/issues/495) lifecycle | A matching binary did not create configuration, start the proxy or authenticate its endpoint; #528 delivered that implementation, with real-service acceptance still open | Same source review helps contracts; neither common history nor bundling starts a correctly owned service |
| [#433](https://github.com/Androlax2/agent-cockpit/issues/433) enforcement | Plans and fake-engine tests do not prove denied-path/network behavior; disposable real Linux/WSL2 acceptance and protected workflow authority remain separate | Cross-component fixtures become easier to review together; kernel/service/network prerequisites and host approval do not disappear |
| [#527](https://github.com/Androlax2/agent-cockpit/issues/527) public CA | Pinned macOS Seatbelt denial also hides the required CA. Linux re-exposes it. A later correction also needs exact capability reporting and real macOS proof | Atomic adapter/capability tests help catch mismatches; repository placement cannot supply the missing behavior or a macOS runner |
| Greyproxy correction delivery | #8 fixed schema/identity at `50e2fff`; `.1` predates it. Darwin release execution blocked the corrected distribution; #9 narrows the release matrix | Integrated candidate builds remove one intermediate release/pin handoff. Separate engine/security review, approved publication and platform truth remain mandatory |

For #527 the pinned [CA resolver](https://github.com/Androlax2/greywall/blob/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/internal/sandbox/utils.go),
[macOS denial](https://github.com/Androlax2/greywall/blob/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/internal/sandbox/macos.go)
and [Linux re-exposure](https://github.com/Androlax2/greywall/blob/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/internal/sandbox/linux.go)
support the diagnosis. The issue retains later correction failures and capability
reporting findings. This research does not certify the latest Greywall tip as fixed.

## Compare the three repository models

| Model | Advantages | Costs and failure modes | Best fit |
| --- | --- | --- | --- |
| Separate forks, pinned artifacts | Existing release history, standalone consumers, direct upstream PRs and distinct security ownership; independent emergency engine release | Several reviews/releases/pins per coordinated change; contract drift until integration test; several issue trackers | Alpha 1 and a future where patches converge upstream; add coordinated contract checks without relocating source |
| Vendored snapshot or subtree | One reproducible source checkout; upstream identity can remain explicit; subtree history can retain authorship | Two apparent sources of truth if forks also accept changes; repeated sync conflicts; vendor edits obscure maintained ownership; snapshot-only import loses navigable history | A mostly upstream engine with a small bounded patch queue; use one canonical edit location and record every sync |
| First-class packages | Engine+adapter+fixtures can change in one PR; shared issue tracking and candidate release; direct AI-agent priorities | Cockpit owns two Go projects and their advisories, compatibility and releases; broader checkout/CI/toolchain burden; harder extraction/upstream PR mapping | Conditional Alpha 2 recommendation if sustained fork ownership is intended |

A subtree is an import/sync mechanism, not a separate runtime design. First-class
packages could initially use a history-preserving subtree import, then become the
canonical development location. Do not maintain editable mirrors in both locations.
A Rust rewrite would add a new correctness project and is not required by any option.

## Proposed layout, ownership and upstream policy

| Proposed location | One-sentence ownership |
| --- | --- |
| `engines/greywall/` | Existing Go module owns platform isolation and proxy-session launch integration |
| `engines/greyproxy/` | Existing Go module owns authenticated network enforcement, rule/credential storage and its migrations |
| `crates/cockpit-sandbox/` | Existing Rust adapter owns Cockpit policy translation, verified install, lifecycle and evidence |
| Engine-local provenance record | Names exact upstream/fork bases, import commit, original URLs, patch mappings, licenses and reproducible build inputs |

Keep existing Go module paths initially to avoid an unrelated import rewrite. Engines
are not Cargo workspace members or Rust capability crates. Require engine-owner review
for enforcement/auth/schema changes and adapter-owner review for interface changes;
release owner reviews packaging, provenance and pins. Security disclosures continue
through the existing private security process. Ownership must survive a combined PR.
Existing sandbox owner `ses_e63e1fb50ffeoYB8onQ0H4V3bG` and Greyproxy owner
`native01a19d0c-8a9c-72c0-90ec-5feb71cb2cc8` retain their current implementation work.

Proposed sync policy: designate an engine maintainer, review upstream releases/advisories
at least monthly and before each Cockpit release, with urgent patches handled separately.
Import a named upstream commit on a dedicated sync branch; keep mechanical sync separate
from local behavior changes. Record original authors, commit IDs and upstream PR status.
Offer generally useful fixes upstream; retain an explicit patch ledger until upstream
ships them and compatibility tests allow removing them. Preserve fork repositories and
links as historical archives if canonical ownership moves; do not delete or rewrite them.
The ledger should expose divergence rather than promise the fork will retire on a date.

## Licenses and distribution inputs

Read sources: Greywall's [Apache-2.0 license](https://github.com/Androlax2/greywall/blob/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/LICENSE)
and [NOTICE](https://github.com/Androlax2/greywall/blob/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/NOTICE),
Greyproxy's [MIT license](https://github.com/Androlax2/greyproxy/blob/15ad5fbfd582d3d85fce79fec09f5a26bb18323e/LICENSE),
and embedded tun2socks' [GPLv3 text](https://github.com/xjasonlyu/tun2socks/blob/v2.5.2/LICENSE).

- Retain Greywall's license, relevant attribution/NOTICE and prominent modified-file
  notices under Apache section 4. Preserve its authorship and fork provenance.
- Retain Greyproxy's copyright and permission notice in copies/substantial portions,
  including its Greyproxy Authors and ginuerzh attribution. A top-level Apache license
  cannot replace the MIT file or third-party notices.
- Greywall's Linux archives embed unmodified tun2socks v2.5.2 and ship its GPL license.
  GPL section 6 requires a compliant corresponding-source delivery route for object
  distribution; section 1 includes relevant build/install scripts. Preserve source
  availability directions and verify the chosen route for the exact artifact before
  any new distribution. Merely copying a license is insufficient. Section 5 distinguishes
  an aggregate from a combined work; placement alone does not settle that question.
  Do not relabel the embedded binary Apache or infer that linking engines into Rust
  has no licensing consequences.

This inventory is not an artifact-wide dependency-license audit or a determination of
combined-work status. A migration must inventory Go dependencies, embedded assets and
any bundled frontend notices as well as the Rust graph. Retain per-component licensing;
Cargo's allowlist does not audit Go or prebuilt embedded binaries. Complete this review
before changing distribution, whichever repository model Theo selects.

## Builds, releases and platform tradeoffs

The pinned [Greywall workflow](https://github.com/Androlax2/greywall/blob/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/.github/workflows/cockpit-release.yml)
uses Go 1.25.14 with `GOTOOLCHAIN=local`, cross-builds three targets and runs native smoke
jobs before tag publication. Its [build script](https://github.com/Androlax2/greywall/blob/a9ccc58c4e57f7ad7aaf7c2ab3779d71cf485baf/scripts/cockpit-release-build.sh)
verifies embedded tun2socks digests, stamps source identity/commit time, trims paths and
normalizes archive metadata. Greyproxy's [Go module](https://github.com/Androlax2/greyproxy/blob/15ad5fbfd582d3d85fce79fec09f5a26bb18323e/go.mod)
declares Go 1.24.0/toolchain 1.24.5; its [workflow](https://github.com/Androlax2/greyproxy/blob/15ad5fbfd582d3d85fce79fec09f5a26bb18323e/.github/workflows/cockpit-release.yml)
selects Go through that file. Its [packager](https://github.com/Androlax2/greyproxy/blob/15ad5fbfd582d3d85fce79fec09f5a26bb18323e/scripts/cockpit-release.sh)
uses readonly modules, deterministic timestamps and tar/gzip metadata. Both workflows
include checksums and provenance attestation steps. These are source properties;
this research did not independently rebuild twice or verify released attestations.

Proposed integrated releases should still emit standalone engine archives plus a
machine-readable compatible engine pair for Cockpit. Keep version/help contracts,
exact source IDs, asset hashes and source/provenance mapping; changing repository origin
must not make the installer accept arbitrary binaries. Select each engine's toolchain
explicitly before considering a shared Go upgrade. Keep their lock/sum files and
security scans independent of Cargo. Isolate release inputs from PR-writable caches.
Cache ordinary CI by OS/architecture/toolchain and module sums only after measuring
correctness and benefit; never bypass native smoke tests because a cross-build passed.

Linux x86_64/arm64 packaging is distinct from Linux/WSL2 enforcement. WSL2 needs its own
service/kernel capability tests; a Linux archive does not prove WSL2 acceptance. Retain
macOS refusal and future native tests; restoring support needs public-CA isolation,
capability reporting, corrected archives and real enforcement evidence. No Windows-native
engine/package promise follows from WSL2 support. A common tree adds Go tests and binary
builds to relevant PRs; measure CI wall time, queue time and cache behavior before claiming
savings. Docs-only and unrelated Rust changes should not rebuild both engines by default.

## AI-agent priorities and bounded verification proposal

All benefits below are expected, not measured here. Preserve current owners; do not start
these changes under this research issue. Begin with existing fixtures and approved
isolated runners; never substitute a user's live services, credentials or provider calls.

| Priority | Improvement and evidence | Expected benefit and bounded proof |
| --- | --- | --- |
| P0 | Consume the independently approved #8 correction; unique per-launch credentials/policy, not shared labels | Avoid cross-session grant selection. Test two simultaneous sessions with overlapping/disjoint grants, duplicate/missing secrets, expiry/revocation/rotation and HTTP/SOCKS handlers; require exact identity isolation |
| P0 | Preserve private-state denial while exposing the public CA (#527) | Permit TLS trust without granting private-state access. Future macOS fixture and native disposable runner must show readable cert, denied key/token/database, denied writes and allowed TLS; missing capability must refuse |
| P0 | Carry #433 evidence through real install/lifecycle boundaries | Distinguish requested from observed isolation. Run four sequential launches and two concurrent launches with allowed controls, denied fixture paths and exact allow/block proxy decisions; timeouts alone never pass |
| P1 | Startup/stop ownership and cleanup (#495) | Reduce stale clients, orphan processes and misleading readiness. Fault-inject occupied ports, wrong PID, interrupted setup, child failure and restart; assert only owned processes/session state are cleaned and failed startup stays refused |
| P1 | Bounded resources under many agent sessions | Prevent noisy sessions starving control work. Start with 2 and 4 fixture sessions, cap probe time/output, measure startup/stop latency, CPU/RSS, file descriptors and rule counts before/after cleanup; choose budgets from baseline data, not invented targets |
| P1 | Explicit owner approvals and actionable diagnostics | Preserve authenticated ownership through pending network decisions and stale results. Test replaced/revoked sessions and approval rechecks; emit typed next actions with launch identity and redacted policy/decision evidence, never secrets or hosted content |
| P2 | Candidate artifact and contract matrix | Catch version/capability/archive mismatches before publication. Check full SHA, flag/capability parsing, both archive layouts/licenses, tamper refusal, deterministic rebuild comparison and supported target smoke; keep signing/publication a separate operation |

No new performance target or security acceptance is asserted. Record toolchain, source
and artifact identities, OS/kernel, session count, deadlines, successful controls and
cleanup outcomes for each future measurement. A benchmark improvement must include the
same workload and baseline; a test skip stays a skip.

## Staged migration and rollback proposal

1. **Decision gate:** Theo chooses placement, standalone support, engine maintainers and
   release policy in a separate decision record. Link it from #546; leave this issue's
   final acceptance open until then. Alpha 1 owners finish their existing work first.
2. **Provenance-only import:** after authorization, choose reviewed exact `dotfiles`
   commits with their owners; preserve history/attribution, local licenses and original
   module paths. No state/install path change. Verify tree equivalence and patch ledger.
   Rollback is removal/revert of the new source subtree; old forks/releases stay usable.
3. **Candidate builds:** add separately authorized CI/build work with fixtures, contract
   tests and standalone artifacts, without publishing or updating production pins. Build
   the same target twice in clean environments and compare digests. Exercise old adapter
   compatibility and new candidate refusal paths. Rollback disables the candidate route.
4. **Release transition:** only after engine/security/release review and platform proof,
   propose exact new artifacts, source mapping, checksums and pin update. Preserve the
   verified installer and immutable pair directories; do not overwrite active sessions'
   binaries or auto-restart them. Publication needs its own approval. Existing fork tags,
   download URLs and engine names remain intact; migration must not strand old clients.
5. **Opt-in upgrade and state:** test a copied fixture proxy database through migration 17,
   registration and restart. Back up private state with owner-only access before a real
   approved upgrade. Do not reconstruct missing grants from another session. Treat
   downgrade compatibility as unproven: stop only the owned service and restore its full
   prior database/state snapshot plus prior compatible binaries if needed. Do not run an
   old binary against a migrated database merely because old files still exist.
6. **Canonical ownership switch:** after acceptance, route new engine development to the
   common repository and link old issue trackers/PRs to their successors. Preserve fork
   history and independent users' source access. Keep an emergency engine-only patch/release
   path; source and release coupling must not force an unrelated Cockpit upgrade. Rollback
   can resume the existing forks and old pin route without deleting history or user state.

Standalone recommendation: continue building the `greywall`/`greywatch` and `greyproxy`
executables without Cockpit, keep existing CLI/API/module contracts, and document engine-only
release support. This costs ongoing tests, packaging and advisory handling. Theo may instead
choose Cockpit-only maintenance; that requires an explicit deprecation/support policy for
external users, not silent removal of their interfaces or releases. No usage counts or
third-party compatibility survey was performed, so downstream impact is unresolved.

## Research delivery versus acceptance

This document supplies comparison, ownership/interface proposal, license/source inventory,
AI-agent priorities and staged migration/rollback. It changes no executable behavior.
Verification for this PR is `make docs-ci`, diff inspection, primary-source cross-checking
and the actual supplied `$code-review medium` workflow. Broad Rust/Go suites and live
platform probes are not appropriate proof for prose and were not run for this research.
Implementation, platform acceptance, artifact reproducibility and Theo's final decision
remain unproven/open. No shared coverage row is claimed by documentation alone.
