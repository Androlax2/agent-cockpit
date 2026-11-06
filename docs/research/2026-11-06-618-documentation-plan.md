# Repository-wide documentation plan

Issue [#618](https://github.com/Androlax2/agent-cockpit/issues/618).
**Research and proposal, 2026-11-06. Theo chose the recommended option on all five
[questions](#questions-for-theo) the same day.** Merging this document accepts research delivery
only. It rewrites, moves and deletes nothing; each slice in [Plan](#plan) is still its own PR and
waits for Theo's go-ahead. The answers are Theo's replies to the #618 worker (Torlek session,
2026-11-06); the pull request description quotes them. Decision records are covered only as far as this plan
needs; [#548](https://github.com/Androlax2/agent-cockpit/issues/548) owns them.

## Answer

The README is not the verbosity problem. At 617 words it is shorter than most READMEs below. The
problems are elsewhere:

1. **The first-run story is told three times.** README, [QUICKSTART](../QUICKSTART.md) and
   [USER-GUIDE](../USER-GUIDE.md) each walk install, `torlek init`, a project and a session; the
   guide and quickstart even share the `torlek-example` repository. The site publishes the guide's
   copy and not the quickstart. Its Phone status page and Updates sections are not published at all.
2. **The user docs are 1% of the Markdown.** Outside `apps/` there are 412 files and about
   437,000 words. README, QUICKSTART, USER-GUIDE and KNOWN-GAPS are 4,380 words. About 73% is
   decisions, plans, research, audits and acceptance records; the site renders all 156 decision
   records and 22 design reports next to the user guide.
3. **Reference is written as prose.** `crates/*/README.md` is 29,651 words; the daemon's alone is
   3,255, with a 250-word list of dependency edges that `Cargo.toml` and
   `crate_graph.rs` already state.
4. **Statements disagree.** README says "Every agent works inside a sandbox"; further down
   it says "This release has no sandbox". QUICKSTART says each agent is kept in a sandbox.
   USER-GUIDE says there is none.

Recommendation: write down **one authority per topic** and the reader each page serves, publish the
user path on the site and keep records in the repository, generate reference, and apply ten writing
rules. Do it in six small slices, none a rewrite. Keep the current tooling; nothing here needs a
new tool.

## What other projects do

Word counts are `wc -w` on the Markdown fetched 2026-11-06.

| Project | README | What the README holds | Where the rest lives |
| --- | --- | --- | --- |
| [tmux](https://github.com/tmux/tmux) | 312 | What it is, platforms, dependencies, build | man page |
| [Helix](https://github.com/helix-editor/helix) | 195 | Pitch, features, then four pointers: install, contributing, help, credits | [book/](https://github.com/helix-editor/helix/tree/master/book/src), with a `generated/` directory |
| [Syncthing](https://github.com/syncthing/syncthing) | 470 | Goals, getting started, build, signed releases | separate docs repo, linked as "source" |
| [gh](https://github.com/cli/cli) | 648 | One paragraph, install per OS, "for usage see the manual" | manual site; `.github/CONTRIBUTING.md` is 620 words |
| [Zellij](https://github.com/zellij-org/zellij) | 707 | What it is, install, a five-line development start | zellij.dev documentation |
| [uv](https://github.com/astral-sh/uv) | 1,080 | Highlights, install, feature tour, link to docs | docs site; CLI reference from `uv help`; docs deploy on release |
| [Starship](https://github.com/starship/starship) | 1,338 | Install and shell setup | `docs/` in the repository (VitePress, 20+ locales) |
| [Caddy](https://github.com/caddyserver/caddy) | 1,415 | Features, install, quick start, "Full documentation" | caddyserver.com/docs |
| [ripgrep](https://github.com/BurntSushi/ripgrep) | 2,874 | Comparisons, install, build | GUIDE.md (6,408), FAQ.md (6,571); CONTRIBUTING.md is 26 words |

Torlek's README (617) sits in the middle. Length alone does not separate good from bad: ripgrep's
is the longest and works because it opens with a quick-links list.

Practices that recur, and what each means here:

| Practice | Evidence | Fit |
| --- | --- | --- |
| The README gets a reader to install and a first success, then links. | [GitHub Docs](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-readmes): "A README should only contain information necessary for developers to get started using and contributing"; gh, Zellij, Helix, Syncthing | Fits. Torlek's already does. |
| Navigation is by reader task. | uv's top level is Getting started, Guides, Concepts, Reference ([mkdocs.yml](https://raw.githubusercontent.com/astral-sh/uv/main/mkdocs.yml)); Caddy's is Install, Tutorials, Reference, Develop; [Diátaxis](https://diataxis.fr/) names four needs: tutorials, how-to guides, reference, explanation | Fits as a lens. Diátaxis itself says [do not create empty sections or plan top-down](https://diataxis.fr/how-to-use-diataxis/); improve pages and the structure emerges. |
| Reference is generated or purely descriptive. | uv: `cargo dev generate-all` ([CONTRIBUTING](https://raw.githubusercontent.com/astral-sh/uv/main/CONTRIBUTING.md)); Helix `generated/`; Diátaxis [reference](https://diataxis.fr/reference/): "Describe and only describe" | Fits. The site already generates CLI help and config defaults from the binary. |
| Contributors get a short guide and one architecture map. | matklad, [ARCHITECTURE.md](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html): "Keep it short", "only specify things that are unlikely to frequently change", "a map of a country, not an atlas of maps of its states", name files but do not link them; [rust-analyzer's](https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/docs/book/src/contributing/architecture.md) is 4,014 words for 40+ crates | Fits. `docs/ARCHITECTURE.md` exists but maps `apps/`. |
| Decision records are short, immutable and superseded, not edited. | [Nygard](https://www.cognitect.com/blog/2011/11/15/documenting-architecture-decisions): "one or two pages", reversed decisions are kept and marked superseded; [Backstage](https://github.com/backstage/backstage/tree/master/docs/architecture-decisions) keeps 17 files, one a template. None of the five contributor guides I read (ripgrep, uv, Zellij, Starship, gh) mentions decision records. | Fits in form. Count and placement are #548's. |
| Docs follow the release. | uv deploys docs on release; [Kubernetes](https://kubernetes.io/docs/contribute/style/content-organization/) offers a version menu (v1.33 to v1.37); [Keep a Changelog](https://keepachangelog.com/en/1.1.0/): "Changelogs are for humans, not machines" | Fits. #102 already versions the site; `CHANGELOG.md` is generated. |
| Plain, direct prose. | [Google style guide](https://developers.google.com/style/highlights): second person, active voice, "Put conditions before instructions", "Don't pre-announce anything", descriptive link text | Fits; rules below use it. |
| Docs ship with the code change. | [Write the Docs](https://www.writethedocs.org/guide/docs-as-code/): same tools, review and merge gates as code | Fits; rule 3 below. |

What does not fit, and why: a **separate documentation repository** (Syncthing, Astral), because we
need one authority per topic and the site already stages the repository's Markdown; **translations**
(Starship); a **new generator** (mdBook, MkDocs), because Theo approved the Fumadocs site in
issue #102 and nothing here is blocked by it; **word limits**, because the evidence shows a navigable long
page works and a repeated short one does not.

## Documentation map

Five kinds of document, one reader each. A page serves one reader; a fact has one authority and
everything else links to it.

| Kind | Reader | Home | Published on the site |
| --- | --- | --- | --- |
| Front door | Someone deciding to try Torlek | `README.md` | Install page |
| User docs | Someone running Torlek | `docs/QUICKSTART.md`, `docs/USER-GUIDE.md`, `docs/KNOWN-GAPS.md` | Get started, Guide |
| Generated reference | Someone looking up a command or setting | built from the binary and protocol schema | Reference |
| Contributor docs | Someone changing Torlek | `CONTRIBUTING.md`, `docs/DEVELOPMENT.md`, `docs/ARCHITECTURE.md`, crate READMEs and rustdoc, `docs/RELEASING.md` | Project (contributing, development, architecture) |
| Records | Someone asking why or when | `docs/decisions/`, `docs/design/`, `docs/research/`, `docs/audits/`, `docs/acceptance/`, `docs/superpowers/` | Repository only, except design reports |
| Agent-facing | Agents | `AGENTS.md`, `CLAUDE.md`, `.agents/`, `crates/cockpit-agent-context/bundle/` | the shipped bundle, as Reference |

One authority per topic:

| Topic | Authority | Everyone else |
| --- | --- | --- |
| What Torlek is, platforms | README, two sentences | link |
| Install | README "Install" (the site's Install page stages it) | link |
| First session | QUICKSTART | README and guide link |
| Approvals, providers, settings | USER-GUIDE task sections | QUICKSTART gives three lines and links |
| Known limits | KNOWN-GAPS (77 words; the model to copy) | link; release notes quote it |
| CLI and configuration | generated | never retyped |
| Architecture | `docs/ARCHITECTURE.md` codemap | crate READMEs say only what the crate owns |
| Why a choice was made | one record ([#548](https://github.com/Androlax2/agent-cockpit/issues/548)) | link the record |

**Site shape.** Keep the approved r4 layout and the four groups. Change what feeds them: Get started
= README Install plus QUICKSTART; Guide = approvals, providers, configuration, updates, phone
status, support; Reference = generated pages and the agent bundle; Project = contributing,
development, architecture, design reports. Drop decision records from navigation until #548 rules;
they stay searchable in the repository. This is a change to `site/scripts/content.mjs` only.

## Writing rules

Derived from the evidence above and from the repetition measured here. Ten, not a checklist:

1. **Name the reader and task in the first line.** One page, one reader. If it serves two, split it.
2. **Task first.** Command or answer before explanation; numbered steps, one action each.
3. **One authority per fact.** `git grep` before writing a fact; if it exists, link in one sentence.
   A change that alters behavior updates its page in the same PR.
4. **Reference only describes.** No story, advice or history in a reference page; generate it when
   the binary can.
5. **State what is true now.** No issue numbers, milestones, "no longer" or "since" in user pages.
   Status goes in KNOWN-GAPS and release notes. Issue numbers belong in contributor pages,
   records and KNOWN-GAPS, where they locate the rationale or the tracking issue.
6. **A caveat appears once, at the step that needs it.** Security and data-loss caveats keep their
   full wording (unsandboxed agents act with your rights; an approval runs once). Delete the
   repeats, not the warning.
7. **One short runnable example, with fictional data.** Show output only when the reader must
   compare it.
8. **Plain words.** Second person, active voice, conditions before instructions, sentence-case
   headings that name a task or a noun the reader would search for.
9. **No implementation diary.** No commit hashes, test lists, "reports N" or acceptance evidence in
   a guide. That goes in the PR or a record.
10. **Relative links inside the repository.** The site rewrites them. A moved page fixes its
    inbound links in the same PR; `lychee` in CI catches misses.

Test for any paragraph: delete it. If a reader could still finish the task or decision, it stays
deleted.

## Before and after

Three excerpts. Each keeps its necessary meaning; what moves is named.

### README: opening

Before (77 words, intro):

> Torlek runs your coding agents (Claude Code, Codex, OpenCode) side by side, each in its own
> terminal session. Every agent works inside a sandbox and must ask you before it runs anything
> outside it. One screen shows all your agents, their questions and a todo list per project.
> This is an early test version (Alpha 1) for Linux and for Windows through WSL2. It may lose its
> own data between versions, so keep your work in Git.

After (74 words):

> Torlek runs your coding agents (Claude Code, Codex, OpenCode) side by side, each in its own
> terminal session, and lets an agent ask you to run commands on your machine. One screen shows
> every agent, its questions and a todo list per project.
>
> **Alpha 1, Linux and Windows through WSL2.** Torlek may lose its data between versions: keep your
> work in Git. There is no sandbox yet, so agents run with your rights (see Known gaps).

The length is the same (74 words against 77): the change is accuracy. The wording follows
USER-GUIDE. Slice 2 adds the no-sandbox line to KNOWN-GAPS, which the link needs; whoever ships
the sandbox ([#546](https://github.com/Androlax2/agent-cockpit/issues/546)) rewrites both. The proposed README keeps four parts: this opening, **Install** (the existing command block
and the sandbox consent sentence, unchanged), **First session** (`torlek daemon install`,
`torlek init`, link to QUICKSTART) and **More** (guide, contributing, security, legacy guide). Whether
the 200-word agent-install prompt stays in the README is [question 3](#questions-for-theo).

### User guide: first session, step 2

Before (224 words): the sandbox opt-out, the `required` mode's refusal, a cleanup of removed
Greywall/Greyproxy services, the `shared` desktop default, `config reload`, and the TUI bell setting
in one list item.

After (about 95 words):

> **Step 2.** Accept running agents without a sandbox. Add this to the owner configuration file
> (`torlek config show` prints its path):
>
> ```toml
> [agents]
> sandbox = "off"
> desktop_session = "withheld"
> ```
>
> `sandbox = "off"` records that you accept this: an unsandboxed agent has your rights and can act
> as you, so Torlek's grants do not bind it. `desktop_session = "withheld"` keeps agents off your
> screen and desktop session; the default, `shared`, exposes your display. If the daemon is
> already running, run `torlek config reload`.

Moved, not lost: `sandbox = "required"` behaviour and the bell setting to the guide's Configuration
section; Greywall cleanup to Updates. `config reload` stays, because a running daemon keeps the old
settings until it runs. Lost: nothing a first-session reader needs.

### Crate README: dependency edges

Before (about 250 words): the daemon's "Dependency edges" lists 18 crates with a clause on why each
is used.

After (about 45 words):

> ## Dependencies
>
> Declared in `Cargo.toml`; the allowed edges are enforced by
> `cockpit-workspace-rules/tests/crate_graph.rs`. No crate links the daemon: frontends reach it
> through `cockpit-client` over the socket.

Lost: the per-edge reason. Where one is not obvious (`cockpit-agent-context`: "the bundle it writes
and serves"), the section that uses it (Startup, Coordinator kit) already says so.

## Inventory and dispositions

Outside `apps/`. "Retire" means remove from the tree at the stated point; git keeps the history.

| Document(s) | Words | Reader and authority | Finding | Disposition |
| --- | --- | --- | --- | --- |
| `README.md` | 617 | Front door; authority for install | Sandbox contradiction; agent prompt is a third | **Shorten** lightly |
| `docs/QUICKSTART.md` | 801 | User; first session | Not on the site; repeats README opening and the guide's example; says sandboxed | **Keep**, make the authority, fix sandbox claim |
| `docs/USER-GUIDE.md` | 2,885 | User; tasks | First session duplicated; migration notes and issue numbers inline; two sections unpublished | **Shorten**; split by task in place |
| `docs/KNOWN-GAPS.md` | 77 | User | Model page | **Keep** |
| `CONTRIBUTING.md` | 1,015 | Contributor | Sound; process only | **Keep**; add a pointer to the writing rules |
| `SECURITY.md`, `CODE_OF_CONDUCT.md`, `CHANGELOG.md` | 534, 722, 42 | Public, standard | Standard files | **Keep** |
| `docs/DEVELOPMENT.md` | 1,359 | Contributor; checks | Documents `make` for `apps/`, which agents must not use | **Shorten** at cutover |
| `docs/ARCHITECTURE.md` | 707 | Contributor | Maps `apps/` (16 mentions) | **Rewrite as a crate codemap** at cutover (#66, #126) |
| `docs/RELEASING.md` | 3,534 | Maintainer | Long, accurate to a maintainer's task | **Keep**; split by task if a slice touches it |
| `docs/packaging/*` | 10,108 | Maintainer; per-channel | `arch-runtime-inventory.md` (4,308) is a survey | **Keep** four; **relocate** the inventory to `docs/research/` |
| `docs/security/repository.md` | 1,995 | Maintainer | Forge settings | **Keep** |
| `docs/design/greenfield/` | 23,171 | Contributor; target design | Authoritative until built | **Keep**; contributor entry point is ARCHITECTURE, not these |
| `crates/*/README.md` | 29,651 | Contributor; per crate | Dependency lists, issue numbers; 24 are rustdoc | **Shorten** per crate when next edited |
| `AGENTS.md`, `CLAUDE.md`, `.agents/` | 929 + rules, skills | Agents | Already compact | **Keep**; add a docs rule row |
| `docs/decisions/` (156), `docs/DECISIONS.md` | 60,433, 4,647 | Maintainer | Median 325 words, max 1,860 | **#548** |
| `docs/superpowers/` (97) | 142,165 | None now | Old plans and specs, last change 2026-11-03 | **Keep**, unpublished, listed as Records |
| `docs/research/`, `audits/`, `acceptance/`, `gates/`, `design-plans/` | about 112,000 | Maintainer; dated | Records | **Keep** as dated records, unpublished |
| `docs/legacy/README.md` | 11,685 | Legacy desktop users | Linked from README | **Retire** at cutover (#66) |
| `DESIGN.md` (6,645), `docs/chief-supervision.md`, `docs/session-links.md`, `docs/agent-docker-build.md`, `docs/security-audit-2026-10-19.md` | about 10,000 | `apps/` | Describe `apps/`; `agent-docker-build.md` cites the removed Greyproxy | **Retire** with `apps/` (#66, #126) |
| `docs/PROVIDER_CONFORMANCE.md` | 883 | Contributor | Test-suite status | **Relocate** into the provider crate READMEs at cutover |
| `site/README.md`, `site/content/` | — | Site maintainer | Belongs to #102 | **Keep** |

Link migration for every move: `git mv`, fix inbound links in the same PR, run the focused
`lychee` check CI already runs. Alpha URLs are not promised stable; from Beta 1, a renamed site
route keeps a one-line pointer page for one release. Old versioned site builds keep their old
pages.

## Reader validation

Two tests, run once before Alpha 2 and again when a slice changes the path they cover.

1. **New user.** A volunteer with no Torlek context, a clean Linux or WSL2 home, and only the Pages
   URL. Task: install and start a first session. Pass: reaches a running session with no help, and
   no step needs a second reading. Record where they paused. This is #98's open volunteer walk; one
   walk satisfies both.
2. **Contributor.** Someone who knows Rust but not Torlek, with the repository and no design
   reports. Five questions: which crate owns approvals; the focused test command; where a
   decision is recorded; how a release is cut; where the wire protocol is defined. Pass: each
   answered in two minutes from README, CONTRIBUTING, ARCHITECTURE and the site, without opening
   `docs/design/`, `docs/decisions/` or `docs/research/`.

Ongoing checks stay light: `typos` and `lychee` already run on all Markdown in CI. Add one test in
`cockpit-workspace-rules`, beside `decision_records.rs`, that fails when a root Markdown file, a
file directly in `docs/`, or a directory under `docs/` is not named in `docs/README.md` (slice 1).
Records directories are listed once each, not per file, so the map stays about 100 lines. That
keeps the map true without a reviewer.

## Plan

Owners are the worker of the issue named. Each slice is its own PR.

| # | Slice | Size | Answers and gates |
| --- | --- | --- | --- |
| 1 | `docs/README.md` (the map and writing rules, about 100 lines), a row in `AGENTS.md`, a line in CONTRIBUTING, the map test | 1 to 2 hours | Q4 A, Q5 A; Theo's go-ahead |
| 2 | First-run authority: QUICKSTART fixed and staged; guide's first session trimmed to settings; README sandbox sentence fixed | half a day | Q2 A; #98 owner; adds the no-sandbox line to KNOWN-GAPS |
| 3 | Site feed (`content.mjs`): QUICKSTART, Updates and Phone status published; decision records out of navigation | 2 to 3 hours | Q1 B; #102 owner |
| 4 | Crate READMEs: drop dependency lists and issue numbers, one crate per PR as touched | 10 minutes each | Theo's go-ahead |
| 5 | Cutover: rewrite ARCHITECTURE, retire `docs/legacy` and the `apps/` documents, trim DEVELOPMENT | half a day | #66, #126 |
| 6 | Records: apply #548; move or index `docs/superpowers/` | after #548 | #548's answer |

Order against [#570](https://github.com/Androlax2/agent-cockpit/issues/570)'s rename: do slices 1 to
4 first, so the rename edits fewer lines.

## Questions for Theo

Answered 2026-11-06: **1B, 2A, 3A, 4A, 5A** (all recommended). Each answer shaped a slice above.

1. **Who is the public site for?** A: users only, with Contributing and Architecture under
   Project; records stay in the repository. B: users and contributors, plus design reports (the
   current shape, minus decisions). C: everything as today. **Recommend B.**
2. **Which page owns the first session?** A: QUICKSTART owns it; the guide keeps settings and
   tasks. B: merge QUICKSTART into the guide. C: keep three tellings. **Recommend A**: the
   quickstart already works as a five-minute page.
3. **The README's 200-word "install with your AI agent" prompt?** A: keep it as is. B: move it to
   a site page and leave one line. C: shorten it. **Recommend A**: it is a copy-paste artifact, so
   its length is not repetition; fix only the sandbox sentence.
4. **Where do the writing rules live?** A: `docs/README.md` beside the map, linked from
   CONTRIBUTING and AGENTS.md. B: a new `.agents/rules/docs.md`. **Recommend A**: humans and agents
   read one file.
5. **Add the map test?** A: yes, in `cockpit-workspace-rules`. B: rely on review. **Recommend A**;
   it is about 30 lines and matches `decision_records.rs`.

## Sources

All accessed 2026-11-06. Method: `curl -fsSL RAW_URL | wc -w` for each file named, so counts include
markup; the 26-word ripgrep guide is that file in full; "no mention of decision records" is
`grep -i -E 'adr|decision'` over the five contributor guides, which matched only uv's
`needs-decision` label.

- READMEs and contributor guides, raw files on each default branch: tmux/tmux (`README`),
  helix-editor/helix, syncthing/syncthing, cli/cli (plus `.github/CONTRIBUTING.md`),
  zellij-org/zellij, astral-sh/uv (plus `CONTRIBUTING.md`, `mkdocs.yml`, `docs/`),
  starship/starship (plus `docs/`), caddyserver/caddy, BurntSushi/ripgrep (plus `GUIDE.md`,
  `FAQ.md`, `CONTRIBUTING.md`).
- Docs sites: [uv](https://docs.astral.sh/uv/), [Caddy](https://caddyserver.com/docs/),
  [Kubernetes content organization](https://kubernetes.io/docs/contribute/style/content-organization/).
- [Diátaxis](https://diataxis.fr/), [workflow](https://diataxis.fr/how-to-use-diataxis/),
  [reference](https://diataxis.fr/reference/).
- matklad, [ARCHITECTURE.md](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html);
  [rust-analyzer architecture](https://github.com/rust-lang/rust-analyzer/blob/master/docs/book/src/contributing/architecture.md).
- Nygard, [Documenting architecture decisions](https://www.cognitect.com/blog/2011/11/15/documenting-architecture-decisions);
  [MADR](https://adr.github.io/madr/); [Backstage ADRs](https://github.com/backstage/backstage/tree/master/docs/architecture-decisions).
- [GitHub Docs: About READMEs](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-readmes);
  [Google developer documentation style guide](https://developers.google.com/style/highlights);
  [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/);
  [Write the Docs: docs as code](https://www.writethedocs.org/guide/docs-as-code/).
- This repository at `728c3fff`: `README.md`, `docs/`, `site/scripts/content.mjs`, `crates/*/README.md`,
  `.github/workflows/ci.yml` and `docs.yml`.
