# Cockpit review — evidence register and delivery matrix

Companion to [the architecture review](audit-2026-10-30.md). Observations were collected
on 2026-10-30. Historical plans and receipts are evidence of their stated scope, not of
the latest moving branch or every running binary.

## Baselines

| Baseline | Exact identity | What it establishes |
| --- | --- | --- |
| Review working checkout | `/home/theo/IdeaProjects/agent-cockpit`, branch `feat/fail-closed-git-protection`, HEAD `585b1f293932e02f4a2518b03d43381d36866653` | Inherited, stale source; not the source used to judge current refactoring |
| Local `origin/main` | `20748511c7cc82a2ac5e0f11f35d981778d03154`; origin `/home/theo/agent-cockpit` | A local tracking ref, not fresh GitHub main |
| Integration checkout (I) | `/home/theo/IdeaProjects/.parallel/agent-cockpit/todo-5779dac9`, branch `fix/arch-package-followup`, HEAD `52e8471a5e119510d1837adf6dcc6f4f912d68ec` | Approved tracked-source snapshot with core extraction and later packaging/sidebar adaptation |
| Observed GitHub main | `4e616e86e01cc83f9205afd50ba04be181d52e54` | Merge of #15 at `2026-10-30T15:16:37Z`; exact main check failed |
| Live checkout (L) | `5ec8ff90be157c580236c606ce3ba9ac536cb453` | Four-crate tree, prior applied changes and sidebar repair; no `cockpit-core` |
| Captured service | OSD PID `774992`, active since `2026-10-30 16:08:54 CET` | Runtime observation from host receipt; not cryptographic binary/source attestation |
| Sidebar sample | PID `773994`, executable `/home/theo/agent-cockpit/apps/target/release/cockpit-sidebar` | Captured main-thread waits; process predates that OSD start |

I and L are different delivery lines, not interchangeable aliases for GitHub main.
The live-only `cc606b788d9f99821b6296849e5ee4232aa26725` Chief wake change admits a
narrowly verified own observed coordination todo. Its corresponding behavior is absent
from I. It must be reconciled deliberately before deploying a newer integration tree.

### Preserved working state

The pre-existing unstaged diff is two files, **24 insertions / 1 deletion**:
`apps/cockpit/tests/cockpit_script.rs` and `config/bin/.local/bin/cockpit`.
The index was empty. Existing untracked `.claude/`, `.envrc`, `.outgoing-archive/`,
`.worktrees/`, OpenCode configuration, plan/spec documents and
`tools/tests/test_cockpit_apply_help.py` are unrelated work. None was overwritten,
staged, committed or treated as part of the refactor's delivered source.

## Approved evidence artifacts

The pinned artifacts below are under `.handoff/chief-review-and-freeze-20261030/`.
Archive members are inspected in place; the source archives contain tracked files,
not the integration owner's unrelated untracked drafts.

| Artifact | SHA256 |
| --- | --- |
| `integration-source-52e8471.tar` | `ffbe606ec2d9586b35f1f4606ec6ddecf6f02d2e4c475cee0f1087750e499c9f` |
| `live-source-5ec8ff9.tar` | `bd73d6f973dbf7ffedbe47fb8ba453fdfd03e8a43aeebde05e1a8b9ac6c0bc25` |
| `packaging-source-52e8471.tar` | `479b4b3c2a026e907b018fa8f152fb947cac418ac4b5aa66751542110abcd055` |
| `sidebar-freeze-20261030T162234.jsonl` | `09a411b3e7d6e8a459cb3af574e13e0bb9765e0ddb53592b09beb8e0e1217bf2` |
| `cockpit-sidebar-4ac0505d.bin` | `4ac0505d452ce8b9c456967472d042e3ffe4b3592d16ac3eeeae76b1524337aa` |
| `sidebar-ipc-boundaries-privileged-20261030.trace` | `f7f91ad8180f6c803696d334b3767ad8ac20b5b1ce4b600d69f19c22397bc17d` |

