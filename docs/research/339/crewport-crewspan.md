# Crewport and Crewspan — bounded naming research for #339

Observation date: **2026-11-02**. Scope: two candidate names for a personal, terminal-first Rust AI coding-agent orchestrator. Evidence only; no naming selection, registration, implementation, or trademark clearance.

Method: public unauthenticated GET requests to named registries and GitHub APIs, plus bounded web searches. HTTP statuses below are actual returned statuses; `timeout` means **no HTTP status or result count was obtained**, not 404 or zero matches. Web-search tools expose result cards, not authoritative total counts or source HTTP status. Search-index excerpts are explicitly distinguished from fetched primary documents. No explicit 403, proxy-denial message, or blocked-path message was returned; repeated `Error: The operation timed out.` / `Request timed out` failures have unknown cause. No access-control bypass was attempted.

## 1. What existing software, package, repository, and brand uses collide?

### Takeaway

**CrewPort already names an AI-agent-work marketplace**, with first-party public material describing “contract enforcement as a service for AI agent crews.” **Crewspan already has an exact GitHub organization, a field-service software project, and an exact-name fork of Paperclip, an AI-agent orchestrator**. These are positive collision findings irrespective of the incomplete package checks. [CrewPort marketing repository](https://github.com/ologos-repos/crewport-ads/blob/main/README.md); [Crewspan organization API](https://api.github.com/users/crewspan); [field-service README](https://github.com/Hailemeskel-Yigrem/crewspan/blob/main/README.md); [Paperclip fork API](https://api.github.com/repos/sico-vibes/crewspan).

### Cited Findings

#### Package lookup matrix

Each linked URL preserves the literal submitted query or exact package identifier. A 404 at an exact package API is evidence that the requested public record was not returned, **not** proof that registration is permitted. These checks do not enumerate every package's installed executable files.

| Source and scope | `crewport` | `crewspan` |
| --- | --- | --- |
| crates.io search; `q=<name>`, page 1, 10 results requested | [Request](https://crates.io/api/v1/crates?page=1&per_page=10&q=crewport): timeout; count unavailable. A second fetch of this URL also returned `Request timed out`. | [Request](https://crates.io/api/v1/crates?page=1&per_page=10&q=crewspan): timeout; count unavailable. |
| Homebrew **core formula**, exact JSON identifier | [Request](https://formulae.brew.sh/api/formula/crewport.json): timeout; count unavailable. A second fetch also timed out. | [Request](https://formulae.brew.sh/api/formula/crewspan.json): timeout; count unavailable. |
| Homebrew **cask**, exact JSON identifier | [Request](https://formulae.brew.sh/api/cask/crewport.json): timeout; count unavailable. | [Request](https://formulae.brew.sh/api/cask/crewspan.json): timeout; count unavailable. |
| Arch **official**, `name=<name>` | [Request](https://archlinux.org/packages/search/json/?name=crewport): **200**, `valid:true`, `results:[]`, **`count:0`**. | [Request](https://archlinux.org/packages/search/json/?name=crewspan): **200**, `valid:true`, `results:[]`, **`count:0`**. |
| Arch **AUR**, exact `info` lookup | [Request](https://aur.archlinux.org/rpc/v5/info?arg[]=crewport): **200**, `type:"multiinfo"`, **`resultcount:0`**, `results:[]`. | [Request](https://aur.archlinux.org/rpc/v5/info?arg[]=crewspan): **200**, `type:"multiinfo"`, **`resultcount:0`**, `results:[]`. |
| Debian, names, all suites and sections | [Request](https://packages.debian.org/search?keywords=crewport&searchon=names&suite=all&section=all): timeout; count unavailable. | [Request](https://packages.debian.org/search?keywords=crewspan&searchon=names&suite=all&section=all): timeout; count unavailable. |
| Ubuntu, names, all suites and sections | [Request](https://packages.ubuntu.com/search?keywords=crewport&searchon=names&suite=all&section=all): **200**, says names “contain crewport,” all architectures; **“Sorry, your search gave no results.”** Zero displayed substring results, hence no exact result in this search. | [Request](https://packages.ubuntu.com/search?keywords=crewspan&searchon=names&suite=all&section=all): **200**, says names “contain crewspan,” all architectures; **“Sorry, your search gave no results.”** Zero displayed substring results. |
| Fedora package search, `query=<name>` | [Request](https://packages.fedoraproject.org/search?query=crewport): timeout; search semantics and count not verified. | [Request](https://packages.fedoraproject.org/search?query=crewspan): timeout; search semantics and count not verified. |
| Nix package search, unstable channel | [Request](https://search.nixos.org/packages?channel=unstable&query=crewport): timeout; count unavailable. | [Request](https://search.nixos.org/packages?channel=unstable&query=crewspan): timeout; count unavailable. |
| npm, exact **unscoped** package | [Request](https://registry.npmjs.org/crewport): **404**, literal `{"error":"Not found"}`. | [Request](https://registry.npmjs.org/crewspan): **404**, literal `{"error":"Not found"}`. |
| PyPI, exact project | [Request](https://pypi.org/pypi/crewport/json): **404**, literal `{"message": "Not Found"}`. | [Request](https://pypi.org/pypi/crewspan/json): **404**, literal `{"message": "Not Found"}`. |

#### GitHub: counts versus exact names

All GitHub requests below returned **HTTP 200**. Repository-search responses had **`incomplete_results:false`**. Search `in:name` still returned substring and owner-name hits, so its total is not an exact repository-name count. Exact counts below were calculated by case-insensitive equality of the returned repository's **`name`**, not `full_name`.

| Lookup | Crewport | Crewspan |
| --- | --- | --- |
| Initial literal repository query | [`crewport in:name`, `per_page=30`](https://api.github.com/search/repositories?q=crewport+in:name&per_page=30): **24** total. Initial tool display truncated item details; exact counting uses the complete compact follow-up below. | [`crewspan in:name`, `per_page=30`](https://api.github.com/search/repositories?q=crewspan+in:name&per_page=30): **1** total, exact `Hailemeskel-Yigrem/crewspan`. |
| Include forks, complete bounded page | [`crewport in:name fork:true`, `per_page=100`](https://api.github.com/search/repositories?q=crewport+in:name+fork:true&per_page=100): **28** total and returned; **3 exact basename matches**, all non-forks. | [`crewspan in:name fork:true`, `per_page=100`](https://api.github.com/search/repositories?q=crewspan+in:name+fork:true&per_page=100): **2** total and returned; **2 exact basename matches**, one non-fork and one fork. |
| Exact owner handle, shared user/org namespace | [`/users/crewport`](https://api.github.com/users/crewport): `login:"crewport"`, **`type:"User"`**, 3 public repositories, created `2025-07-26T02:36:25Z`. The exact handle is occupied by a user, not an organization. | [`/users/crewspan`](https://api.github.com/users/crewspan): `login:"crewspan"`, **`type:"Organization"`**, 0 public repositories, created `2026-09-08T16:18:34Z`. Exact organization exists even though it has no public repos. |

Exact Crewport repository basenames:

1. [`9Michael-Cooley/CrewPort`](https://github.com/9Michael-Cooley/CrewPort): metadata describes “Artificially orchestrated microservices” and “AI-driven workflows.” [README API, 200](https://api.github.com/repos/9Michael-Cooley/CrewPort/readme) claims development-workflow and CLI support. Its prose is generic, repeats boilerplate, and calls itself Python while repository metadata labels TypeScript; treat this as **name-use evidence**, not proof of a substantial working competitor.
2. [`savithapreflight/crewport`](https://github.com/savithapreflight/crewport): exact name, no description; GitHub metadata identifies TypeScript. [Search evidence](https://api.github.com/search/repositories?q=crewport+in:name+fork:true&per_page=100).
3. [`Sriramrajugit/crewport`](https://github.com/Sriramrajugit/crewport): exact name, no description; GitHub metadata identifies JavaScript. [Search evidence](https://api.github.com/search/repositories?q=crewport+in:name+fork:true&per_page=100).

Exact Crewspan repository basenames:

1. [`Hailemeskel-Yigrem/crewspan`](https://github.com/Hailemeskel-Yigrem/crewspan): [README API, 200](https://api.github.com/repos/Hailemeskel-Yigrem/crewspan/readme) calls it a **“Multi-tenant field service operations platform for scheduling technicians, managing work orders, inventory, invoicing, and SLA compliance.”** README describes FastAPI, React, Celery, and a Python SDK.
2. [`sico-vibes/crewspan`](https://github.com/sico-vibes/crewspan): [repository API, 200](https://api.github.com/repos/sico-vibes/crewspan) identifies **parent and source `paperclipai/paperclip`**, description “The open-source app everyone uses to manage agents at work,” homepage `https://paperclip.ing`, creation `2026-09-24T16:47:01Z`, last push `2026-10-14T23:41:21Z`. [README API, 200](https://api.github.com/repos/sico-vibes/crewspan/readme) still brands the software **Paperclip** and says “Open-source orchestration for teams of AI agents.” This is **an exact repo-name collision in the same category, not evidence of an independently branded Crewspan AI product**.

Non-exact hits and materially related brands:

- **CrewPort AI marketplace — strong primary evidence.** [`ologos-repos/crewport-ads` README](https://github.com/ologos-repos/crewport-ads/blob/main/README.md), retrieved via [API, 200](https://api.github.com/repos/ologos-repos/crewport-ads/readme), says **“Marketing collateral for CrewPort — contract enforcement as a service for AI agent crews.”** Its lead positioning is **“The AI agent freelance marketplace.”** It names developers running AI agent crews as operators and links [crewport.ai](https://crewport.ai). The repository basename `crewport-ads` is a suffix hit, but the **product brand CrewPort is exact**.
- **CrewPort agent protocol — strong primary evidence.** [`crewport-repos/AICP` README](https://github.com/crewport-repos/AICP/blob/main/README.md), retrieved via [API, 200](https://api.github.com/repos/crewport-repos/AICP/readme), defines an MCP identity profile, agent identities, scopes, and lifecycle-aware tool projection. Its implementation table lists **CrewPort** at `https://crewport.ai` and says the protocol is maintained by CrewPort. `crewport-repos` is not the exact organization handle `crewport`; `AICP` is not an exact repository basename.
- **Crewport hiring platform — index evidence only.** Search `"Crewport"` returned [crew-port.com](https://crew-port.com), describing hiring conversations and recruiting shortlists; another excerpt names AI interviewers Aria and Zara. Direct `fetch` and `webfetch` timed out. Domain spelling has a hyphen, but displayed **Crewport** brand is exact.
- **CrewSpan field-service brand — index evidence only.** Search `"Crewspan"` returned [www.crewspan.app](https://www.crewspan.app): **“Field service software for HVAC, plumbing and electrical shops. Coming soon. Already using CrewSpan? Sign in.”** Both direct fetch methods timed out. Do not infer a relationship with the similarly named GitHub project or organization without evidence.
- Other exact Crewport brands surfaced in index excerpts: [crewport.eu](https://crewport.eu), ground transportation for flight crews; [mycrewport.com](https://mycrewport.com), maritime sea-time and CV tools; [www.crewport.io](https://www.crewport.io), music distribution with an AI release wizard. These are broader discoverability collisions, not independently fetched legal identities.
- **CrewAI is a similar, not exact, AI brand.** [Official repository API, 200](https://api.github.com/repos/crewAIInc/crewAI) describes a **“Framework for orchestrating role-playing, autonomous AI agents.”** Both candidates share its “crew” team/orchestration vocabulary; neither is an exact `CrewAI` spelling match. Direct `https://www.crewai.com` fetch timed out.
- Search `"Crewspan"` also returned **CrewSnap** at [crewsnap.app](https://crewsnap.app) and [crewsnap.com](https://www.crewsnap.com), and **Creospan** on [its LinkedIn page](https://www.linkedin.com/company/creospan). These are **similar spellings, not exact matches**; returned excerpts describe scheduling/staffing software and technology consulting respectively. No claims about common ownership or legal conflict are made.
- Examples deliberately excluded from exact-name counts: `crewporttest`, `crewportal`, `CrewPortraits`, `CrewPortalHub`, and owner-name matches such as `crewport/gpt`. The latter's description is “AI chatbot for Crewport — smart assistant for seafarers,” a further brand-use clue but not a repository named `crewport`. [Complete search response](https://api.github.com/search/repositories?q=crewport+in:name+fork:true&per_page=100).

#### Literal discovery searches

| Literal web query | Returned evidence and limits |
| --- | --- |
| `"Crewport"` | 8 result cards; exact brands plus substring `crewportal.emirates.com`. No authoritative total count. |
| `"Crewspan"` | 8 cards; exact field-service brand, social account, similar CrewSnap/Creospan, and unrelated CREW. No authoritative total count. |
| `"crewport.ai"` | 8 cards; exact marketplace, hiring and music brands, and historical Crewport references. No authoritative total count. |

### Inferences

- Crewport has a **direct AI-agent category collision**, independently supported by two fetched first-party repository READMEs, not merely a search snippet or generic “crew” overlap.
- Crewspan has a **confirmed exact software-name and organization collision**. Its Paperclip fork adds same-category repository ambiguity, but retaining Paperclip's README means we should not inflate it into a separate established Crewspan competitor.
- Empty Arch/AUR/Ubuntu results and npm/PyPI 404s are narrower observations than these positive brand collisions. They do not establish publishability, binary uniqueness, trademark availability, or a clean search identity.

### Gaps

- **Unresolved for both candidates:** crates.io, Homebrew core, Homebrew casks, Debian, Fedora, Nix. Every attempted URL and failure is recorded in the matrix. No valid zero-count claims can be made for these sources.
- No scoped npm-package enumeration, PyPI normalized-name variant sweep, historical deleted/tombstoned package review, distro file-content search, private GitHub search, or comprehensive code search was performed. AUR `-git` / `-bin` packages were not enumerated.
- Search excerpts are not verified current page bodies when direct fetch failed. Brand use does not establish a registered trademark, company ownership, market share, or operational maturity.

## 2. What domain-registration and software-class trademark evidence is available?

### Takeaway

**Registration status of all four requested domains remains unknown. USPTO and EUIPO class-9/42 searches could not be completed through their public portals.** These are unavailable checks, never clearance or “no match.”

### Cited Findings

#### Domain requests

| Candidate domain | Public lookup attempted | Result |
| --- | --- | --- |
| `crewport.dev` | [RDAP bootstrap lookup](https://rdap.org/domain/crewport.dev) | `Error: The operation timed out.` No HTTP status, redirect destination, registration record, or result count obtained. |
| `crewport.sh` | [RDAP bootstrap lookup](https://rdap.org/domain/crewport.sh) | Same timeout; registration unknown. |
| `crewspan.dev` | [RDAP bootstrap lookup](https://rdap.org/domain/crewspan.dev) | Same timeout; registration unknown. |
| `crewspan.sh` | [RDAP bootstrap lookup](https://rdap.org/domain/crewspan.sh) | Same timeout; registration unknown. |

- Attempt to retrieve [IANA's DNS RDAP bootstrap data](https://data.iana.org/rdap/dns.json), to identify authoritative TLD services, also timed out. No authoritative RDAP response was obtained; `rdap.org` is a lookup/bootstrap service, not itself registration evidence.
- Literal web query `"crewport.dev" OR "crewport.sh" OR "crewspan.dev" OR "crewspan.sh"` returned **8 unrelated cards**, including staffing sites and an onboarding video. None supplied registration evidence. The search tool supplied no total count or source HTTP statuses; this is **not a zero-domain or availability result**.
- Actual other-TLD brand use is independently evidenced by the CrewPort marketing README linking [crewport.ai](https://github.com/ologos-repos/crewport-ads/blob/main/README.md). It does not establish registration status of `.dev` or `.sh`.

#### Trademark requests and limitations

| Office | Public portal attempted | Intended bounded name/class search | Observed result |
| --- | --- | --- | --- |
| USPTO | [Trademark Search](https://tmsearch.uspto.gov/) | Exact `CREWPORT` and `CREWSPAN`; goods/services in international classes **009 and 042**, including live and pending records | Root GET timed out. **No portal search was submitted; no class filter applied; result counts unavailable.** |
| EUIPO | [eSearch plus](https://euipo.europa.eu/eSearch/) | Exact `CREWPORT` and `CREWSPAN`; Nice classes **9 and 42** | Root GET timed out. **No portal search was submitted; no class filter applied; result counts unavailable.** |

Supplementary public web-index queries, **not substitutes for office database searches**:

- Literal `site:uspto.gov ("crewport" OR "crewspan") ("009" OR "042" OR "trademark")` returned **8 cards** about other marks, including ARCHER COMMAND, PALWORLD ONLINE and AMERICAN LEGEND HOMES. No card identified either candidate as a mark. Some irrelevant snippets contained dates beyond the requested observation date, so they are not used as date-bounded evidence. **No authoritative exact-match count, status, owner, serial number, or class record was established.** [Target official portal](https://tmsearch.uspto.gov/).
- Literal `site:euipo.europa.eu ("crewport" OR "crewspan") ("9" OR "42")` returned **8 cards**, mainly examination guidelines and TMclass software terminology. No card identified either candidate as a mark. **No authoritative exact-match count was established.** One official [TMclass result](https://euipo.europa.eu/ec2/search/find?language=en&text=software&searchMode=WORDSPREFIX&officeList=&page=1&size=25&sortBy=relevance) showed software in class 9 and software porting in class 42; its **1,237 results concern the word `software`, not either candidate**, and must not be repurposed as name-search counts.

### Inferences

- Existing AI/software use makes real legal follow-up material even if a later exact wordmark query returns zero. The gathered evidence supports a collision concern, not a legal conclusion.
- No claim that any requested domain is registered, unregistered, purchasable, or reasonably priced is justified. Search indexing, DNS behavior, and registration status are different questions.

### Gaps

- All four `.dev`/`.sh` registration checks are unresolved. No registrar checkout, purchase, account, WHOIS workaround, or unverified availability service was used.
- USPTO and EUIPO **class 9/42 clearance is incomplete for both names**. Exact and similar mark records, owners, status, territorial scope, and goods/services remain unchecked. Public-search timeout is the precise limitation; no evidence identified it as a 403 or JS-only response.

## 3. Do the strings work as short commands and XDG names, and what do they suggest in English/French?

### Takeaway

Both strings satisfy the mechanical **8 ASCII letters** constraint and support straightforward command and directory spellings. That syntactic fit does not resolve the positive identity collisions above; interpretation and pronunciation are qualitative observations, not user-tested findings.

### Cited Findings

- The [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/latest) search excerpt assigns configuration to `$XDG_CONFIG_HOME`, persistent restart state to `$XDG_STATE_HOME`, and sockets/other runtime objects to `$XDG_RUNTIME_DIR`. It specifies the runtime directory's owner-only permissions and mode 0700. Literal search was `site:specifications.freedesktop.org "XDG_STATE_HOME" "XDG_RUNTIME_DIR"`; **8 official result cards** returned. Direct fetch of [the initially attempted URL](https://specifications.freedesktop.org/basedir-spec/latest/) timed out, so this statement is based on indexed official text, not a successfully fetched full specification.
- Literal query `"crewport daemon" OR "crewport-next" OR "crewport-dev" OR "crewspan daemon" OR "crewspan-next" OR "crewspan-dev"` returned **8 cards**, including the CrewPort marketplace and music platform, and unrelated daemon references. **No displayed card established an exact invocation/suffixed executable collision.** The engine apparently broadened the query; there is no reliable zero count. The original [CrewPort marketplace](https://crewport.ai) and [music platform](https://www.crewport.io) results remain base-brand hits, not proof of a `crewport-dev` binary.
- Literal dictionary discovery query `site:dictionary.cambridge.org crew port span French` returned **8 cards**, including Cambridge entries for [crew, English–French](https://dictionary.cambridge.org/dictionary/english-french/crew), [port, French–English](https://dictionary.cambridge.org/dictionary/french-english/port), and [span, English](https://dictionary.cambridge.org/dictionary/english/span). Indexed `span` headings distinguish **time, length and bridge** senses; French `port` examples concern maritime ports. Direct `crew` and `span` fetches each returned `Request timed out`; the excerpts did not expose a complete bilingual definition set.
- **Crewport is also an existing place/history reference**, not solely a newly coined brand. Search surfaced a [Washington State University Libraries record titled “Crewport Labor Camp”](https://content.libraries.wsu.edu/digital/collection/nash/id/8275), with original date “Fall 1967,” and a [Revisiting Washington historical account](https://revisitwa.org/waypoint/crewport-labor-camp). This evidence is limited to returned indexed excerpts; neither page was fetched in this bounded pass.

### Inferences

#### Mechanical inspection of the supplied strings — no commands executed

| Aspect | Crewport | Crewspan |
| --- | --- | --- |
| ASCII lowercase binary token | `crewport`: **8 letters** | `crewspan`: **8 letters** |
| Stable invocation | `crewport` | `crewspan` |
| Daemon subcommand spelling | `crewport daemon` | `crewspan daemon` |
| Next-channel binary | `crewport-next`: **13 characters** | `crewspan-next`: **13 characters** |
| Development binary | `crewport-dev`: **12 characters** | `crewspan-dev`: **12 characters** |
| Configuration component | `$XDG_CONFIG_HOME/crewport/` | `$XDG_CONFIG_HOME/crewspan/` |
| State component | `$XDG_STATE_HOME/crewport/` | `$XDG_STATE_HOME/crewspan/` |
| Runtime/socket example | `$XDG_RUNTIME_DIR/crewport/daemon.sock` | `$XDG_RUNTIME_DIR/crewspan/daemon.sock` |

- Both base tokens contain no whitespace, non-ASCII characters, leading hyphen, shell metacharacters, or path separator. Neither requires shell quoting as written. `<name> daemon` keeps the actual binary at 8 letters; the entire invocation is not an 8-character token. The suffix variants exceed 8 characters, so they work only if the limit applies to the base product/binary name rather than every development executable.
- Lowercase spelling gives consistent package/CLI/XDG components. Base-name directories are syntactically suitable; separate `-next` / `-dev` directory components would also be syntactically possible if independent channels are ever desired. This is a naming check, **not a proposal to implement a channel scheme**. XDG offers no global product-name reservation and cannot prevent same-name application data collisions.
- No shorter alias is selected or assessed as available. Eight-letter full names already meet the supplied short-binary constraint. Shared `crew` prefixes provide no disambiguation until the fifth character.
- **English interpretation:** `crew + port` naturally suggests a team's harbor/base or a connection point; it fits coordination but also transportation, maritime work and network-port associations. `crew + span` suggests connecting a team across work or time, but its purpose is less immediately literal. These are researcher interpretations of familiar components, not dictionary definitions of the compounds.
- **French interpretation:** `port` is immediately recognizable as a French maritime word, while `crew` and `span` read as English imports. Crewport's metaphor is therefore more accessible than Crewspan's without English knowledge. No obvious French vulgar/offensive reading stood out to the researcher, but this is **not** a linguistic or dialect clearance. “Crew-span” also invites visual transposition with **CrewSnap**, already surfaced as software branding.
- A practical spelling/pronunciation concern for both is whether a listener writes the English compound as one word or two. No pronunciation testing was conducted. Crewport's existing exact AI marketplace is more consequential than this minor usability issue.

### Gaps

- No candidate binary was installed or run, no local PATH or shell alias inventory was used, and no package file-content database was searched. Global executable uniqueness remains unproven.
- `-next` / `-dev` were assessed structurally and through one bounded public web query, **not** through a fresh full package-registry matrix for each suffix. No absence/availability claim is made for them.
- English/French assessment is qualitative. Full dictionary fetches failed; no native-speaker survey, phonetic testing, slang corpus, or exhaustive regional-meaning review was performed.
- Research notes are the only project-file change. No build or tests were run: this assignment changes no software. Registry/domain/legal gaps are intentionally retained for the coordinating researcher rather than silently converted into a naming decision.
