# Are all decision records needed?

Issue [#548](https://github.com/Androlax2/agent-cockpit/issues/548).
**Research and proposal, 2026-11-06. Theo chose the recommended option on all five
[questions](#questions-for-theo) the same day** (Torlek reply to the #548 worker, 2026-11-06:
"Q1 A, Q2 A, Q3 A, Q4 A, Q5 A"). Merging this document accepts research delivery only. It
deletes, merges, moves and rewrites no decision record; every slice in [Plan](#plan) is its own pull
request and waits for Theo's go-ahead. It builds on the
[documentation plan (#618)](2026-11-06-618-documentation-plan.md), where Theo chose 1B: decision
records stay in the repository and out of the public site. Counts below are for `81f41047`.

## Answer

No. About two thirds of the records earn their place; a third are fragments, status notes or
evidence write-ups that the criterion does not admit.

Theo's definition is the test: *durable "why we did it this way" records that are not obvious from
the code; a dated entry when a non-obvious architectural or dependency decision is made or revisited.*
Applied to all 165 files in `docs/decisions/` (the 156 that #618 counted, plus 9 merged since):

| Disposition | Records | Meaning |
| --- | --- | --- |
| Keep | 106 | Passes every test below. |
| Merge | 38 | Sound, but a follow-up or sibling of another record on the same topic; fold into that record as a dated "revisited" entry. |
| Supersede | 6 | Reversed or replaced, wholly (279, 94, 43, 433-observations) or partly (61, 754); keep for history, mark with a status line that says what still holds. |
| Relocate | 7 | Useful rationale that belongs in a test or module doc comment, a crate README, `docs/RELEASING.md` or `CONTRIBUTING`. |
| Remove | 8 | Nothing durable the issue, code or a later record does not already carry. |

If every row were executed the log would shrink from 165 to **112 files** (-32%), and every rejected
alternative kept except in the 8 removals, which git history keeps. The per-record table with reasons
and inbound links is in [`548/dispositions.md`](548/dispositions.md).

The structure is not the problem. One file per decision stays (see [Structure](#structure)); what
changes is what is admitted and how a revisited decision is written down.

## Method

1. **Inventory.** Name, size, issue number and date of every file; links to each record from code,
   workflows and docs (`git ls-files` search for the file stem); cited paths that no longer exist.
2. **Read.** Four reviewers each read 40 to 44 records in full against the five tests below and
   checked claims with `git grep`. I re-checked every Remove, the cross-chunk conflicts, and the
   contradictions listed in [What the reading found](#what-the-reading-found).
3. **Reconcile.** Where two reviewers gave conflicting dispositions across a chunk boundary
   (4 cases: 226, 284, 397-ci-cost, 433-live-enforcement-ci) I applied one rule: merge only inside one
   issue or one topic family, and never into a record that is itself merged away.

### The five tests

| Test | Question | Fails when |
| --- | --- | --- |
| C1 Non-obvious | Is the reason absent from the code, tests, comments and other docs? | A test name or one-line comment already says why. |
| C2 Durable | Does it still hold in `crates/`? | It describes `apps/`, a removed component (the sandbox engines) or a one-off task status. |
| C3 Kind | Architecture or dependency, or something else? | It is a test-harness technique, a PR work log, conformance evidence or CI tuning. |
| C4 Unique | Is the same reason stated in a design report, README or another record? | A README states it; then only the "Rejected" lines are unique. |
| C5 Revisited | Has a later record or the code reversed it? | A later record says so; the old one needs a status line, not silence. |

Kinds across the 165: architecture 93, behaviour 25, policy 21, implementation 14, dependency 11
(one more record is both dependency and architecture). Implementation and behaviour records are
where the merges, relocations and removals concentrate; architecture is 76 keeps of 93.

## What the reading found

1. **Topic families are split by issue number.** 31 issues have more than one record; #406 has seven.
   Stories read across several files: finished label (226, 273, 269), open question (182, 213, 214),
   blocking launch steps (272, 324, 325), desktop session (303, 305, 307), approval-pane keys
   (317, 337, 345), release packaging (562, 563, 565 each twice, days apart). A reader has to open
   three files to learn one decision.
2. **Amendments contradict their parent.** `2026-11-01-48-codex-hook-trust-and-honest-capabilities`
   says "Outcome is not reported"; `2026-11-01-48-codex-delivery-through-a-private-app-server`
   says "`Stop` now reports `--outcome completed`". `2026-11-01-47-opencode-server-and-plugin` says
   events were "not captured live"; #417 captured them. `2026-11-02-419-native-input-…` says
   "Codex's target remains 0.161.0"; `CONFORMANCE_VERSION` is `0.166.0`
   (`crates/cockpit-codex/src/lib.rs:30`). A stale record is worse than none: it is read as current. Each contradiction has its own remedy
in [Plan](#plan): the 48 pair merges (slice 3), 47 gets its stale bullet struck (slice 2), the
419 record is removed (slice 5).
3. **The sandbox-engine records are dead.** `2026-11-05-828-remove-sandbox-engines` states "Supersedes
   the engine parts of" 61, 476, 470 and 495. `crates/cockpit-sandbox/engine-pins.json`, the
   installer and the managed proxy no longer exist. Three of the four hold nothing else; 61 keeps
   a surviving principle ("refuse, never weaken").
4. **Eight records are evidence or status, not decisions.** Conformance write-ups (417, 419-native-input,
   436-codex-0166-pin), a 835-byte smoke-test note (493), a README task note (98), and the engine
   trio (above). Their provenance lives in fixture READMEs and `docs/PROVIDER_CONFORMANCE.md`.
5. **Seven are harness technique or PR logs** (36, 257, 300, 439, 191, 192-release-app-setup,
   436-approval-routing-evidence). `2026-11-02-191-candidate-release-gates` is 1,860 words of host
   approval ids, run ids and review notes; `docs/RELEASING.md` carries the durable part.
6. **Code and workflows cite the log, so moves need link repair.** 61 records are named outside
   `docs/decisions/`, 28 from `crates/`, and `ci.yml` and `release.yml` point at eight of them. Only 18
   of the 59 non-keep records have such a link, 16 of them outside historical plans
   (`docs/superpowers/`, which stay as written). The list is in the appendix and in [Plan](#plan).
7. **`docs/DECISIONS.md` is a different case.** Its 14 greenfield entries are cited by 5 comments in
   `crates/` and all are non-obvious architecture. Its 4 older entries describe `apps/`: "Reuse Make
   as the verification entry point" contradicts the rule that agents never run `make`; "Extract
   responsibilities without relocating the workspace" is already marked superseded; "Start from the
   current applied source" is branch hygiene, not a decision. `decision_records.rs` pins the count at 18.
8. **Rejected alternatives are the value.** 131 of 165 records carry a `**Rejected:**` line, and crate
   READMEs already state the "what". No record failed only because of size, and none is proposed for
   removal for that reason.

## Structure

The smallest structure the content justifies is the one that exists: one file per decision.

| Option | For | Against |
| --- | --- | --- |
| **A. One file per decision, with a status line and in-place revisits (recommended)** | The README's reason still holds: 165 files in seven days, 61 on one day, and a central file conflicted until it was frozen. Filename rule, test and 61 inbound links stay valid. | Needs the hygiene below, or the log keeps accreting fragments. |
| B. One central dated log | One place to read. | Parallel pull requests edit the same file again, which is why `DECISIONS.md` was frozen. Every inbound link changes. |
| C. One file per topic | Fewer files; a topic reads as one story. | Topic names need an owner and a rename rule; parallel pull requests on one topic still conflict; links change. |

Option A plus merging the 38 siblings gets most of C's benefit without its naming problem: a
revisit is a dated section in the original file.

## Proposed convention

This replaces the "Content" section of `docs/decisions/README.md` when approved. The file name
(`YYYY-MM-DD-<issue>-<slug>.md`) and `decision_records.rs` are unchanged.

1. **Admit a record only if all four answers are yes:** would a reader of the code and tests be
   surprised; is it an architecture, protocol, state, dependency, tool or pin choice; will it still be
   true after the next release; is the reason written nowhere else.
2. **Not a record:** conformance evidence (fixture README), test-harness technique (module doc),
   release or CI run logs (the pull request), task status and open items (the issue), product
   behaviour a test already names. Put the useful sentence where the reader will look for it.
3. **Status line under the title:** `Status: accepted`, `Status: superseded by <file>` or
   `Status: partly superseded by <file>: <what still holds>`.
4. **A revisit is a dated section in the original record:**
   `## Revisited 2026-11-06 (#NNN)` with the change and the reason. A reversal gets the old record's
   status line and a new record that links it. The exception is a record the superseding record
   names as fully void, with no surviving bullet (470, 476, 495 under 828): it is removed, and git
   history keeps it. The follow-up issue's own file is not created for a
   one-bullet amendment.
5. **Length:** a title, a status line and about five bullets; over 500 words is a sign the record is a
   work log. Today the median is 331 words and 8 records pass 750.
6. **Instructions** (`AGENTS.md`, `CONTRIBUTING.md`, the pull request template) say "decision log" and
   link the four-question test instead of "non-obvious decisions" alone.

## Plan

Each slice is its own pull request, in this order. Times are for one worker.

| # | Slice | Touches | Time |
| --- | --- | --- | --- |
| 1 | README convention, `AGENTS.md`, `CONTRIBUTING.md`, PR template wording; add a test that every `Status: superseded by` and `Status: partly superseded by` line names an existing file | `decision_records.rs` extended, not loosened | 1 to 2 hours |
| 2 | Status lines on the 6 supersedes (their targets are all keeps, so later slices do not move them); strike the stale bullet in 47 | 7 records | 1 hour |
| 3 | Merge the 38 siblings into their base records in five batches by area (TUI/approvals, sessions, release/packaging, provider adapters, CI/sandbox); repoint links | about 60 files | 3 to 4 hours |
| 4 | Relocate the 7: move the surviving sentences, then delete the records; repoint `docs/gates/m1-latency.md`, `docs/gates/m3-acceptance.md`, the `latency.rs` header | 7 records, 6 targets | 1 to 2 hours |
| 5 | Remove the 8; repoint `docs/research/2026-11-05-546-own-sandbox-and-traffic-control.md` (lines 522-523) | 8 records | 30 minutes |
| 6 | At the cutover (#66): retire the 4 `apps/`-era entries of `docs/DECISIONS.md` and set the count in `decision_records.rs` | 1 file, 1 test | 30 minutes |

Link repair for every move: `git mv` or fold, fix inbound links in the same pull request, then run
`cargo nextest run --locked -p cockpit-workspace-rules --test decision_records` and
`cargo fmt --all -- --check`. The inbound list per record is in the appendix. Decision records
added after `81f41047` are judged against the admission test in the pull request that adds them;
each slice starts by listing `docs/decisions/` and checking the new files against the appendix.
The count of 18 in `decision_records.rs` is for `docs/DECISIONS.md` and is not affected.

Risks. Folding records loses their individual dates in the file list (the dated section keeps
them). Removal loses only what git history keeps. The reviewers' weakest calls are listed in
[Weak calls](#weak-calls); I ask about them rather than decide.

## Weak calls

- **417, 419-native-input, 436-codex-0166-pin:** removal is a judgement. Each has a `Rejected` line
  (for example "advancing the version label on fixture replay alone"). The reviewer found the policy
  in `crates/cockpit-codex/README.md` and the fixture READMEs; I confirmed the 0.166.0 pin in code.
- **410-coordinator-kit** spans many decisions and could be split, not kept whole.
- **829-native-sandbox-launch** is five decisions bundled in 1,119 words; trimming to the `Rejected`
  lines is optional.
- **845-tmux-socket-by-path:** the `cockpit-tmux` README carries its core reason.
- **754-per-project-github-token:** partly superseded by 832 (engine bullets void; file convention
  and mode checks hold). A status line, not a rewrite.

## Questions for Theo

Theo answered **A on all five** on 2026-11-06, in a Torlek reply to the #548 worker; the pull request
description quotes it, as #618's did. Each question lists the recommendation first.

**Q1. Structure of the log.** A: keep one file per decision, add a status line and write revisits
in place (recommended). B: one central dated log. C: one file per topic. A keeps the parallel-pull-request
rule and every inbound link; B and C change the links and reintroduce conflicts.

**Q2. The 8 removals** (470, 476, 495: dead sandbox-engine records that #828 names as superseded;
417, 419-native-input, 436-codex-0166-pin, 493, 98: evidence or status notes). A: remove all 8
(recommended). B: remove only the 3 engine records. C: remove none, add status lines.

**Q3. The 38 same-topic fragments.** A: fold each into its base record as a dated "revisited" section,
in five pull requests by area (recommended). B: fold only the pairs that contradict or reverse each
other (about 10). C: no merges.

**Q4. `docs/DECISIONS.md`** (frozen at 18 entries; 4 describe `apps/`). A: leave it until the cutover
(#66), then retire those 4 and update the count in `decision_records.rs` (recommended). B: convert all 18
to files now and repoint 5 code comments. C: remove the 4 now.

**Q5. Enforcement.** A: extend `decision_records.rs` so a `Status: superseded by` or
`Status: partly superseded by` line must name an existing file (recommended). B: README and pull request template wording only.

## Sources

- Issue #548 and the quoted definition (Theo, 2026-11-02).
- [Documentation plan, #618](2026-11-06-618-documentation-plan.md), row "Decision records" and
  question 1B.
- `docs/decisions/README.md`, `crates/cockpit-workspace-rules/tests/decision_records.rs`,
  `docs/DECISIONS.md` (18 entries).
- [Nygard, Documenting architecture decisions](https://www.cognitect.com/blog/2011/11/15/documenting-architecture-decisions):
  a reversed decision is kept and marked superseded.
- [MADR](https://adr.github.io/madr/): a status field and a revisit date on each record.
- [`548/dispositions.md`](548/dispositions.md): 165 rows with reasons and inbound links.
