# Arch Package Inventory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce the exact runtime ownership inventory needed to stage the first Arch binary package safely.

**Architecture:** Inspect tracked paths and real callers before defining any package copy rules.
Keep the source-checkout installation working and the live host untouched. This first slice
produces the package input specification, not a nominal package with missing runtime support.

**Tech Stack:** Existing shell/Python/Rust sources; Markdown inventory; read-only Git inspection.

**Spec:** [Arch binary package design](../specs/2026-10-30-arch-binary-package-design.md).

## Global Constraints

- Linux x86_64 on Arch is the first target; no broader compatibility claim.
- Preserve installed command names, approval/authentication, resource admission and state.
- No untracked files, personal credentials or learned grants enter the package inventory.
- No dependency install, manifest edit, host activation, AUR publication or release publication.
- Do not change or duplicate the existing release-workflow owner's work.

## Review Focus

- A wrapper resolves through a public symlink: its binary and adjacent resources still resolve.
- No checkout exists in the test home: startup must not quietly consult `~/agent-cockpit`.
- A provider or optional desktop profile is absent: diagnose that capability without overwriting configuration.
- A runtime helper lives under Waybar: distinguish required collection from optional presentation.
- A file grants capabilities or enables a service: do not classify it as a harmless universal default.

---

### Task 1: Audit the package payload and layout dependencies

**Files:**

- Create: `docs/packaging/arch-runtime-inventory.md`
- Read: `config/**`, `packages/**`, `apps/**/src/**`, `Makefile`
- Do not modify runtime source or the existing untracked portability roadmap.

**Interfaces:**

- Consumes: tracked source paths, Cargo executable declarations and actual script/resource callers.
- Produces: one row per proposed payload/integration path, with source path, destination,
  ownership category, consumer, runtime prerequisite and activation requirement.
  Omitted paths carry an explicit reason; unresolved paths are release blockers.

- [x] **Step 1: Establish a clean source identity**

Record HEAD and the inspected merge identity; inspect tracked changes and preserve all
untracked work. Use a new authorized feature branch before committing package work;
do not continue publishing new work to the merged PR's branch. Do not rebase or reset it.

- [x] **Step 2: Enumerate inputs using tracked paths**

Use `git ls-files` rather than directory globs that hide dot-directories. Include symlinks
and their targets in the inventory, but do not follow a link outside the checkout.
Enumerate Cargo binary targets from manifests and surrounding source without editing them.

- [x] **Step 3: Trace consumers and classify each path**

For each candidate, identify real callers and classify runtime, optional integration,
security-sensitive configuration, user state or development-only material. Inspect runtime
paths for fixed homes, checkout assumptions and external resources. Review all five failure
conditions above and record a later entry-point regression case for each affected owner.

- [x] **Step 4: Separate dependencies and component identity**

Record external commands, interpreters and linked native libraries actually required by the
payload; distinguish observed tool versions from compatibility promises. Record exact existing
Greywall/Greyproxy pins, but do not choose replacement artifacts or execute build scripts.
Identify required Arch package-name research without guessing dependency names.

- [x] **Step 5: Validate the inventory against source**

Every destination must map to an inspected tracked source or an explicitly identified built
artifact. Every omitted required helper is a blocker. Confirm no enabled-unit link, learned
grant, credential or whole-home Stow operation is included. Confirm proposed private layout
preserves wrapper-relative lookup. Documentation checks must pass; no product test claim is
made for this documentation-only slice.

- [x] **Step 6: Commit only the inventory and approved planning files**

Show the diff stat, run the affected Markdown/link checks through existing admission,
and commit only the reviewed documentation on the package feature branch. Report remaining
runtime blockers and propose the next bounded implementation plan. No push or deployment
is implied by the local commit.