The supplemental packaging archive is **P**. Hashes establish the bytes reviewed, not
authority to execute the files or proof of a reproducible release.

Host results live at
`/run/user/1000/cockpit-answers/-home-theo-IdeaProjects-agent-cockpit/runs/<id>.{result,output}`.
They are runtime-retained evidence and may disappear after retention/reboot; the
facts, identities and decisive excerpts are preserved here without copying unrelated logs.

| Receipt | Result | Evidence used |
| --- | --- | --- |
| `5dd9281fac9b` | Exit 0 | PR #14 and issues #2/#3/#4; integration/live Git identities; approved source export |
| `d3e959ac9dfd` | Exit 0 | GitHub main, PR/run list, failed #14 log and repository-rule query |
| `6fb99ac2df5a` | Exit 0 | Failed merged-main log, package PR status and live service observation |
| `110ca507a6fa` | Exit 0 | Supplemental packaging/runtime-source export; empty command output is not a test pass |
| `044a453ddef5` | Recorded exit 0 in incident handoff | Original five-minute sidebar capture, copied into this review's evidence directory |
| `bc043ad3697a` | Historical diagnostic | Team/Chief admission and stop records; bounded native-database inspection refusal |
| `a2797eb17fca` | Historical diagnostic | Targeted native session/turn outcomes, HTTP 429s and eventual reply |
| `61b953441016` | Exit 0, inspected existing receipt | PID `773994` executable and installed file share `4ac0505d…`; installed checkout HEAD `5ec8ff9` |
| `5043aaeeb8c6` | Exit 124, inspected existing receipt | Privileged trace attached/detached at the planned 75-second timeout; not an acceptance-test pass |
| `eb274a33a66f` | Exit 0, inspected existing receipt | Copied executable matches `4ac0505d…`; this review hashed but did not execute it |

Direct integration-checkout read returned `File not found`; blocker `9b3d985d` records
that limitation. Sandbox GitHub CLI returned `HTTP 401: Requires authentication`.
Approved exports/host reads supplied review evidence; neither underlying access issue
was repaired or bypassed. The existing blocker remains an environment concern.

## Original intention → actual delivery

Primary intent documents in I:

- `docs/audits/audit-2026-10-28.md` and `baseline-2026-10-28.md`.
- `docs/superpowers/plans/2026-10-28-engineering-workflow.md`.
- `docs/superpowers/plans/2026-10-29-rust-module-and-crate-boundaries.md`.
- `docs/superpowers/plans/2026-10-30-arch-package-delivery.md`.

“Recorded pass” below means the historical ledger/PR reports a pass. “Observed” means
source or remote output was independently inspected in this review.

