# Rust Module and Crate Boundaries Implementation Plan

> **For agentic workers:** Use executing-plans for approved inline execution or
> subagent-driven-development only if Theo selects delegated implementation.
> This document authorizes neither implementation nor execution of its commands.
> Unchecked steps are proposed work, not evidence of completion.

**Goal:** Make Cockpit easier to navigate and change, first through cohesive Rust
modules (A), then through a real headless dependency boundary (B).

**Architecture:** Keep explicit concrete types and feature-owned modules. A reorganizes
existing methods without changing their behavior or ownership. B proposes one new
`cockpit-core` library below the existing UI/binary package; it does not introduce
service/repository layers, a generic event framework, or a crate for every feature.

**Tech Stack:** Existing Rust 2024 workspace, Ratatui/Crossterm, SQLite, Python graph
guards and Make verification. Reuse existing dependencies; manifests still need approval.

**Spec:** Theo's request in this session: plan both A and B from main, update the plan
as main changes, and touch no product code now. Design decisions and proposed move
maps are included here rather than in a second planning document.

**Related distribution roadmap:** [First packaged Linux release milestone](https://github.com/Androlax2/agent-cockpit/milestone/2)
addresses CI-built packages, Greywall/Greyproxy composition, `config/**` ownership,
public project documentation and GitHub milestones/issues. Those are separately gated
productization changes, not permission to alter runtime behavior inside A/B.

## Status and source of truth

- Original status was planning only. Theo's latest instruction now authorizes inline
  refactor implementation and routine verification after the original performance-worker
  checkpoints are collected and verified. Installation, dependency changes, security,
  protected Git and host execution retain separate approval. No deployment is authorized.
- Existing public scope is issue [#3](https://github.com/Androlax2/agent-cockpit/issues/3)
  (A1–A5), followed by [#4](https://github.com/Androlax2/agent-cockpit/issues/4)
  (gated B1–B3). Both are open with no GitHub assignee at this inspection; absence of
  an assignee does not establish that another session has released ownership.
- Current original session is `ses_e7e1238ddfferBKBXaGITxBLCs`. Fresh supported team
  context reports `task: null`, `members: []`, `links.approved: []`. Current runtime
  ownership of another refactor session and original worker artifacts remain unverified.
- Subsequent fresh TODO inspection found no matching #3/#4 implementation task. Under
  Theo's explicit instruction, recorded task `5b93b063` (saved request `a660e42890b1`)
  and started it in this original session. No new worker was created. Team context's
  null binding is not evidence of a second owner or authority to replace one.
- Execution checkpoint: A1 is committed as `2239c62` after verified local quality checks,
  Python220/Rust3266 tests (zero skipped) and independent review. A2–A5 is committed
  as `122e431` after final makeci passed Python222/Rust3267 tests (zero skipped) and
  independent review. Issue #3 is locally implemented; public delivery/closure is not claimed.
  Theo subsequently approved the five manifest/lockfile paths for internal core extraction.
  B1 is committed as `8b1e98d`; B2/B3 are committed as `eb1cbc0` after final makeci passed
  Python225/Rust3269 tests (zero skipped), graph checks and independent whole-branch review.
  No push or deployment occurred.
- Repository verified from the running copy's remote: `git@github.com:Androlax2/agent-cockpit.git`.
- Baseline: GitHub main `f04897d2d17eab279d0c250858430599ef12781f`, merge of PR #1.
- Planning branch: `docs/rust-architecture-plan-20261029`, created from that exact main.
- This clone's configured `origin` points to `/home/theo/agent-cockpit`, not GitHub.
  Never silently substitute its main for GitHub main or rewrite the remote configuration.
- The running copy may receive other owners' applied work before it reaches GitHub.
  Such work remains an ownership/compatibility hold even when absent from this baseline.

### Main-update procedure

At the beginning of each resumed planning turn and immediately before presenting a
revision, read GitHub main's SHA using existing authorized access. No background watcher
is installed; this session cannot promise unattended updates between turns.

If the SHA changes:

1. Record old/new SHAs and inspect the changed paths and owning tasks. Read their current
   guidance; do not assume old line ranges, signatures, test filters or ownership still apply.
2. Synchronize only this unpublished planning branch, preserving the plan and unrelated
   local files. Any Git/host operation remains within current permission and host review.
   If synchronization would overwrite work or rewrite published history, stop for a decision.
3. Reconcile the move map, callers, test inventory, dependency graph and ownership table.
   No automatic conflict resolution, source edits or scope expansion.
4. Add a row to the revision ledger and mark affected approvals stale. An updated plan
   is not authorization to implement its new revision.

| Revision | GitHub main baseline | Changes and verification |
| --- | --- | --- |
| 1 | `f04897d2d17eab279d0c250858430599ef12781f` | Initial source-only plan; execution checks not run. |
| 2 | `f04897d2d17eab279d0c250858430599ef12781f` | Incorporated pinned OpenLogi research, optional structural guards, workflow/hygiene follow-ups and current pane ownership release. No implementation checks run. |
| 3 | `1c56fd7082db0b1d6077425b55277f4864f3a287` | Public main comparison found identical source trees and no changed files; added separately gated package/config/portability roadmap. Local planning checkout remains at the earlier, content-identical baseline; no Git synchronization performed. |

Final main recheck: host read receipt `30c65322f49d` returned the same SHA after drafting;
no newer GitHub-main commits required reconciliation in revision 1.

Revision 2 independently read GitHub main through its public unauthenticated REST API;
the SHA was unchanged. The sandbox GitHub CLI's 401 is not a blocker to these public
metadata reads. Authenticated Actions logs may still require separately reviewed host access.

Revision 3 observed GitHub's merge reconciling applied/GitHub histories. Both baseline
and new main resolve to tree `54c980911930c48ace4103793159d84fe56f0f0d`, so the A/B
source move map is unchanged. This is a source-equivalence observation, not renewed CI
success or release authorization. Refresh main/ownership again before implementation.

## Global constraints

- Preserve `apps/Cargo.toml`, `apps/target/release`, installed command names, Make entry
  points, package-local binary discovery and existing compatibility import paths.
- Preserve wire fields, serialized text, validation, database schema, connection and
  transaction ownership, session/runtime identity, sender checks and host approval owners.
- No UI redesign, key/copy/color change, scheduling optimization, error-policy change,
  dependency-version update, dead-code deletion or repository cleanup in these refactors.
- No arbitrary line-count target. Large linear functions may remain; each extraction
  must have a named responsibility and a concrete navigation or dependency benefit.
- Keep data validation at its existing boundary; do not use new defaults to mask errors.
- No trait-per-type architecture, service classes, generic managers, or callback framework.
  Use plain functions, existing enums and explicit calls.
- Retain maintained docs, design decisions, FigCraft/tokens/previews and active-worker
  documents. This plan grants no ownership of another worker's files.
- Future manifest/lockfile changes require explicit approval; no install, security or
  host-operation approval is implied. Theo's later instruction authorizes routine #3
  implementation and verification in this clone, not B's dependency changes.

## Evidence and review focus

At the baseline, `apps/cockpit/src` contains 71 Rust files / 76,487 lines, including
20,935 lines in `tests.rs`. `app.rs` has 9,914 lines and Chief store has 5,873.
These counts identify navigation hotspots, not correctness failures.

1. Input/overlay ownership at widths 40/60/100/130 and small heights: paste, resize and
   shortcuts must not escape the composer or change the selected identity (A1/A2/A4).
2. Session refresh vs painting: repaint must not persist continuity, spawn work or
   change restart/sleep/unknown-team eligibility (A3).
3. Cross-session draft recovery, cancellation and atomic JSON/link invalidation must
   retain their existing locks, file bytes and rollback behavior (A2/B1/B2).
4. Cross-crate public types must retain a single identity; shared Chief/Team SQL calls
   must use the same connection and preserve `Storage` errors (B2/B3).
5. Tests and binaries must still be discovered from the correct package/root; moving
   fixtures must not skip tests or silently select installed executables (A1/B2/B3).

### Existing strengths to keep

- Real App/Screen and public CLI/store tests, isolated homes and fake host operations.
- `cockpit-protocol` excludes UI/SQLite dependencies; the daemon already excludes GTK.
- Make owns verification, admission protection and the shared full-suite lock.
- Existing private `app/{chief_tab,chief_ui,continuity_ui,new_session,teams_tab}.rs`
  establish the module pattern to extend rather than replace.

### OpenLogi evidence and adoption decisions

Research resolved OpenLogi `master` to
`d3af987d8353594587c88513fac885f26c43f2bf`; all references below are immutable.
Actual source/configuration was inspected. No OpenLogi CI run was inspected, so this
comparison establishes implementation patterns, not a claim that its checks pass.

| Observed OpenLogi pattern | Evidence | Cockpit decision |
| --- | --- | --- |
| Concrete state owners and private responsibility modules | [desktop state](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/crates/openlogi-desktop/src/state.rs) | Adopt private module seams in A; do not combine mechanical moves with a new state model. |
| Shared fixture with behavior-specific publication tests | [orchestrator tests](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/crates/openlogi-agent-core/src/orchestrator/tests.rs), [publication cases](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/crates/openlogi-agent-core/src/orchestrator/tests/publication.rs) | Preserve observable entry points and distinguish unknown, observed-empty and unavailable state in A1/A3. |
| Save/rollback ownership and explicit runtime orchestration | [configuration state](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/crates/openlogi-desktop/src/state/config.rs), [runtime](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/crates/openlogi-desktop/src/runtime.rs) | Preserve persistence-before-publication and request identity where Cockpit already requires them; do not transplant another storage implementation. |
| Path-scoped ownership checks with ast-grep | [rule registration](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/sgconfig.yml), [protocol owner rule](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/crates/openlogi-ipc/.ast-grep/rules/ipc-protocol-version-owner.yml) | Evaluate one concrete A5 rule; retain Cargo metadata for transitive crate boundaries. |
| Named checks, explicit skip reasons and aggregate outcomes | [CI runner](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/xtask/src/commands/ci.rs), [CI guidance](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/.agents/rules/ci.md) | Keep Make canonical; propose reporting and affected-consumer guidance in W1/W2, not a new task runner. |
| Explicit local artifact exclusions and durable decisions | [ignore policy](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/.gitignore), [decisions](https://github.com/AprilNEA/OpenLogi/blob/d3af987d8353594587c88513fac885f26c43f2bf/docs/DECISIONS.md) | Classify repository content in W4; retain durable rationale and maintained design evidence. |

Do not copy OpenLogi's hardware/platform crate count, async/RPC stack, GPUI state
framework, xtask, Nix packaging, auto-fixing hooks, or full platform matrix. Its core
has feature-dependent filesystem/locale capabilities; a crate named core is not proof
of purity. Cockpit's actual production graph is the acceptance criterion for B.
Caching and additional platform checks wait for measured need after CI bootstrap works.

### Known independent CI blocker

PR #1 head `0ac8a0b66a687233cababf6e16a06c5f19bae5a8` failed run `40894463644`.
Merged main failed run `40894662268` at the baseline SHA. Both failed job `quality`
at pinned `taiki-e/install-action`, before `make ci`:

```text
install-action: cargo-nextest is supported but version 0.9.150 for 'x86_64_linux' is not supported (updating install-action might resolve this)
```

Host read receipts: `1f51debdce12`, `902c4669e94d`, `ee20fb577c73`. This establishes
a reproducible CI bootstrap incompatibility, not a product test regression or passing
GitHub checks. Resolve separately with approved scope before claiming a reliable
refactor gate. No workflow edit, tool downgrade or rerun belongs to this plan-only turn.

## Active ownership holds

These are known holds, not a complete or permanent ownership registry. Refresh supported
team/todo context before implementation. A closed task alone does not release shared hunks.

| Task | Protected area | Planning rule |
| --- | --- | --- |
| `c005677d` | Daemon registry/tests and GTK presenter pane reconciliation | Refactor owner has explicitly released its own claim on the bounded pane scope; diagnostics overlap still requires sequencing. No new refactor work may preempt the original pane worker. Delivery route is currently unavailable; see release record below. |
| `8e473428` | Diagnostics modules, library exports, Chief/Team store instrumentation | Sequence export/store/crate moves after its checkpoint or explicit bounded owner release. |
| `56954d2c` | Sidebar maintenance/team snapshots | A3 and team/sidebar moves wait for current owner's exact hunk agreement. |
| `153066df` | Chief database reconciliation | Chief/store moves wait for checkpoint or explicit release. |
| `614fa5a0` | Phone conversation caching | Remote module moves wait for checkpoint or explicit release. |
| `d021dc46`, `0990586d` | Gaming/admission and continuation contracts | Preserve all guards and documents; no ownership release beyond earlier whitespace edits. |
| `b77a0881`, `1f838f53`, `5a82d5dd`, `b90b30c5` | Ongoing Chief/UI/recovery work and shared App methods | Recheck current ownership; do not treat an old implementation checkpoint as completion. |

No new worker is recruited to replace an existing owner. Coordination cannot grant
Git, manifest, security or host authority. If an owner cannot be reached, the task stays held.

**Current pane release record:** the refactor owner has no remaining ownership hold on
original worker `ses_e7887d312ffeQqLOWfSFOgHKCk` / task `c005677d` for daemon
`prune_dead`, `reconcile_with_panes`, capture/result identity, matching tests/lib export,
optional panes module and OSD activate scheduling/completion. This releases only the
refactor owner's claim, not diagnostics ownership or implementation authority. The latest
supported `cockpit-team context` returned `links.approved: []`, `members: []`, `task: null`;
no handoff was delivered through a stale historical link. Existing delivery blocker:
`a52f6bdb`. Failed CI, resource admission and host approvals are separate from ownership.
Before B3, coordinate with the original pane owner rather than assuming its scope is free.

### Required checkpoint consolidation before implementation

Theo directed consolidation into this existing plan/issues, not resumption of parallel
performance patches. The following checkpoint details are supplied reports, **not verified
artifacts or test evidence**. Fresh todo reads confirm all four tasks still appear doing;
their notes contain the original audit, not artifact paths, hashes or verification receipts.

| Original task/session | Reported preserved checkpoint | Evidence required before consolidation is complete |
| --- | --- | --- |
| `56954d2c` / `ses_e77fa24c8ffeRHYUsXdbQaBgjG` | Three sidebar regression drafts; offline base64 dependency failure | Exact clone/base SHA, draft paths/diff, preserved failed command/log, current owner handoff and no concurrent edits |
| `153066df` / `ses_e77f9af10ffecheq9Cnnww4gFt` | Observe-persistence slice: 25 additions / 4 deletions; 246-line file with seven tests. Reported RED 3 pass / 4 fail; GREEN 7 pass; compatibility 231 pass; no skips | Inspect actual source/test artifacts and diff, base/working identity, exact receipt/logs and commands; retain original RED and GREEN evidence, not only this summary |
| `614fa5a0` / `ses_e77f939b6fferStRjEmTQT2OAi` | Phone regression drafts | Exact clone/base SHA, test draft paths/diff, any commands/receipts and current owner handoff |
| `c005677d` / `ses_e7887d312ffeQqLOWfSFOgHKCk` | Pane checkpoint has no implementation | Owner-confirmed clean/no-implementation checkpoint, any preserved design/test artifacts and current scope handoff; do not infer cleanliness from silence |

Collect through current supported coordination or owner-provided accessible artifact paths.
Do not reuse old links after runtime changes, restart/recruit replacement workers, traverse
unapproved private clones or treat reported counts as verification. Keep queues, receipts,
uncommitted/untracked files and source holds intact. No sites-crema investigation, unrelated
cleanup/UI/autonomy work or Stadeo recovery changes belong to this refactor.

**Current blocker:** team context exposes no approved links or teammates, so this session
cannot obtain the original workers' current checkpoint paths/receipts or verify another
live refactor owner through that route. Existing coordination blocker `a52f6bdb` records
the pane route failure. No implementation starts before this precondition is satisfied.
After verification, record each checkpoint's artifact hash/base/receipt here and map it
to A3/B store/remote/daemon scopes without silently resuming its separate performance task.
The first implementation task remains A1's existing-test inventory and behavior-group
partition, not a new roadmap or an unreviewed performance patch.

**Updated access evidence:** Chief relay can supply checkpoint artifacts without direct
links. Ordinary reads of its three approved child roots all returned `File not found`:
`/home/theo/IdeaProjects/agent-cockpit/.handoff/chief-reconciliation-153066df`,
`/home/theo/IdeaProjects/agent-cockpit/.handoff/sidebar-56954d2c`, and
`/home/theo/IdeaProjects/agent-cockpit/.handoff/phone-cache-614fa5a0`.
Access blocker `cecd22b6` records exact errors. No alternate-path/host workaround was
attempted. Exact accessible artifact paths and hashes/receipts are needed before A1.
Pane relay reports no tracked edits on excluded `585b1f2`; retain requirements only,
never integrate that baseline. Database source/test reports remain unverified until read.

**Preserved-artifact update:** Theo-reviewed host request `74d344138d07` exited 0 and
copied only the specified artifacts to this clone's new nonoverwriting directory
`.handoff/refactor-checkpoints-5b93b063-v1/`. All three HEADs matched full `20748511`.
Its `manifest.json` and `SHA256SUMS` preserve file identities, diffs and reported receipt IDs.
No worker source, checkout, queue or runtime was changed and no patch was applied.

- Database source SHA256 `9a23d04a58e4a8bb4f8f15b03c87d950593c2359f07005f59e9dcfa846767eb2`
  and 246-line seven-test file SHA256
  `7bb9dc0910c642a3e6a4f4856966187e5e1f71fc0495c4b9c672c8c2ee61b5f0`
  matched the relay. Verified diff is +25/-4, hash
  `f35fe2bab3367443d1be02dcf45b9ac523462612b6e05d8430110ce7f0740a6d`.
  Read the actual patch/tests: differential row persistence in the existing transaction;
  tests cover unchanged rows, timestamp/runtime replacement, removal, rollback and reopen.
- Sidebar +170-line diff hash
  `6784b5671b63804690e9a3de8a7d0a39d851e0053fc762ec5fad8161ba3ef4ed`
  matched. Preserved the 80-line plan, hash
  `9af986f2faa3495e747753e30ed43751188cb9e7f0dfa7a66584863cda9d0740`.
  Read three actual regression drafts for stalled IPC, fetch count and stale endpoint results.
- Phone +165/-1 diff hash
  `f447e18efff5ad745cafc525eae3b76d6f9b619354ff03921ca6fcaa9ba96eb4`
  matched. Read eight Rust regression drafts and the visibility/polling JavaScript draft.
  No phone implementation or test result is claimed.

**Evidence still outstanding:** the database RED/GREEN/compatibility and sidebar failed
test outputs are identified only by the relayed `sh_...` receipt IDs; no original log
content/path was copied or independently inspected. Do not claim their reported outcomes
verified. Chief must relay those bounded logs or exact accessible log paths. Artifact
preservation is verified; full checkpoint evidence consolidation remains pending.

**A1 read-only inventory started:** source scan found 732 `#[test]` functions in
`apps/cockpit/src/tests.rs`, 2 in existing `tests/session_link_saves.rs`, 20 in
`tests/teams_chief_tabs.rs`, and 2 inline in `app.rs`. This is a source inventory, not
runner discovery or a passing test run. No test has yet been moved or renamed.

**Superseding consolidation/implementation update:** copied test outputs are now accessible
under `.handoff/refactor-test-logs-5b93b063`. Each file's SHA256 matches its manifest;
actual log contents confirm database RED3 pass/4 fail/0 skip, GREEN7 pass/0 skip,
compatibility231 pass/0 skip, and sidebar metadata failure before test execution.
Historical receipt IDs remain recorded; this is verification of copied evidence, not
a new execution of the worker commands. No preserved performance patch was applied.

A1 implementation has begun in the original task/session: first test-only partition
moves two unchanged continuity deferral tests from `src/tests.rs` into child module
`src/tests/continuity.rs`, retaining parent fixtures/helpers. Focused --offline invocation
failed on missing cached base64 before execution; normal --locked retry succeeded
(2 passed,867 filtered/skipped). Then the full cockpit library run passed869 tests,
0 skipped. Logs and exact commands/outcomes are preserved in `.handoff/` and the ledger
`.handoff/refactor-5b93b063-progress.md`. No workspace-wide success is claimed.

## A — Cohesive modules, unchanged package layout

### A1: Partition tests before moving production responsibilities

**Files:** modify `apps/cockpit/src/tests.rs`; create child modules under
`apps/cockpit/src/tests/`: `todo_ui.rs`, `sessions_ui.rs`, `session_lifecycle.rs`,
`continuity.rs`, `menus.rs`, `reviews_ideas.rs`, `layout_input.rs`, `storage_lifecycle.rs`.
Keep existing `session_link_saves.rs` and `teams_chief_tabs.rs` separate and intact.

Implementation move-map refinement: the existing behavior inventory also contains
cohesive status/usage, team/link, subscription/queue and error/audit groups. Place those
in `status_usage.rs`, `teams_links.rs`, `subscription_queue.rs`, `errors_audit.rs` rather
than recreating a mixed large storage/test module. This changes test organization only;
no new production feature, permission or state abstraction is introduced.

**Interfaces:** retain the parent `Fixture`, its environment lock and scheduler-server
shutdown/join ownership; child modules use parent test helpers. Preserve production
privacy and package ownership. Do not expose private APIs solely to relocate tests.

- [x] Inventory every existing unit test by full name and owning responsibility, including
  parent helpers and inline app tests; record before/after identities in review evidence.
- [x] Move one cohesive group at a time without assertion changes or renaming test functions.
  Module prefixes may change; account for them explicitly in selectors and inventories.
- [x] Keep tests grouped by behavior (publication, lifecycle, rollback, input), not merely
  by the private function they call. Each group continues through the real App/store entry
  points and the existing common fixture; no mock-only replacement for observable behavior.
- [x] Keep `TODO_SAVE_BARRIER`, `pause_todo_before_json` and their transaction regressions
  together in the parent for A; they require deliberate relocation only in B2.
- [x] Run affected test groups and verify no existing test is lost, ignored or disabled.
  Retain fixture panic/unwind regressions and existing environment serialization.
- [x] Independently review this test-only checkpoint before production moves. Commit only
  under implementation approval; checkpoint `2239c62` was authorized by Theo's later request.

### A2: Extract Todo panel and composer coordination

**Files:** modify `apps/cockpit/src/app.rs`; create `apps/cockpit/src/app/todo_tab.rs`;
exercise A1's `tests/todo_ui.rs` and existing `apps/cockpit/tests/todo_terminal.rs`.
`todo_composer.rs`, `todo_editor.rs` and `todo.rs` retain their current owners in A.

**Move set:** `build_todo`, `todo_fixed_lines`, `todo_subtitle`, `todo_owner`,
`todo_category_filter`, `reveal_todo`, `show_todo_tab`, `switch_todo_tab`, `handle_todo`,
`toggle_todo_status`, `hand_to_claude`, `hand_to_new_session`, `viewed_session_for`,
`todo_hints`, `handle_composer`, `todo_context`, `todo_details`, and Todo-only constants.

**Interfaces:** keep `impl App`, fields and all current signatures. In particular retain
`handle_todo(&mut self, key: Key, screen: &mut dyn Screen) -> String` and the public
handoff methods. Use `pub(super)` only where the parent needs a formerly private method;
do not widen fields or crate APIs. Keep global input precedence in `App::handle`.

- [x] Pin behavior through existing composer/cancellation/paste/resize/cross-session
  regressions; add a characterization test only for an uncovered move boundary, before moving it.
- [x] Move the named methods mechanically into a private child module. Preserve save,
  recovery, handoff and error ordering exactly; no new Todo controller/state container.
- [x] Verify existing tests including `todo_composer_add_saves_one_complete_item_without_external_editor_or_picker`,
  `todo_composer_cancel_preserves_disk_filters_queue_and_proposal_metadata`,
  `todo_composer_keeps_focus_and_draft_at_all_terminal_sizes`, and `todo_terminal`.
- [x] Review a move-only diff and test inventory before the next extraction.

### A3: Name the session observation and lifecycle boundary

**Files:** modify `apps/cockpit/src/app.rs`; create `app/session_refresh.rs` and
`app/sessions_tab.rs` beneath that directory; test A1's session/continuity groups.
Existing `app/continuity_ui.rs` stays the owner of its current methods.

**Move map:** `session_refresh.rs` owns `gather_sessions`, `settle_exits`,
`hold_restarting_cards`, `add_asleep_cards` and their existing observation-specific
private data. `sessions_tab.rs` owns `build_sessions`, `current_notices`,
`session_attention`, `session_card`, `has_new_result`, `card_badges`, `card_meta`,
`stopped_blocker`, `session_dependency_activity`, `session_task_title`,
`session_display_title`, `selected_session_detail_lines`, `session_summary_lines`,
`sessions_hints`, `sessions_help`, `handle_sessions`, and their card-only helpers.

**Interfaces:** existing signatures and App ownership remain unchanged. `tick`,
`maintain`, `refresh`, `redraw`, dispatch methods and `render` remain orchestrators
in `app.rs`. Keep the precise `gather_sessions`/settlement/build sequence; method
extraction must not turn lifecycle work into an extra per-frame operation.

- [x] Preserve original sidebar/Chief checkpoints and verified pause/handoff evidence;
  no performance implementation was applied or original-worker source changed. The A3
  method-only move does not edit Chief/store or the separate sidebar test draft.
- [x] Confirm existing regression coverage for no writes during render, restart identity,
  asleep cards, unknown membership and queued work. Add any missing entry-point
  characterization before moving the associated block; do not optimize it here.
- [x] Pin the distinction between not-yet-observed, observed-empty and unavailable team/session
  data through existing entry points. Unknown membership must continue to block sleep;
  stale or unavailable observations must not clear an existing request or target a replacement
  runtime. These characterize current behavior, not a new status enum or polling policy.
- [x] Move observation and presentation methods with exact-body inventories and focused
  verification. A2–A5 share final checkpoint `122e431`; A1 is separately committed.
- [x] Verify `continuity_notes_preserve_unknown_state_and_are_not_written_during_render`,
  `a_session_restarting_in_place_keeps_the_viewer`,
  `a_running_session_drops_its_stale_sleep_note`,
  `unknown_membership_blocks_manual_and_automatic_sleep`, and session/queue tests.
- [x] If gathering and presentation cannot be separated without changing behavior,
  keep their invocation coupled and document it; a prettier file is not a reason to reorder effects.

### A4: Extract the remaining panel responsibilities, not arbitrary chunks

**Files:** modify `app.rs`; create its private `status_panel.rs`, `reviews_tab.rs`,
`ideas_tab.rs`, `menus.rs`, `render.rs`, `input.rs` child modules. Test A1's matching groups.

**Move map:** Status build/subtitle -> `status_panel`; review build/count/subtitle/input ->
`reviews_tab`; idea build/source/input -> `ideas_tab`; existing Effort/Settings menu types
and their drawing/handling/pairing methods -> `menus`; layout/panel/header/tabs/footer/help
drawing -> `render`; `Key`, `read_key`, line editing and click decoding helpers -> `input`.
Do not move team/Chief methods out of their existing feature-owned children in this task.

**Interfaces:** retain `cockpit::app::{App, Key, Screen, read_key, color, style, fit,
put, width_of, project_color}` and other existing public names with explicit re-exports.
Keep `App::handle` and overlay precedence as the readable root dispatcher. Internal
re-exports use the narrowest visibility required by existing sibling modules/tests.

- [x] Start with Status/reviews/ideas, then menus, then paint/input helpers, one reviewed
  checkpoint at a time; skip a proposed file if it has no cohesive body after earlier moves.
- [x] Keep exact strings, colors, geometry, hit testing, raw-mode cleanup and input timing.
- [x] Exercise screen-entry tests, `chief_ui`, `team_ui`, `guide_terminal`, `todo_terminal`
  and `sidebar_refresh` as their consumers are affected. No visual redesign is proposed.
- [x] Verify the root app module now explains state and orchestration instead of hiding
  them behind generic forwarding layers. Keep an honest method-to-owner map in review evidence.

### A5: Remove the small reverse dependencies required by B

**Files:** create `apps/cockpit/src/background.rs` and `todo_editor/text.rs`; modify
`lib.rs`, `app.rs`, `app/refresh.rs`, `status.rs`, `costs.rs`, `reviews.rs`, `todo_editor.rs`.
Test the existing app/draft/refresh tests; add `tools/tests/test_module_boundaries.py`.

**Interfaces:** move `background_changed() -> ()` and `take_background_changed() -> bool`
from `app::refresh` to `background`, preserving their single atomic and Release/Acquire
ordering. Keep old public paths as re-exports of the same functions, never duplicate flags.
Move `editor_text(title: &str, notes: &str, why: &str) -> String`,
`parse_editor_text(text: &str) -> Option<(String, String)>` and `EDITOR_HELP_MARK`
to `todo_editor/text.rs`; preserve existing crate-private app paths with re-exports.

- [x] Add a source boundary test rejecting non-UI `status.rs`/`costs.rs`/`reviews.rs`
  references to `app::refresh`, and `todo_editor.rs` references to `app` for text formatting.
  Observe the expected RED before changing production call sites.
- [x] Move the functions, retaining exact bytes, trim behavior, ordering and wake coalescing.
- [x] Verify boundary tests GREEN plus draft recovery/cancellation and existing refresh tests.
  A signal followed by two consumes must yield true then false; multiple signals coalesce.
- [x] Document A's remaining shared App state. Do not claim method relocation alone
  proves independent components or a headless library.

**Optional ast-grep evaluation, not a required dependency:** compare the small Python
guard above with a structural rule for the same observed reverse dependency. The default
remains the existing Python toolchain unless a reviewed rule demonstrates a concrete
reduction in false matches or maintenance. Evaluation/install/adoption needs separate
approval; this plan does not add ast-grep to Cargo, CI or local hooks.

If approved, proposed files are root `sgconfig.yml`,
`.ast-grep/rules/background-notification-owner.yml`, and
`.ast-grep/tests/background-notification-owner.yml`; the existing Python architecture
test orchestrates path coverage rather than maintaining a second competing rule.
Test an allowed owner/re-export path, forbidden consumer call/import paths and unrelated
code/comments/strings. Include explicit cases for differently formatted Rust paths and
aliases; document unresolved aliases and macro expansion as limitations, not coverage.
Verify real path selection, including hidden-directory discovery if rules later cover
hidden configuration. A snippet match alone does not prove a rule runs on the right files.
Do not use syntax checks to claim type resolution, transaction safety or transitive
dependency enforcement; Cargo graph checks and behavioral tests remain authoritative.

## B — One enforceable headless core boundary

### Design decision and approval gate

Proposed final production dependency graph:

```text
cockpit (existing UI, terminal adapters, compatibility exports and binary owners)
  -> cockpit-core -> cockpit-protocol
osd -> cockpit-daemon -> cockpit-core
osd -> cockpit                    (GTK executable adapters only, as required)
```

Daemon dev-dependencies may use `cockpit` for existing cross-layer App tests; production
dependencies must not. All existing protocol types keep one identity through re-exports.
No separate `cockpit-tui`, database crate or one-crate-per-feature is proposed now.
The new crate is justified by excluding Ratatui/Crossterm/GTK from daemon production
dependencies, not merely shortening a directory listing.

**B starts only after A, current-owner coordination, explicit approval of this move map
and manifests, and a usable baseline verification gate.** If headless separation cannot
preserve compatibility without an unresolved cycle or transaction split, stop at A and
revise this plan. Do not weaken the intended dependency guard to force B through.

### B1: Finish separating headless data from terminal adapters

**Files:** create `apps/cockpit/src/chief/summary.rs`, `approval/terminal.rs`,
`usage/terminal.rs`, `teams/summary.rs`; modify `chief.rs`, `chief/view.rs`, `costs.rs`,
`remote.rs`, `approval.rs`, `usage.rs`, `teams.rs`, `teams/view.rs`.
Review Chief/phone owners before touching these files.

**Interfaces and responsibilities:**

- Move Chief's pure digest helpers and their constants (`fallback_session_name`,
  `one_line`, `since_label`, `answer_summary_line`, `habit_summary_line`,
  `DIGEST_LINE_WIDTH`, `DEFAULT_LOOKBACK_MS`) and required pure helpers to `chief/summary.rs`.
  Preserve signatures; keep old `chief::view` public names through re-exports.
- Introduce proposed `chief::summary::status(snapshot: &serde_json::Value) -> String`
  by extracting the current `ChiefView::status` calculation. Both `ChiefView::status`
  and remote projection call it, preserving output for every existing status state.
  The core must not instantiate `ChiefView` merely to calculate a phone status string.
  Current `ChiefView::status` also depends on local `read_error`, not just its snapshot:
  retain its existing early stale-review error branch in the view wrapper before calling
  the pure snapshot helper. Characterize stale retained records explicitly. Extract the
  existing snapshot-only startup/worker-unavailability calculations, not approval gates.
- `habit_summary_line` currently reaches `teams::view::short_project` through `scope_name`.
  Move the existing pure `short_project` and its `shown` helper to `teams/summary.rs`,
  keeping their current bodies and crate-private compatibility paths via imports/re-exports.
  Point Chief summary code at that headless owner; do not duplicate escaping/trimming logic
  or leave a core-to-UI edge hidden behind the new summary module.
- Move terminal adapters `terminal_size`, `explanation`, `choose`, `choose_or_remote`,
  `edit`, `reserve_last_row`, private `draw_status`, and
  `run(command: &str, cwd: &str, log_path: &std::path::Path, request: Option<&str>) -> i32`
  into `approval/terminal.rs`, preserving their exact current signatures and original
  public approval exports. `run` executes through a PTY and calls `terminal_size`:
  leaving it in core would recreate a reverse dependency. Keep its command construction,
  environment, phase reporting, remote-input handling and cleanup byte-for-byte equivalent.
  Keep validation, redaction, request paths, remote-answer paths, prompt detection and
  approval authority in their existing non-terminal owners. Preserve shared constants
  and path helpers in core so terminal adapters call downward, never the reverse.
  Move terminal-only `TYPE_AHEAD` and `REMOTE_POLL` with their consumers; shared public
  constants such as `TERMINAL_RESET` keep a single canonical definition and facade path.
- Move `usage::report_forever() -> !` to `usage/terminal.rs` with its old public export.
  Keep `report_lines(columns)` and usage evidence logic independent of terminal size probes.

- [x] Add characterization for digest/phone status and draft formats before
  relocation; test stale/missing states through existing public consumers, not only helpers.
- [x] Compare `ChiefView::status` and phone status for Off/Starting/Checking/On/failure
  snapshots; preserve unknown-state behavior rather than supplying a convenient default.
- [x] Perform move-only adapter changes. Any required authority/permission behavior change
  stops the task for separate security approval; it is not part of this refactor.
- [x] Verify `approval_terminal`, `run_requests`, `guide_terminal`, `chief_ui`, `costs`,
  and existing remote tests. Preserve type-ahead guard, remote answer validation and raw-mode cleanup.
- [x] Inventory remaining production references to `app`, `chief::view`, Ratatui and
  Crossterm outside UI/adapters. Resolve every reverse edge before B2; no forbidden exception list.

### B2: Extract the core once, retaining explicit compatibility exports

**Create:** `apps/crates/cockpit-core/Cargo.toml`, `src/lib.rs` and the moved modules below.
**Modify:** `apps/Cargo.toml`, `apps/Cargo.lock`, `apps/cockpit/Cargo.toml`, original library
module/export files, `tools/tests/test_crate_boundaries.py`, `docs/ARCHITECTURE.md`,
`docs/DEVELOPMENT.md`. Every manifest/lock edit needs explicit approval first.

| Current source under `apps/cockpit/src/` | Proposed owner under `apps/crates/cockpit-core/src/` | What remains in `cockpit` |
| --- | --- | --- |
| `agents.rs`, `paths.rs`, `util.rs`, `background.rs` | Same module names | Explicit re-exports |
| `audit.rs`, `billing.rs`, `codex.rs`, `continuity.rs`, `costs.rs`, `errors.rs`, `failsafe.rs` | Same names, including existing child modules | Explicit re-exports |
| `prompts.rs`, `push.rs`, `queue.rs`, `remote.rs`, `reviews.rs`, `sessions.rs`, `settings.rs`, `sleep.rs`, `status.rs`, `subscription_fallback.rs` | Same names and children | Re-exports; remote binary/assets stay in existing package |
| `todo.rs`, `todo_editor.rs`, `todo_editor/text.rs`, `todo_images.rs`, `todo_requests.rs` | Same names/children; editor is draft persistence, not a renderer | Minimal draft API exports needed by composer; no duplicate model |
| `usage.rs` | Headless functions/types | `usage/terminal.rs` and explicit facade combining the two |
| `approval.rs` | Non-terminal helpers and request/path contracts | `approval/terminal.rs` and explicit facade; authority unchanged |
| `teams.rs`, `teams/{client,links,store}.rs`, `teams/store/` | Headless collaboration/client/store owners | `teams/{view,sidebar}.rs`, existing compatibility namespace |
| `chief.rs`, `chief/{bundle,store,summary}.rs`, existing store children | Headless Chief/client/store owners | `chief/{view,inbox}.rs`, existing compatibility namespace |
| `app.rs`, `app/`, `todo_composer.rs` | Not moved | Existing UI ownership, names and binaries |
| `bin/`, `assets/`, `tests/` integration targets | Not moved | Preserve binary discovery, assets and test fixture roots |

**Interfaces:** existing `cockpit::...` public types/functions remain imports of the
single core definition. Mixed namespaces (`chief`, `teams`, `approval`, `usage`) use
small explicit module facades with core exports and UI children; never a dependency
from core back to `cockpit`. Expose only the minimum previously crate-private draft
operations consumed across the new boundary, with documented signatures; no blanket
visibility widening of App internals, test helpers or stores.

- [x] Add graph regressions requiring a real `cockpit-core` workspace member and forbidding
  its production graph from reaching `cockpit`, `osd`, `cockpit-daemon`, GTK, Ratatui or
  Crossterm. They must fail before extraction. Preserve existing protocol/daemon guards.
- [x] Copy no models: relocate canonical definitions, keep protocol re-exports and preserve
  the Team/Chief shared SQL connection, migrations, transaction boundaries and `storage_error`.
- [x] Keep each persistence/rollback owner intact: failed writes must not publish successful
  state or send a reload/handoff, and acknowledgements must retain their original request
  identity. Use the existing rollback/link-invalidation tests to verify moves, not a generic
  repository abstraction or independently committing Chief/Team stores.
- [x] Relocate non-UI inline unit tests and their fixtures into core with their subject. Keep UI
  unit tests in `cockpit`. Specifically move the Todo JSON barrier hook and all four
  dependent transaction tests (two parent cases and `tests/session_link_saves.rs`) into
  core's `#[cfg(test)]` support with the same concurrency assertions;
  do not enable test hooks in production or depend on a dependency's disabled `cfg(test)`.
  Existing cache-only Reviews/status fixture helpers use a default-off `test-support`
  feature enabled through the terminal dev-dependency; verify actual normal production
  feature trees exclude it. This feature must not expose the transaction barrier or
  change authority, storage or request-validation behavior.
- [x] Split test-only fixture setup only where required by the package boundary, retaining
  environment locking and shutdown ownership. Keep `common/link_fixture.rs` usable via a
  verified checkout-root path or a reviewed test-only copy; no new fixture package by default.
- [x] Verify `CARGO_MANIFEST_DIR`, inline fixtures and root discovery explicitly for the
  deeper crate. Keep all CLI tests under the package that owns their binaries.
- [x] Verify graph guards, compatibility imports, serialized outputs, draft/SQL regressions,
  every moved unit-test group and existing CLI integration targets. Account for all old tests.
- [x] Update architecture/development docs with the implemented graph after verification.
  Do not modify Make, launchers, hooks, compose or CI merely to accommodate a cosmetic move.

### B3: Switch daemon production dependencies and prove the boundary

**Files:** modify `apps/crates/cockpit-daemon/Cargo.toml`, its `src/{lib,registry,chief,teams}.rs`
and any other current production consumers; preserve its existing test files and update
their imports only as needed. Modify `tools/tests/test_crate_boundaries.py` and approved
lockfile changes. Keep OSD binaries and `apps/osd/src/lib.rs` compatibility exports.

**Interfaces:** daemon uses `cockpit_core` for production services and
`cockpit_protocol` types through core's canonical re-exports. `cockpit` is at most a
dev-dependency for existing cross-layer UI tests. No protocol or host-operation change.

- [x] Add a daemon graph assertion rejecting production reachability to `cockpit`,
  Ratatui and Crossterm as well as the existing GTK rejection. Observe RED first.
- [x] Preserve pane/Chief checkpoint handoffs before moves. Diagnostics integration
  sequencing remains held for any later publication/handoff; its separate instrumentation
  was not overwritten or incorporated. Store bodies moved unchanged.
- [x] Replace production imports/dependency only; leave `System` injection, scheduling,
  report identity and SQL calls at their original owners. Observe graph guard GREEN.
- [x] Exercise daemon tests, OSD consumers and `team_client`, `team_protocol`, `chief_protocol`,
  `chief_store`, `team_store`, `chief_ui`, `team_ui`, `todo_terminal`, `sidebar_refresh`.
- [x] Independently review the dependency graph and compatibility surface, then run the
  authorized final shared gate once. Do not declare headless success from compilation alone.

## W — Separately scoped workflow and repository follow-ups

These OpenLogi-inspired proposals are not additional A/B implementation scope. Each
needs its own bounded review and authorization; no W source/config change is approved
by updating this document. Coordinate W1/W2 with existing workflow owner `5fd3baec`,
and W4 with README owner `a3fea48f` and all active document owners. Do not start duplicates.

### W1: Repair CI bootstrap, then expose independent check outcomes

**Proposed scope:** `.github/workflows/ci.yml`, existing workflow assertions in
`tools/tests/` where present, and `docs/DEVELOPMENT.md`. Inspect actual test coverage
before choosing a new test file; do not claim a workflow assertion already exists.

- [ ] Preserve the original PR/main failed-run evidence above. Verify a compatible immutable
  installer revision supports every currently pinned checker before proposing the exact
  workflow diff; do not invent a replacement SHA, downgrade tools or enable fallback silently.
- [ ] Submit the smallest bootstrap repair separately. A subsequent run is separately
  authorized; passing setup is not evidence that formatting, tests or other checks passed.
- [ ] After bootstrap works, measure each gate's duration and propose independent lightweight
  check reporting. Reuse Make targets; avoid two definitions of executable check commands.
- [ ] Report check states distinctly as passed, failed, blocked/not run or deliberately skipped
  with a reason. Independent checks may continue for diagnosis, but any required failure or
  missing prerequisite must leave the overall gate failing. Preserve serial heavy-test admission
  and the shared full-test lock; this is not approval for parallel heavy runs.
- [ ] Update stale development prose saying CI has never run with exact observed run/commit
  evidence. Do not replace it with an unverified success claim.

### W2: Document affected-consumer verification and constrain lint exceptions

**Proposed scope:** `docs/DEVELOPMENT.md`, `.agents/rules/ci.md`, `.agents/rules/rust.md`
and verify-cockpit guidance. Future lint-policy manifest edits require their own approval.

- [ ] Document complete reverse production/dev/test consumers rather than selecting only
  the edited package. Current baseline: protocol changes reach cockpit, daemon and osd;
  cockpit changes reach daemon/osd and executable tests; daemon changes reach osd and its
  cross-layer tests. Recompute this map after B; include package-local binary fixtures.
- [ ] Require the full authorized gate for workspace/lockfile/toolchain changes, unresolved
  impact or Rust-bearing conflict resolutions unless a reviewed equivalent proof covers
  the entire affected graph. A focused pass is never described as a full pass.
- [ ] Inventory existing lint exceptions before proposing enforcement. Prefer narrowly
  scoped, reasoned `#[expect]` where appropriate and supported; do not mechanically replace
  legitimate conditional exceptions or enable all pedantic lints in one batch.
- [ ] Keep ordinary formatting/check commands read-only. Optional hooks, if ever requested,
  are explicitly installed and documented; do not copy OpenLogi's modifying hooks by default.
  Defer weekly multi-platform suppression sweeps until actual debt justifies their cost.

### W3: Prove graph isolation instead of copying a workspace layout

**Scope:** B's existing Cargo graph tests and their fixtures; no new task runner, crate
or portability target. This is an acceptance refinement for B, not a second extraction.

- [ ] Evaluate resolved production dependencies, including feature-unified dependencies,
  not only direct manifest imports or default-feature claims. Separate dev-only cross-layer
  test edges from runtime edges explicitly, retaining one canonical protocol type identity.
- [ ] Keep installed binary and fixture compatibility as acceptance criteria alongside graph
  isolation. No arbitrary file count or maximum-line rule can replace these checks.
- [ ] Consider compilation on another target only if portability becomes an approved contract;
  Cockpit's current supported platform remains Linux. Do not add WASM merely because OpenLogi does.

### W4: Classify maintained content and scratch output without deleting history

**Proposed scope:** a small `docs/INDEX.md` linked from maintained contributor/development
guidance, plus narrowly justified `.gitignore` additions only for identified new local
scratch output. The existing cleanup proposal remains read-only and separately approved.

- [ ] Classify exact tracked paths as maintained reference, durable decision/design evidence,
  active owner-held plan, completed execution record or disposable local output. Link to
  replacements and identify owners where known; unknown status means retain, not delete.
- [ ] Keep completed design decisions, FigCraft files, tokens, referenced previews, fixtures
  and generated assets consumed by builds. Generated content is not automatically disposable.
- [ ] Distinguish Git retention, quality-check discovery and package/build inputs. Preserve
  `tools/quality.py` tracked/shebang discovery, hidden tracked files and tested exceptions;
  do not hide Markdown from checks or add Nix packaging to solve a navigation problem.
- [ ] Do not delete/move existing plans, broaden ignore rules over tracked design/docs,
  install hooks, or rewrite active workers' status. Any deletion proposal needs exact paths,
  reference repairs, explicit owner releases where needed and Theo's bounded-list approval.

## Verification and publication gates for future implementation

Commands below define verification; execution results are recorded separately. The original
planning turn ran none of them. Later routine #3 execution is explicitly authorized;
manifest/dependency, host and publication approvals remain separate.
Use the account-home admission runner required by the current environment. A denied
runner/lock is a blocker, never a reason to bypass it or launch duplicate broad runs.

| Scope | Future verification command / expected result |
| --- | --- |
| Focused App groups | `cargo nextest run --release --locked --manifest-path apps/Cargo.toml -p cockpit --lib -E 'test(todo_composer)'` (or the exact moved group's filter); selected tests pass with nonzero selection and no skipped tests |
| Existing real-terminal Todo entry | `cargo nextest run --release --locked --manifest-path apps/Cargo.toml -p cockpit --test todo_terminal`; all target tests pass |
| New A source guard | `python3 -m unittest discover -s tools/tests -p 'test_module_boundaries.py'`; fail before boundary changes, pass afterward |
| Cargo graph | `python3 -m unittest discover -s tools/tests -p 'test_crate_boundaries.py'`; real locked metadata validates production edges, dev-only UI edges excluded |
| Core after B2 | `cargo nextest run --release --locked --manifest-path apps/Cargo.toml -p cockpit-core --lib`; all relocated tests selected and pass |
| Rust consumer/style checks | `make fmt-check clippy doc`; all affected workspace members/targets pass |
| Full regression gate | `make ci`; uses the shared `make test` lock and classifier prerequisite, not parallel full runs |

Additional integration filters must use verified existing target names from this plan,
not invented commands. Ensure the real fixture binaries are built under explicit build
approval before focused tests that require them. Preserve original failures and distinguish
unavailable prerequisites, changed test paths, baseline failures and genuine regressions.
Local passes do not establish GitHub runner success or live deployment evidence.

Each implementation checkpoint needs an independently reviewable diff and actual command,
exit status, test inventory/count and outcome. A new architecture guard must demonstrably
reject the old forbidden edge. Pure moves use unchanged passing behavioral tests; do not
manufacture a red behavioral test by changing valid expectations.

When adopting a structural rule, record its owner/consumer/unrelated-path fixtures and
known matching limits. When revising CI reporting, test the failing/missing-prerequisite
aggregation path as well as all-green output. Neither change substitutes for the A/B
behavioral test inventory or the Cargo graph proof.

This document itself grants no permission to commit, publish or apply. Later requested
feature-branch checkpoints use the project's Git authorization; push/host approval is separate.
Future executable
changes to manifests, Make, launchers/hooks or other host-run files must be called out in
review. Pushes/PRs and reviewed host application remain separately authorized operations.

## Delivery order and stop points

1. Review this plan and current ownership; coordinate W1's separately approved CI bootstrap repair.
2. Approve/execute A1–A5 incrementally, retaining a working application after each checkpoint.
3. Re-measure coupling, refresh main and approve B's exact revised move map/manifests.
4. Execute B1–B3 in dependency order with independent review and graph evidence.
5. Consider publishing/applying only after separate authorization and verified final source.

W2 guidance and W4 classification may be proposed independently to their existing owners.
They do not block the released pane worker or authorize implementation of A/B. W3 is
part of B's eventual evidence; optional ast-grep adoption is not a prerequisite for A.

The packaged-release roadmap can proceed through its own approved design slices without
waiting for every B move. It must explicitly migrate the source-built live-copy workflow;
do not use this plan's behavior-preservation requirement to hide or forbid that later,
separately reviewed product change. Source compatibility and installed-path migration are
different acceptance gates.

Stop after A if B's dependency benefit no longer exists after incoming work, ownership
cannot be sequenced, or a required behavior change is discovered. Neither stage requires
deleting completed design decisions or introducing a new UI design.

## Planning review evidence

- Fresh builder review identified one omitted reverse edge: `approval::run` calls
  terminal-size probing and terminal painting. B1 now explicitly owns its adapter move;
  core retains the request/path/validation owners. No source change was made to resolve it.
- Plan self-review checked the proposed core/UI ownership against draft formatting,
  background notification, digest helpers, remote Chief status, terminal adapters and
  the Todo transaction test hook. B remains gated on a fresh exhaustive edge inventory.
- Original plan preparation ran no implementation verification; subsequent results below
  supersede that planning-only state. GitHub CI's original failure remains unresolved.
- Revision 2 incorporates three read-only OpenLogi research scopes (architecture,
  CI/workflow and repository hygiene), all pinned to the same source SHA. Recommendations
  distinguish observed source patterns from proposed Cockpit changes and preserve the
  existing source/host/manifest/ownership approval boundaries.

### Current execution evidence

- A1 source inventory/body hashes and independent review confirm 725 unchanged tests
  moved into 12 feature children,7 retained root tests, existing22 child tests unchanged;
  library869. No test assertion or fixture/lock/server/barrier ownership changed.
- A1 full local verification: Python220 passed, Rust3266 passed/0skipped. Original
  makeci120s harness timeout is preserved; checks preceding its interrupted OSD build
  passed, then shared maketest was resumed with a sufficient bound. No GitHub pass inferred.
- A2 Todo extraction:869 library+4 terminal tests passed/0skipped. A3 session move:
  869 library tests passed/0skipped. A4 panel/menu/render/input move:869 library and
  178 integration consumer tests passed/0skipped.
- A5 literal-source guards: RED4failures -> GREEN2tests. Library now870 tests, all pass,
  0skipped, including a compatibility-path singleton/coalescing test. Clippy passed.
- Fresh A2–A5 review found no important findings and verified199 original app function
  bodies unchanged. Minor limitation retained: source guards are not Rust alias/type
  resolution; production Cargo graph guards remain the B acceptance criterion.
- Final A2–A5 makeci passed, receipt `sh_188b68f31001dKhAUs4Z1WIhAC`;
  `.handoff/a2-a5-final-ci.log`: Python222/OK, Rust3267passed/0skipped. Commit `122e431`.
  Ledger `.handoff/refactor-5b93b063-progress.md` preserves commands, failures and rulings.
  #3 locally implemented; no push, public issue closure, #4 execution or deployment claimed.

### B approval and current evidence

The exact next permission is workspace/internal path-dependency and lockfile edits in
`apps/Cargo.toml`, `apps/Cargo.lock`, `apps/cockpit/Cargo.toml`,
`apps/crates/cockpit-daemon/Cargo.toml` and new `apps/crates/cockpit-core/Cargo.toml`.
This would introduce the planned local core crate and reassign existing dependencies;
no new third-party dependency, version upgrade or tool installation is requested.
Theo explicitly approved exactly those paths for the internal core and existing dependency
reassignment. No new third-party dependencies, version upgrades or installs were approved.
The manifest gate is satisfied; protected host/publication/security approvals remain separate.

B1 separated pure summary calculations and terminal adapters while retaining view-local
stale/pending state. Its 338 affected integration tests and Clippy passed. Core/Cockpit
libraries now pass870 tests/0skipped after B2/B3, with390 consumer tests/0skipped.
All five real graph/feature checks pass; before extraction the new core/daemon assertions
failed as expected. All294 third-party lock package records are unchanged. Fresh independent
whole-branch review found no important code findings or lost test names.

The first B final gate failed Ruff formatting, not tests; the exact line is corrected.
Final gate rerun (`.handoff/b-final-ci-2.log`, full receipt retained in the ledger)
passed: Python225 in31.331s/OK;
Rust3269 passed(1slow),0skipped in378.980s across79 binaries. Source checkpoint `eb1cbc0`
contains the tested extraction. This establishes local A/B implementation, not GitHub
runner success, publication or deployment. Source/worker ownership holds remain explicit.

### Final local delivery boundaries

- Existing feature branch `docs/rust-architecture-plan-20261029`; original session and
  checkout retained. Phase A commits `2239c62`, `122e431`, plan checkpoint `783fa09`,
  B1 checkpoint `8b1e98d`, core extraction `eb1cbc0`. No history rewritten.
- All294 external lock package entries, protocol identity, SQL transaction/durability
  ownership, executable paths and approval checks preserved. All original performance
  artifacts still pass their recorded checksums; no performance implementation was applied.
- Source guards remain intentionally literal-path checks, not Rust alias/type resolution.
  Optional ast-grep, packaging, unrelated cleanup/UI/autonomy and public rollout remain out of scope.
- GitHub issues #3/#4 remain open: no push/PR/merge, host execution or deployment approved
  or performed. Diagnostics task `8e473428` still appears doing; its sequencing and the
  explicit old-to-core path map must be preserved before any later shared integration.
