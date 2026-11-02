# Name the product before the alpha

**Theo chose Torlek on 2026-11-02**
([decision record](../decisions/2026-11-02-339-product-name.md)). Theo was not keen on the eight
proposals studied below, and Torlek came out of a later second-opinion pass. Its checked table,
sources and limits are in [the Torlek appendix](339/torlek.md). In short:

- **Packages:** no package on crates.io, npm or PyPI, in Homebrew, in Arch (official or AUR), or in
  Debian, Ubuntu or Fedora.
- **Trademarks:** USPTO, TMview and WIPO returned no marks.
- **Domains:** no product domain will be bought.
- **Not established:** legal clearance, the official Nix index, `.sh` and a native-speaker check.

The rename itself is [#560](https://github.com/Androlax2/agent-cockpit/issues/560).

The rest of this report is the **eight-name study that preceded the choice**, kept as evidence.
Its rankings were provisional: Crewkiln and Runlark survived its checks, and the other six had
software collisions. They were not chosen.

The four original candidate appendices remain **historical initial attempts**. Their unavailable
package/domain observations are superseded where specified by the [package follow-up](339/package-followup.md)
and [domain/trademark follow-up](339/domain-trademark-followup.md), both dated 2026-11-02.
Their positive collisions and unrefreshed observations remain evidence; no name has been cleared.

## Two provisional finalists survive the eight-name comparison

The brief is a personal, terminal-first Rust coding-agent orchestrator with a base name of
at most eight letters. All eight strings meet that limit using plain ASCII; ranking weights
observed identity collisions first, then product fit and command ergonomics.
Ranks 3–8 order rejected alternatives, not approved fallback choices.

| Rank / proposed preference | Base CLI / letters | English → French meaning and ergonomic trade-off | Evidence and disposition |
| --- | --- | --- | --- |
| **1. Crewkiln** | `crewkiln` / 8 | Crew = équipage/équipe; kiln = four. Team craftsmanship fits agents; unfamiliar “kiln” and final `ln` complicate French pronunciation; literal crew-in-an-oven imagery is awkward. | **Provisional recommendation**; no exact collision established in checked sources. [Evidence](339/crewkiln-runlark.md); [crew](https://www.collinsdictionary.com/dictionary/english-french/crew), [kiln](https://www.collinsdictionary.com/us/dictionary/english-french/kiln). |
| **2. Runlark** | `runlark` / 7 | Run = courir/marcher; lark = alouette/blague. Shorter and playful, but suggests sport or frivolity more than agent coordination; meaning remains English-dependent. | **Provisional alternative**; no exact collision established in checked sources. [Evidence](339/crewkiln-runlark.md); [run](https://dictionary.cambridge.org/us/dictionary/english-french/run), [lark](https://dictionary.cambridge.org/us/dictionary/english-french/lark). |
| 3. Runtide | `runtide` / 7 | Execution/flow; tide = marée. Compact nautical metaphor, but also running/sport; opaque without English. | **Reject:** exact gameplay framework and developer playground names. [Evidence](339/runtide-taskwake.md); [tide](https://dictionary.cambridge.org/us/dictionary/english-french/tide). |
| 4. Runhelm | `runhelm` / 7 | Execution/steering; helm = barre. Reads as “run Helm,” adding developer-tool ambiguity. | **Reject:** exact GitHub organization/repository; adjacent Helm software uses. [Evidence](339/runhelm-taskhelm.md); [helm](https://dictionary.cambridge.org/us/dictionary/english-french/helm). |
| 5. Crewspan | `crewspan` / 8 | Team spanning work/time; span has length/time/bridge senses. Both parts require English; visual transposition with CrewSnap is plausible. | **Reject:** exact field-service software and organization; same-name Paperclip fork. [Evidence](339/crewport-crewspan.md); [span](https://dictionary.cambridge.org/dictionary/english/span). |
| 6. Crewport | `crewport` / 8 | Team harbor/connection point; French “port” is immediately recognizable. Clear metaphor, but transport/network associations compete. | **Reject:** exact AI-agent marketplace brand and software repositories. [Evidence](339/crewport-crewspan.md); [port](https://dictionary.cambridge.org/dictionary/french-english/port). |
| 7. Taskhelm | `taskhelm` / 8 | Steering tasks; tâche + barre. Purposeful, but `skh` is less fluid and Helm is crowded. | **Reject:** exact npm command, local-development workbench and older PyPI task manager. [Evidence](339/runhelm-taskhelm.md); [task](https://dictionary.cambridge.org/dictionary/english-french/task). |
| 8. Taskwake | `taskwake` / 8 | Waking work; wake = réveiller/sillage, also an English funeral gathering. `skw` is less immediate in French. | **Reject:** existing coding-agent task CLI already uses the command/config/state names. [Evidence](339/runtide-taskwake.md); [wake](https://dictionary.cambridge.org/us/dictionary/english-french/wake). |

These are interpretations of coined compounds, not dictionary definitions of the full names.
Dictionary support is publisher search excerpts: the notes do not establish successful full-entry
fetches. No obvious English/French insult emerged, but no native-speaker, dialect or slang clearance
was performed. Crewkiln also has non-software search noise from an indexed “Arc Crew Kiln” apparel
listing, not a verified exact concatenated software brand ([Parlez](https://parlez.co.uk/products/arc_crew_kiln_211025)).

The rejection evidence is stronger than a missing registry record. Taskhelm's npm metadata names
the executable `taskhelm`, and its README describes parallel git-worktree work
([npm](https://registry.npmjs.org/taskhelm), [README](https://github.com/Kaka-123-D/TaskHelm/blob/main/README.md)).
Taskwake's fetched README explicitly installs `~/.local/bin/taskwake` and uses
`~/.config/taskwake` and `$XDG_STATE_HOME/taskwake/tasks` for an AI-agent background-task ledger
([README](https://github.com/sdpwh7-ux/taskwake/blob/main/README.md)).

CrewPort's fetched marketing README calls it an AI-agent freelance marketplace
([primary evidence](https://github.com/ologos-repos/crewport-ads/blob/main/README.md)).
Crewspan's exact field-service project is separate evidence from its Paperclip fork:
the latter retains Paperclip branding, so it is **repository ambiguity, not proof of a distinct
Crewspan-branded AI competitor** ([field-service README](https://github.com/Hailemeskel-Yigrem/crewspan/blob/main/README.md),
[fork metadata](https://api.github.com/repos/sico-vibes/crewspan)).
Runtide has a fetched coding-playground README; Runhelm has an exact software repository and organization
([Runtide](https://github.com/treatwashere/runtide/blob/main/README.md), [Runhelm](https://api.github.com/repos/runhelm/runhelm)).

## Package evidence supports a shortlist, not availability

**Legend:** **H** = confirmed exact hit; **A** = successful absence in the stated search/list scope;
**404** = exact public resource not returned, not registration permission;
**P** = partial evidence; **U** = unavailable substantive result (even if a portal returned HTTP 200/403);
**N** = no registration lookup documented. A timeout is **U**, never zero or 404.
Indexed snippets do not establish an origin HTTP response or authoritative database counts.

| Candidate / evidence | crates.io | Homebrew core | Homebrew casks | Arch official | AUR | Debian | Ubuntu | Fedora | Nix | npm | PyPI |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [Crewkiln](339/crewkiln-runlark.md) | 404 index + A parent | A complete paths | A complete paths | A exact | A exact | A substring | A substring | A default query | A indexed substring¹ | 404 | 404 |
| [Runlark](339/crewkiln-runlark.md) | 404 index + A parent | A complete paths | A complete paths | A exact | A exact | A substring | A substring | A default query | A indexed substring¹ | 404 | 404 |
| [Runtide](339/runtide-taskwake.md) | 404 exact API | A complete paths | A complete paths | A exact | A exact | A substring | A substring | A default query | A indexed substring¹ | 404 | 404 |
| [Runhelm](339/runhelm-taskhelm.md) | 404 exact API | A complete paths | A complete paths | A query | A name search | A substring | A substring | A default query | A indexed substring¹ | 404 | 404 |
| [Crewspan](339/crewport-crewspan.md) | 404 exact API | A complete paths | A complete paths | A exact | A exact | A substring | A substring | A default query | A indexed substring¹ | 404 | 404 |
| [Crewport](339/crewport-crewspan.md) | 404 exact API | A complete paths | A complete paths | A exact | A exact | A substring | A substring | A default query | A indexed substring¹ | 404 | 404 |
| [Taskhelm](339/runhelm-taskhelm.md) | 404 exact API | A complete paths | A complete paths | A query | A name search | A substring | A substring | A default query | A indexed substring¹ | **H command/package** | **H project** |
| [Taskwake](339/runtide-taskwake.md) | 404 exact API | A complete paths | A complete paths | A exact | A exact | A substring | A substring | A default query | A indexed substring¹ | 404 | 404 |

¹ Both **stable 26.05 and unstable**. Updated cells use the [package follow-up](339/package-followup.md);
row links retain initial evidence. Debian's all-suite/section/architecture substring searches returned
no results. Fedora's default query returned no package results; Runtide's “Did you mean runtime?”
is a suggestion, not a hit. Fedora tokenization/fuzzy semantics remain unverified.
The finalist crates.io evidence comes from
the official Git index: exact-file 404s are corroborated by successful parent listings, not a
successful direct crates.io API search ([Crewkiln entry](https://api.github.com/repos/rust-lang/crates.io-index/contents/cr/ew/crewkiln),
[Runlark entry](https://api.github.com/repos/rust-lang/crates.io-index/contents/ru/nl/runlark)).
That does not check reserved names or publication eligibility.

Refreshed Homebrew checks for **all eight names** examined complete, non-truncated source paths,
including aliases, with no matches; they did not inspect every formula body or shipped executable
([core tree](https://api.github.com/repos/Homebrew/homebrew-core/git/trees/HEAD?recursive=1),
[cask tree](https://api.github.com/repos/Homebrew/homebrew-cask/git/trees/HEAD?recursive=1)).
The appendices preserve observed revisions; live HEAD URLs can change.

Nix's official evaluated-package indexes returned `doc_count:0` for each name in stable 26.05
and unstable, superseding the earlier partial source-only evidence. These are lowercase wildcard
substring checks over `package_attr_name`, `package_pname` and `package_programs`, with no
case-insensitive option. The appendix preserves exact POST payloads, concrete indexes, timestamps,
counts and controls; this is not an exhaustive overlay/platform/installed-file inventory
([Nix request evidence](339/package-followup.md#3-was-evaluated-nix-package-evidence-feasible-and-what-remains)).
Taskhelm's archived AUR package is historical evidence, not a contradiction of today's empty
current-AUR search ([archived PKGBUILD](https://github.com/aur-archive/taskhelm/blob/master/PKGBUILD)).

Successful package checks do not cover scoped npm packages, third-party Homebrew taps,
every normalized variant, unpublished projects or all package-owned executable names.
**Short CLI/common-tool check:** neither finalist string appears in the fetched Bash manual or
Coreutils contents; fish documentation timed out. No PATH, alias, zsh or exhaustive POSIX/distro
file inventory was performed ([Bash](https://www.gnu.org/software/bash/manual/bash.html),
[Coreutils](https://www.gnu.org/software/coreutils/manual/html_node/index.html), [scope](339/crewkiln-runlark.md)).
Use the full base token for evaluation; no shortened alias was checked.
`<name> daemon` does not cure a collision in `<name>`, and `-next`/`-dev` forms exceed eight characters.
Those suffix examples are not a new profile or development-binary design.

## GitHub negatives cannot close domain or trademark gaps

GitHub repository counts below are exact case-insensitive basenames inspected in the returned
public searches, not raw substring totals. Crewport/Crewspan searches explicitly included forks;
other searches have the narrower coverage described in their appendices.
Account lookups distinguish the shared user/organization namespace from repository names.

| Candidate / evidence | GitHub exact repositories | GitHub account / organization | `.dev` registration | `.sh` registration | USPTO 9/42 | EUIPO 9/42 |
| --- | --- | --- | --- | --- | --- | --- |
| [Crewkiln](339/crewkiln-runlark.md) | A public name search | 404 user + org | RDAP 404 | U 200 empty form | U JS shell | U 403 CloudFront |
| [Runlark](339/crewkiln-runlark.md) | A public name search | 404 user + org | RDAP 404 | U 200 empty form | U JS shell | U 403 CloudFront |
| [Runtide](339/runtide-taskwake.md) | H: 3 | H user; org 404 | RDAP 404 | N | U JS shell | U 403 CloudFront |
| [Runhelm](339/runhelm-taskhelm.md) | H: 1 | H organization | RDAP 404 | N; web visibility only | U JS shell | U 403 CloudFront |
| [Crewspan](339/crewport-crewspan.md) | H: 2, including Paperclip fork | H organization | RDAP 404 | U prior timeout | U JS shell | U 403 CloudFront |
| [Crewport](339/crewport-crewspan.md) | H: 3 | H user | RDAP 404 | U prior timeout | U JS shell | U 403 CloudFront |
| [Taskhelm](339/runhelm-taskhelm.md) | H: 3 | 404 shared-account lookup | RDAP 404 | N; web visibility only | U JS shell | U 403 CloudFront |
| [Taskwake](339/runtide-taskwake.md) | H: 4 | 404 user + org | RDAP 404 | N | U JS shell | U 403 CloudFront |

Finalist repository searches returned successful empty results, while account endpoints returned
404s; neither response guarantees GitHub registration eligibility
([Crewkiln search](https://api.github.com/search/repositories?q=crewkiln+in:name&per_page=100),
[Runlark search](https://api.github.com/search/repositories?q=runlark+in:name&per_page=100)).
All eight `.dev` results are now **authoritative RDAP 404 / no registration returned**, using the
Google endpoint obtained from a successful [IANA bootstrap](https://data.iana.org/rdap/dns.json) response.
This supersedes initial timeouts/DNS errors, not reservation, premium-pricing or registrar checks.
Finalist `.sh` requests returned HTTP 200 forms with hidden empty results, not executed WHOIS
negatives; the other six retain their initial scope ([follow-up](339/domain-trademark-followup.md)).
The `runhelm.sh` reference in Pelorus is a **script filename**, not domain ownership
([Pelorus](https://pelorus.readthedocs.io/en/v1.3.0/Install.html)).

**No candidate/class trademark queries executed.** USPTO returned HTTP 200 with a JavaScript
application shell and no HTML forms. EUIPO eSearch reset (`ECONNRESET`), then its search landing
page returned HTTP 403, “Request blocked.”, generated by CloudFront; all further EUIPO access stopped
([USPTO](https://tmsearch.uspto.gov/), [EUIPO](https://www.euipo.europa.eu/en/search-ip)).
The installed Chromium's isolated safe launch failed with exit **-6 (SIGABRT)**, **“No usable sandbox!”**
and `ptrace: Operation not permitted (1)`, before any USPTO navigation. No `--no-sandbox` bypass
was attempted. Capability blocker **`1820d58f`** tracks this limitation; neither failure establishes
a greyproxy denial ([exact requests and launch evidence](339/domain-trademark-followup.md)).

Remaining checks are domain reservation/premium/registrar eligibility, actual `.sh` results if needed,
and exact/similar USPTO/EUIPO marks in classes 9/42 with owners, status, identifiers and goods/services.
No trademark counts or legal clearance are established. Successful package and RDAP negatives do not
reverse software collisions: these are **eight evaluated proposals, not eight cleared names**.

## Rename in this order once the follow-up is approved

The following ordered plan is a **proposal for [#560](https://github.com/Androlax2/agent-cockpit/issues/560),
not an execution instruction**.
Repository guidance keeps installed legacy commands, `apps/target/release`, protocol validation,
state ownership and host authority stable ([engineering guide](../../AGENTS.md),
[architecture](../ARCHITECTURE.md)). No new dependencies or manifest edits are authorized here.

Owns: the proposed product identity and coordinated rename order for #339. Dependencies:
Theo's choice precedes the rename and #99 design; #134/#196 profile isolation must survive;
Issue #97 needs the approved identity before the first alpha tag. Product naming can precede #209.
The chosen name is **Torlek** (`torlek` / `TORLEK`); the spelling map below uses it.

Proposed spelling map for **Torlek**:

| Surface | Proposed target after separate approval |
| --- | --- |
| Repository and greenfield crate packages | `Androlax2/torlek`; `crates/torlek-*` and matching Cargo package/import identifiers |
| Public CLI / daemon entry | `torlek` and `torlek daemon`; inventory any separately shipped daemon executable before deciding its replacement |
| Profiles and XDG namespaces | Default `torlek`, named `torlek-next` / `torlek-dev` directory components; preserve `dev` build-default semantics, not a mandatory separate dev binary |
| Units and service identity | `torlek[-<profile>].service`, corresponding launchd labels and explicit ownership markers; retain legacy recognition |
| Environment variables | Inventory every `COCKPIT_*` producer/consumer and propose `TORLEK_*` atomically, including profile/log-level variables |
| Distribution and docs | Release asset names, bundle paths, package metadata, installer/update URLs, examples and help text; preserve historical decision records |

The base names are shell-safe ASCII and fit XDG components; these examples do not establish
exclusivity or tested socket-path lengths. Prefer a daemon subcommand to an extra `torlekd`
command; the recommendation does not authorize changing an existing process boundary.

Proposed ownership handoff, using the existing overnight lanes. These are coordination targets,
not accepted new assignments; Chief must confirm scope with each owner before implementation.

| Surface / owning module or issue | Executor / approval boundary | Handoff required |
| --- | --- | --- |
| Naming, decision record and logo (#339/#99) | Theo chooses the name; #99 Designer **unassigned** pending naming/design approval | Record the selected spelling and research conditions before briefing design |
| Greenfield crate/bin/env rename; `cockpit-config` profile/layout and `cockpit-cli` / `cockpit-daemon` consumers (#134/#196) | Rename implementer **unassigned**; Chief coordinates the follow-up | One atomic producer/consumer inventory and profile/legacy-isolation test evidence |
| Packages, bundle manifests, installers, release URLs and `tools/release.py` (#191/#192/#493/#97) | Existing lane `greenfield-493-release-delivery`; Theo approves protected workflow operations | Agreed spelling map and manifest-edit approval before package/artifact changes |
| README, site and public examples (#98/#102) | Existing lane `greenfield-98-user-documentation` | Verified command and repository targets from rename/release owners |
| Managed sandbox integration references (#495/#433) and provider/context consumers (#419/#436/#51) | Existing lanes `greenfield-470-managed-sandbox` and `greenfield-436-provider-validation` respectively | Audit only naming references affected by the approved rename; avoid duplicating their functional work |
| GitHub repository/settings/required checks; later greenfield cleanup (#209) | Theo performs/approves protected operations; #209 executor **unassigned** | Release/docs owners verify URL dependencies first; #209 keeps its cutover dependency |

1. **Done: Theo chose Torlek and the decision is recorded**
   ([decision record](../decisions/2026-11-02-339-product-name.md)). Theo decided not to buy a
   domain. Legal clearance remains a condition before a public launch, not a prerequisite for the
   internal rename.
2. **Done: the Alpha 1 rename follow-up exists, and #99, #209 and #97 carry the name.**
   [#560](https://github.com/Androlax2/agent-cockpit/issues/560) owns the rename and its acceptance.
   #99 designs the mark for Torlek. Keep greenfield-cleanup #209 separate: its execution depends on
   cutover, while the product rename before alpha can precede that cleanup.
3. **Use a feature branch and inventory the current greenfield surface.** Enumerate crate/package
   and binary names, imports, profiles, filesystem layout, protocol identifiers, environment
   variables, tests, docs, release assets and generated references before editing. Retain `apps/`
   and legacy launchers/services. Any necessary dependency-manifest edits need separate approval;
   this proposal grants no Git, installation or host-action permission.
4. **Make the internal greenfield rename atomic and verify the behavioral boundaries.** Coordinate
   producers/consumers and fixtures rather than doing a blind global replacement. Preserve wire
   contracts, validation, transaction ownership, authenticated sender checks and host approval.
   Do not migrate, delete or silently adopt existing state by default; namespace/state transitions
   require an explicit decision. Test with isolated state and fake host commands.
5. **Preserve #196 profile semantics explicitly in regression coverage.** Precedence remains flag,
   command name, environment variable, build. Non-bundle builds default to `dev`, including local
   release-mode builds; inherited default-profile variables remain visibly ignored there.
   Keep legacy detection and refusal authority at `daemon install`, including PATH shadowing,
   legacy units and same-profile ownership checks; #97's installer must use that boundary.
   Preserve recognition of legacy identifiers even if new-product identifiers change
   ([profile decision](../decisions/2026-11-01-196-profiles.md)).
6. **Coordinate packaging/release and documentation owners.** The packaging/release owner updates
   generated names, bundle layout/manifests, checksums, update URLs, installers and package metadata
   together, after the required approvals. The docs owner updates README/site and examples against
   verified spellings. Run focused profile/install/package checks and the applicable project gates;
   make no release, tag or live-acceptance run as part of this research.
7. **Change GitHub identity/settings only through separate host approval.** Inventory and verify
   repository URLs, workflow references, required-check names and external consumers first; do not
   rename the remote before references are verified. Actual repository/settings/ruleset changes
   are a separately approved operation, followed by verification of links and required checks.

## Workflow status

**#339's research is complete.** Theo chose Torlek, the
[decision record](../decisions/2026-11-02-339-product-name.md) holds the checked table and the
reasons, and [#560](https://github.com/Androlax2/agent-cockpit/issues/560) owns the rename.

The eight-name study's earlier "morning choice" between Crewkiln and Runlark is **superseded**.
Its trademark and browser blockers (`1820d58f`) still describe what the sandbox could not reach;
Theo ran the USPTO, TMview and WIPO searches for Torlek in his own browser.

**Still open (conditions, not blockers to the rename):**

- legal clearance before a public launch
- the official Nix index and `.sh`
- a native-speaker slang check

Review and verification evidence belongs in PR #534.
