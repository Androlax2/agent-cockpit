# Documentation map and writing rules

Read this before you add, move or rewrite a page under `docs/` or a Markdown file at the
repository root. It tells you which page owns a topic and how to write it. Humans and agents
read this one file for documentation rules; [AGENTS.md](../AGENTS.md) and [CONTRIBUTING](../CONTRIBUTING.md) link here.

A page serves one reader. A fact has one authority; every other page links to it.
The plan behind this map is [the documentation plan](research/2026-11-06-618-documentation-plan.md).

## Where things live

Every root Markdown file, every file directly in `docs/` and every directory under `docs/`
appears below as the first cell of a table row. A test in `cockpit-workspace-rules` fails when one is missing.

### Front door and policy

| Page | Reader and task |
| --- | --- |
| `README.md` | Deciding to try Torlek; install |
| `CHANGELOG.md` | Seeing what a release changed |
| `SECURITY.md` | Reporting a vulnerability |
| `CODE_OF_CONDUCT.md` | Knowing the rules of the community |

### User docs

| Page | Reader and task |
| --- | --- |
| `docs/QUICKSTART.md` | Running a first session |
| `docs/USER-GUIDE.md` | Approvals, providers, settings |
| `docs/KNOWN-GAPS.md` | Knowing the current limits |

### Contributor docs

| Page | Reader and task |
| --- | --- |
| `CONTRIBUTING.md` | Opening a pull request |
| `docs/DEVELOPMENT.md` | Building and running the checks |
| `docs/ARCHITECTURE.md` | Finding which code owns what |
| `docs/RELEASING.md` | Cutting a release |
| `docs/PROVIDER_CONFORMANCE.md` | Checking a provider against the conformance suite |
| `docs/packaging/` | Maintaining one distribution channel |
| `docs/security/` | Maintaining the repository's forge settings |
| `docs/redteam-vm.md` | Building and resetting the disposable red-team VM, and briefing the red-team |

### Agent-facing

| Page | Reader and task |
| --- | --- |
| `AGENTS.md` | An agent starting work here |
| `CLAUDE.md` | An agent learning what Git actions are authorized |

### Records

Dated or frozen. They say why or when, never how to use Torlek. Listed once per directory.

| Location | Holds |
| --- | --- |
| `docs/decisions/` | One decision per file ([how](decisions/README.md)) |
| `docs/DECISIONS.md` | Frozen history of early decisions |
| `docs/design/` | Design reports, including the greenfield target |
| `docs/research/` | Dated research and proposals |
| `docs/audits/` | Dated audits |
| `docs/acceptance/` | Acceptance evidence |
| `docs/gates/` | Milestone gate records |
| `docs/superpowers/` | Old plans and specs |

### Describes the old cockpit under `apps/`

Retired at the cutover (#66). Do not extend them.

| Page | Describes |
| --- | --- |
| `DESIGN.md` | The old desktop interface |
| `docs/legacy/` | The old desktop programs, for their users |
| `docs/agent-docker-build.md` | The old agent image build |
| `docs/chief-supervision.md` | The old supervisor |
| `docs/session-links.md` | The old session links |
| `docs/security-audit-2026-10-19.md` | An audit of the old cockpit |

## One authority per topic

| Topic | Authority |
| --- | --- |
| What Torlek is, install | `README.md` |
| First session | `docs/QUICKSTART.md` |
| Approvals, providers, settings | `docs/USER-GUIDE.md` |
| Known limits | `docs/KNOWN-GAPS.md` |
| Commands and configuration | Generated from the binary; never retype them |
| Which crate owns what | `docs/ARCHITECTURE.md`; a crate README says only what that crate owns |
| Why a choice was made | One file under `docs/decisions/` |

## Writing rules

1. **Name the reader and task in the first line.** One page, one reader. If it serves two, split it.
2. **Task first.** Command or answer before explanation; numbered steps, one action each.
3. **One authority per fact.** `git grep` before writing a fact; if it exists, link it in one
   sentence. A change that alters behavior updates its page in the same pull request.
4. **Reference only describes.** No story, advice or history. Generate it when the binary can.
5. **State what is true now.** No issue numbers, milestones, "no longer" or "since" in user
   pages. Status goes in `docs/KNOWN-GAPS.md` and release notes. Issue numbers belong in
   contributor pages, records and `docs/KNOWN-GAPS.md`.
6. **A caveat appears once, at the step that needs it.** Security and data-loss caveats keep
   their full wording; delete the repeats, not the warning.
7. **One short runnable example, with fictional data.** Show output only when the reader must
   compare it.
8. **Plain words.** Second person, active voice, conditions before instructions, sentence-case
   headings that name a task or a noun the reader would search for.
9. **No implementation diary.** No commit hashes, test lists or acceptance evidence in a
   guide. That goes in the pull request or a record.
10. **Relative links inside the repository.** The site rewrites them. A moved page fixes its
    inbound links in the same pull request; `lychee` in CI catches misses.

Test for any paragraph: delete it. If a reader could still finish the task or decision, it
stays deleted.

## Adding a page

1. Pick the reader and the task. If an authority above covers it, edit that page instead.
2. Add the page to the table it belongs to, in the same pull request.
3. Run the `docs_map` test of `cockpit-workspace-rules`; [Development](DEVELOPMENT.md) and
   [AGENTS.md](../AGENTS.md) give the cargo commands.