| Intention | Code / checks / runtime evidence | Delivery state and remaining gap | Impact / confidence |
| --- | --- | --- | --- |
| Establish an honest baseline | October 28 baseline records initial fmt/Clippy/rustdoc/test failures; later 119 Python and 3,149 Rust passes | Historical baseline recorded; not current release proof | Good failure provenance; high confidence in document, not a fresh rerun |
| One local and CI quality gate | I `Makefile:30–85`, `tools/quality.py`, `.github/workflows/ci.yml` call `make ci`; locked Cargo, lint/doc/dependency targets | Implemented; remote execution now works beyond bootstrap but final merged main is red | Substantial improvement, unfinished reliability; high |
| Match the actual language mix | Python/shebang discovery and tests, shell/fish/JS/TS/Lua/Markdown checks | Implemented; a generated `/bin/sh` shim still escaped dialect assumptions until later repair | Host parity needs behavior-level fixtures; high |
| Engineering entry point and two small skills | I `AGENTS.md`, four `.agents/rules/`, two `.agents/skills/`, architecture/development/contribution docs | Implemented; initial four-scenario evaluation explicitly found no advantage over baseline guidance | Better discoverability; no measured token/productivity benefit; high |
| Headless daemon extraction | `cockpit-daemon` owns registry/Chief/Team engine; I and L have daemon/protocol crates | Applied earlier; library's production graph is separate from GTK; executable event loop still GTK-owned | Dependency seam delivered, runtime isolation not implied; high |
| Combined Team/Chief protocol crate | I/L `apps/crates/cockpit-protocol`; canonical types and compatibility exports | Implemented; not a promise of independent Team/Chief stores | Avoids contract cycles and duplicate models; high |
| A1 behavior-grouped tests | Plan records `2239c62`; I `apps/cockpit/src/tests/` | Delivered in #14; recorded Python220/Rust3266 pass, zero skips | Test organization improved; historical counts not current guarantees |
| A2 Todo module | Plan records `122e431`; I `app/todo_tab.rs`, existing terminal tests | Delivered in #14; shared App state deliberately retained | Navigation improved without a new state model; high source confidence |
| A3 session observation/presentation seam | I `app/session_refresh.rs`, `app/sessions_tab.rs`; lifecycle tests | Delivered in #14 as behavior-preserving partition; performance optimization explicitly excluded | Not a freeze fix; high |
| A4 remaining panel/input/render modules | I `app/{status_panel,reviews_tab,ideas_tab,menus,render,input}.rs` | Delivered; plan records body inventories and unchanged behavior | Smaller review surfaces, not independent components; high source / recorded behavioral confidence |
| A5 reverse-dependency cleanup | Core notification and draft text ownership; `test_module_boundaries.py` | Delivered; source guards are literal-path checks, not full Rust name resolution | Useful guard plus limited coverage; high |
| B1 terminal/headless separation | `8b1e98d`; core summaries, `approval/terminal.rs`, `usage/terminal.rs` | Merged; adapter ownership explicit | Protects headless reuse; high |
| B2 one core extraction | `eb1cbc0`; I core and explicit `cockpit::...` compatibility exports | Merged in #14; **not in L** | Actual separation achieved; deployment gap remains; high |
| B3 daemon excludes terminal UI | I dependency manifests and five tests in `tools/tests/test_crate_boundaries.py` | Merged; normal graph excludes terminal/GTK/Ratatui/Crossterm and test support; live graph is earlier | Guards architecture, not latency; high source / recorded test confidence |
| Preserve persistence and dependency behavior | I `teams/store.rs:111–118`, shared Chief connection, `Storage` conversion; external lockfile records equal I/L | 294 external records unchanged; WAL/FULL/250ms busy timeout retained | Meaningful preservation; not a new dependency CVE scan; high |
| Repair CI bootstrap (#2 / W1) | Installer pin changed; subsequent runs reached `make ci` and tests | Bootstrap portion delivered (`a6fd86d7` closed); [#2 remains open](https://github.com/Androlax2/agent-cockpit/issues/2), exact-release verification unfinished | Later failures supersede “tests never ran”; high |
| W2 consumer-aware checks; W4 repository hygiene | Explicitly unchecked follow-ups in module plan | Not established as delivered; preserve active plans and maintained designs | Do not count plan text as enforcement; high |
| Optional structural lint adoption | Plan treats ast-grep as optional; graph and literal source checks exist | No need to claim ast-grep adopted to count A/B complete | Avoid tool adoption as a vanity metric; high |
| Fix sidebar team polling | Source `b1e159207ae829bc5a6a0d50dff3bd4880bd9930`; I adaptation `52e8471`; applied `5ec8ff9`; todo `56954d2c` closed | Applied and short checks passed; subsequent five-minute capture shows recurrent waits | User-visible resolution incomplete; high |
| Prepare Arch package delivery | #15, P staging recipe, runtime lists, release metadata and policy preview; I staging documentation | Merged preparation plus integration follow-up; native release lifecycle/provenance not proved | Useful staging, not an installable release claim; high |

The first plan's suggestion to reassess a separate UI crate was superseded by the later
single-core extraction with `cockpit` retaining UI/binary ownership. Neither plan
promised a root workspace relocation. The later plan explicitly excluded scheduling,
UI redesign and error-policy changes: those cannot fairly be called missing A/B edits,
but they remain essential product work.

## GitHub and CI chronology

| Item | Observed identity/state | Evidence |
| --- | --- | --- |
| [PR #1](https://github.com/Androlax2/agent-cockpit/pull/1) | Merged; head `0ac8a0b66a687233cababf6e16a06c5f19bae5a8` | Original engineering gate; older recorded bootstrap failures `40894463644` / `40894662268` |
| [Issue #3](https://github.com/Androlax2/agent-cockpit/issues/3) | Closed | A1–A5 module/test partition scope |
| [Issue #4](https://github.com/Androlax2/agent-cockpit/issues/4) | Closed | B1–B3 core production-boundary scope |
| [PR #14](https://github.com/Androlax2/agent-cockpit/pull/14) | Head `f84ba9f5d8b36a982737c0505890f6d00659ef6e`; merged as `681effeb7b19d3f64ff29dd92911ab3cb5cfcc26` | [Head run 41047226643](https://github.com/Androlax2/agent-cockpit/actions/runs/41047226643) failed; [merge run 41048152770](https://github.com/Androlax2/agent-cockpit/actions/runs/41048152770) also failed |
| [PR #15](https://github.com/Androlax2/agent-cockpit/pull/15) | Head `bbfd0bcdf15682e257e8c705d7c3acfcdf498bef`, merged | [Head run 41065006077](https://github.com/Androlax2/agent-cockpit/actions/runs/41065006077) succeeded |
| Main after #15 | `4e616e86e01cc83f9205afd50ba04be181d52e54` | [Run 41079202527](https://github.com/Androlax2/agent-cockpit/actions/runs/41079202527) failed |

PR #14's body records a final local `make ci`: Python226/OK, Rust3269 passed,
0 skipped, after a native review found and fixed a clean-checkout documentation dependency.
It also records native simplify with no source edits. Those are historical execution
statements, not native review invocations performed by this research session.

### Remote failure excerpts

Run `41047226643`, ANSI formatting removed only:

```text
Ran 228 tests in 38.106s
preference was never saved: cannot load OpenCode models: invalid catalog: EOF while parsing a value at line 1 column 0
Summary [   4.707s] 159/3269 tests run: 156 passed, 3 failed, 0 skipped
```

The failed tests were:

- `tests::menus::a_malformed_model_preference_can_be_replaced_in_settings`.
- `tests::menus::effort_choices_come_from_the_selected_models_variants`.
- `tests::menus::the_model_picker_loads_the_active_projects_catalog_and_saves_the_choice`.

**3,110 tests were unrun**, not passing or skipped. The later package delivery plan
identifies Bash `[[`/`==` in the generated `/bin/sh` fixture and a POSIX correction;
its head subsequently passed remote CI. This explains the portability failure without
claiming every later test failure has the same cause.

Run `41079202527`, ANSI formatting removed only:

```text
Ran 253 tests in 41.546s
test launches_started_together_never_share_a_temporary_file ... FAILED
ln: failed to create symbolic link '/tmp/cockpit-test-09a675b4c0f6/home/.cache/agent-shims/bin/make': Already exists
Summary [ 199.680s] 1988/3282 tests run: 1987 passed (1 slow), 1 failed, 0 skipped
```

Failure location: `apps/cockpit/tests/greywall_launcher.rs:970`, target
`cockpit::greywall_launcher::launches_started_together_never_share_a_temporary_file`.
**1,294 tests were unrun**. Missing `/etc/ssl/cert.pem` and Git-identity warnings also
occur in that output; they are not proven causes of the assertion failure.
The duplicate-link error is an investigation lead, not a complete concurrency diagnosis.

The ruleset query returned no rows. Branch-protection configuration, permissions and
merge actor behavior were not fully established. The timing proves the #14 head run
finished red after merge; it does not prove a specific protection bypass.

## Measurement methods and limits

### Sidebar capture

- Parse each JSONL object with `time`; exclude the initial metadata record.
- Choose the thread whose `tid` equals the captured process `pid`.
- Count exact `wchan == "unix_stream_read_generic"`.
- Group consecutive matching samples; estimate duration from the first blocked sample
  to the next nonblocked sample. Sampling interval is approximately 0.5 seconds.

Results: **600 samples / 299.667866 seconds; 141 matching samples / 23.5%**.
The first observed long run is left-censored (5.503 seconds observed, true start unknown).
The nine later complete runs are 8.004, 8.005, 6.003, 7.004, 6.504, 7.004,
7.004, 7.504 and 7.004 seconds. Their start-to-start intervals are
30.017–30.517 seconds. This is sampled occupancy, not exact wall-clock blocked time,
a stack trace, or proof of the particular daemon handler.

Earlier three help/Escape cycles measured 30.4/30.3/29.9 ms. They demonstrate response
during those cycles only. They do not falsify an intermittent approximately 30-second
maintenance stall.

### Chief-relayed investigator follow-up

Received after the initial report. These are attributed collaborator observations,
not new user authorization or a repair/deployment receipt:

- Earlier update `973dd5dccd82c9fcedcad5c57ddf4007` definitively expired before delivery,
  according to Chief. Do not count it as a delivered handoff.
- The investigator reports the local `live-source-5ec8ff9.tar` annotation matches
  `5ec8ff90be157c580236c606ce3ba9ac536cb453` and all eight source/test SHA256s match
  the integration owner's receipt. This review has not independently reacquired that
  eight-file receipt; its own archive hash and source inspection remain separate evidence.
- `68e5cb51` is a completed source-access blocker, not a recurrence-repair task. Both
  original sidebar owners reportedly confirmed no active sidebar writes. The dedicated
  investigator remains active and has received the source receipt; no scope is reassigned.
- The initial reported lead was `dispatch_queue()` → `queue_context()` → full team snapshot,
  synchronously on the UI thread on the due 30-second path, before queue-enabled checks.
  Remaining display IPC is another blocking candidate.

Read-only inspection here confirms the source chain in both pinned archives:
L `apps/cockpit/src/app.rs:1836,2204–2245,8286–8288` and
I `apps/cockpit/src/app.rs:1188,1556–1597,2936–2938`;
both `apps/cockpit/src/app/continuity_ui.rs:526–539` fetch the snapshot before reading
`todo-queue`. L `apps/cockpit/src/queue.rs:16,84–85` and I
`apps/crates/cockpit-core/src/queue.rs:16,84–85` establish the 30-second constant and
later disabled-queue rejection. Empty queued/Chief-requested work returns early;
this is not an unconditional snapshot every 30 seconds in every sidebar state.

The later Chief relay identifies accumulated synchronous scheduling IPC as the stronger
lead. Existing receipts and copied artifacts were inspected before the audit-only scope
was restated; no additional live diagnostic was requested by this review. Independent
read-only analysis established:

- The copied executable hash matches both PID `773994` and the installed executable
  in receipt `61b953441016`. This pins that captured binary, not a reproducible build
  attestation or the identity of a process after a subsequent deployment.
- Two complete trace cycles each contain 111 host connections in stack groups of
  1, 22 and 88. Return-to-next-entry `epoll_wait` gaps are 7.824929 s and 6.854054 s;
  entry-to-entry gaps include the preceding wait and are 7.864994 s and 6.894121 s.
  These are instrumented input-poll gaps, not pure IPC-handler time or acceptance results.
- The investigator maps those groups to snapshot, fresh-session and existing-session
  reservation attempts at L `app.rs:2204,2375,2263`. Disassembly/source landmarks
  corroborate the scheduling roles; `addr2line` returned unknown locations because the
  binary is stripped. Exact Rust line attribution remains investigator-reported.
- Chief reports choosing a single-flight background worker; its answer was queued,
  not acknowledged in that update. No product fix or sustained live acceptance was supplied.

Dedicated investigator `ses_12729702f195758623a8976d` retains incident diagnosis and
repair. This audit uses the available evidence only to support bounded scheduling and
input-loop isolation; further mapping or live diagnostics are outside the audit.

### Coordination and context

Admission latency is `finished_at - started_at` for the recorded native attempt.
The 55.936-minute ownership delay is
`(1793279925613 - 1793276569453) / 60000`, comparing reply creation with original
clarification creation. It is not model execution time or total resource consumption.
Four failed requested turns with HTTP 429 explain missing useful responses in this
case; they do not establish an all-project failure rate.

The native database measured **3,767,586,816 bytes** in that earlier diagnostic.
It is the provider's native database, not the Cockpit Team SQLite database. The tool
correctly refused a whole copy above 128 MiB; a later indexed, session-targeted read
provided the needed evidence. Do not use that size to assert Cockpit database growth.

### Instructions and approvals

Measured instruction bytes:

| Artifact | Bytes | Interpretation |
| --- | --- | --- |
| Global instruction file | 28,294 / 432 lines | Approximately 7,074 tokens at bytes/4, a rough heuristic |
| Global skill activation file | 2,813 | Additional policy context |
| I root `AGENTS.md` | 2,990 | Navigation index |
| I four scoped rules | 4,333 combined | Load only matching scope; not necessarily present every turn |
| I two project workflow skills | 4,782 combined | Task-triggered workflows |

The retained approval-result directory contains **34 results, 10 nonzero**. It is
retention-biased, includes other sessions and different action types, and lacks a
complete denominator of requests, retries, human wait and work duration. Do not report
10/34 as the system's approval reliability. Four explicit host approvals belong to
this review's evidence acquisition.

## Verification performed for this research

Read-only inspection covered snapshots, manifests, code paths, tests, plans, GitHub
metadata/logs, approval results and primary external repositories. Python analysis
recomputed archive hashes, capture statistics, instruction sizes and external Cargo
lock-record equality. Core has **29**, not 31, public `pub mod` declarations; line
numbers in `lib.rs` must not be mistaken for a module count.

No product compilation, test rerun, load test, live SQL mutation, dependency installation,
service restart, release or Git write occurred. The research artifacts receive only
document/reference and workspace-preservation checks. Native `/code-review` and
`/simplify` implementation gates are not invoked for this research-only report.

Remaining evidence gaps: full Chief input/token accounting, useful events per wake,
end-to-end throughput, complete approval history, current database growth/query plans,
idle/load resource baselines, cold/warm build cost, binary provenance, native package
lifecycle and a fresh exhaustive sandbox/phone/secret-history security assessment.

### Document validation and scope check

Validation commands explicitly selected the five new report/roadmap files, including
untracked files; they did not rely on a tracked-files-only discovery command.

- `markdownlint-cli2 --config /tmp/opencode/cockpit-review-markdownlint.jsonc` followed
  by the five artifact paths: exit 0, `Linting: 5 files`, `Summary: 0 issues in 0 files`.
- `lychee --offline --include-fragments=anchor-only --no-progress` followed by the same
  paths: exit 0, 43 total links, 35 unique, 11 OK, 0 errors, 32 excluded. External URLs
  were excluded by this offline check; primary-source reads are documented separately.
- Read-only Python validation: five documents, 11 local links, no missing local targets,
  trailing whitespace or unmatched code fences. Archive hash/capture/lockfile calculations
  are reported above; no product test result is inferred.
- `git diff --cached`: empty. `git diff` on the two pre-existing tracked modifications
  was inspected separately; `git status --short` adds only these five research documents
  to the previously recorded visible worktree delta.
- Final `cockpit-todo list`: existing implementation and broad-security tasks remain
  outside this report's completion. No open task was fixed by product changes here.
  The source-access task `68e5cb51` was observed closed by the final list; the later
  Chief-relayed follow-up clarifies that this is not a freeze-repair completion.

One validation setup lookup failed, exit 1:
`KeyError: "filename '.markdownlint-cli2.jsonc' not found"`.
The source export omitted that configuration file. The validator instead used the
existing policy read with `git show origin/main:.markdownlint-cli2.jsonc`:
default rules, `MD013: false`, and sibling-only duplicate headings. A temporary copy
under `/tmp/opencode` supplied those settings; repository configuration was not edited.
