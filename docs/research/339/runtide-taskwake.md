# Runtide / Taskwake naming evidence for #339

Research date: **2026-11-02**. Scope: two proposed names for a personal, terminal-first Rust AI coding-agent orchestrator; public, read-only research. No naming decision or legal clearance. HTTP statuses below are observed responses, not inferred from search snippets. `Timeout` means the tool returned `Error: The operation timed out.` without an HTTP status or result count. Search-engine responses expose snippets, not authoritative database counts. Absence in a checked source does not establish availability.

## 1. What exact software and package-name collisions exist?

### Takeaway

**Taskwake has a direct same-name, same-domain collision:** an existing AI-agent background-task CLI installs `taskwake` and uses `~/.config/taskwake` and `$XDG_STATE_HOME/taskwake/tasks`. **Runtide also has exact-name software uses** (a gameplay framework and a coding playground), plus an occupied GitHub username. Neither is a clean unused name. [Taskwake README](https://github.com/sdpwh7-ux/taskwake/blob/main/README.md); [Runtide framework](https://github.com/st0ianmarius/runtide/blob/master/README.md); [Runtide playground](https://github.com/treatwashere/runtide/blob/main/README.md); [GitHub username](https://api.github.com/users/runtide).

### Cited Findings

#### Package lookup matrix

Each linked URL is the literal GET request. Registry exact-resource 404s are recorded as “no public exact resource returned,” not a promise that the name can be registered. Successful empty searches have an explicit zero count or no-results text. Timeouts remain **unavailable**.

| Source and lookup scope | Runtide | Taskwake |
| --- | --- | --- |
| crates.io, exact crate API | [GET `/api/v1/crates/runtide`](https://crates.io/api/v1/crates/runtide): timeout; count unavailable. A webfetch retry also returned `Request timed out`. | [GET `/api/v1/crates/taskwake`](https://crates.io/api/v1/crates/taskwake): timeout; count unavailable. |
| Homebrew **core**, exact formula API | [GET `/api/formula/runtide.json`](https://formulae.brew.sh/api/formula/runtide.json): timeout; webfetch retry also timed out. | [GET `/api/formula/taskwake.json`](https://formulae.brew.sh/api/formula/taskwake.json): timeout. |
| Homebrew **casks**, exact cask API | [GET `/api/cask/runtide.json`](https://formulae.brew.sh/api/cask/runtide.json): timeout. | [GET `/api/cask/taskwake.json`](https://formulae.brew.sh/api/cask/taskwake.json): timeout. |
| Arch **official**, exact `name` filter | [GET `?name=runtide`](https://archlinux.org/packages/search/json/?name=runtide): **200**, `valid:true`, `count:0`, `results:[]`. | [GET `?name=taskwake`](https://archlinux.org/packages/search/json/?name=taskwake): **200**, `valid:true`, `count:0`, `results:[]`. |
| Arch **AUR**, exact RPC info | [GET `info?arg[]=runtide`](https://aur.archlinux.org/rpc/v5/info?arg[]=runtide): **200**, `resultcount:0`, `results:[]`, `type:"multiinfo"`. | [GET `info?arg[]=taskwake`](https://aur.archlinux.org/rpc/v5/info?arg[]=taskwake): **200**, `resultcount:0`, `results:[]`, `type:"multiinfo"`. |
| Debian, package-name search, all suites/sections | [GET `keywords=runtide&searchon=names&suite=all&section=all`](https://packages.debian.org/search?keywords=runtide&searchon=names&suite=all&section=all): timeout. | [GET `keywords=taskwake&searchon=names&suite=all&section=all`](https://packages.debian.org/search?keywords=taskwake&searchon=names&suite=all&section=all): timeout. |
| Ubuntu, **substring** package-name search, all suites/sections/architectures | [Search `runtide`](https://packages.ubuntu.com/search?keywords=runtide&searchon=names&suite=all&section=all): **200**, “You have searched for packages that names contain runtide”; “Sorry, your search gave no results”: **0 displayed**. | [Search `taskwake`](https://packages.ubuntu.com/search?keywords=taskwake&searchon=names&suite=all&section=all): **200**, same wording for taskwake: **0 displayed**. |
| Fedora packages, search | [GET `/search?query=runtide`](https://packages.fedoraproject.org/search?query=runtide): timeout; search behavior/count not verified. | [GET `/search?query=taskwake`](https://packages.fedoraproject.org/search?query=taskwake): timeout; search behavior/count not verified. |
| Nix packages, unstable search | [GET `packages?channel=unstable&query=runtide`](https://search.nixos.org/packages?channel=unstable&query=runtide): timeout; no evaluated result. | [GET `packages?channel=unstable&query=taskwake`](https://search.nixos.org/packages?channel=unstable&query=taskwake): timeout; webfetch retry also timed out. |
| npm, exact unscoped package | [GET `/runtide`](https://registry.npmjs.org/runtide): **404**, `{"error":"Not found"}`. | [GET `/taskwake`](https://registry.npmjs.org/taskwake): **404**, `{"error":"Not found"}`. |
| PyPI, exact project | [GET `/pypi/runtide/json`](https://pypi.org/pypi/runtide/json): **404**, `{"message":"Not Found"}`. | [GET `/pypi/taskwake/json`](https://pypi.org/pypi/taskwake/json): **404**, `{"message":"Not Found"}`. |

#### GitHub: exact repository names versus substring results

- Literal query **`runtide in:name`**, `per_page=100`: **HTTP 200**, `total_count:3`, `incomplete_results:false`; all **3** returned names are exactly `runtide` (case-insensitive). [API response](https://api.github.com/search/repositories?q=runtide+in:name&per_page=100).
  - [`st0ianmarius/runtide`](https://github.com/st0ianmarius/runtide): repository search reported `updated_at:2026-10-05T09:45:32Z`. Its README API and raw README both returned **200**. Description: “A deterministic TypeScript gameplay framework for abilities, effects, combat, and behavior, independent of rendering and networking.” It expressly says `"private": true` and “never published to npm or any other registry.” Its `runtide/ai` module concerns gameplay AI, not coding agents. Thus an npm 404 coexists with real software use. [README](https://raw.githubusercontent.com/st0ianmarius/runtide/master/README.md), README blob SHA `74f85ade8f472499368e089c67f957b2fb1527f5` from [README API](https://api.github.com/repos/st0ianmarius/runtide/readme).
  - [`treatwashere/runtide`](https://github.com/treatwashere/runtide): search `updated_at:2026-07-31T12:49:40Z`; README API/raw **200**. “A free, browser-based code playground for writing and testing HTML, CSS, and JavaScript instantly.” README links `https://runtide.vercel.app`; that deployment was not separately fetched. This is an exact developer-tool name collision, though not an orchestrator. [README](https://raw.githubusercontent.com/treatwashere/runtide/main/README.md), blob SHA `7f87f7bacace14693744f40d1f93d0698a8a7e88` from [README API](https://api.github.com/repos/treatwashere/runtide/readme).
  - [`runtide/runtide`](https://github.com/runtide/runtide): search `updated_at:2025-10-17T12:15:55Z`; README API **200**. README contents not examined; do not classify this as another software product from its name alone. [README API](https://api.github.com/repos/runtide/runtide/readme).
- Exact account [`runtide`](https://api.github.com/users/runtide): **200**, `type:"User"`, display name Egor, created `2025-10-16T14:35:34Z`, one public repository. Exact organization lookup [`/orgs/runtide`](https://api.github.com/orgs/runtide): **404**. The personal account is positive evidence that the username is occupied, despite no organization record.
- Literal query **`taskwake in:name`**, `per_page=100`: **HTTP 200**, `total_count:6`, `incomplete_results:false`; **4 case-insensitive exact names**, **2 longer substring names**. [API response](https://api.github.com/search/repositories?q=taskwake+in:name&per_page=100).
  - [`sdpwh7-ux/taskwake`](https://github.com/sdpwh7-ux/taskwake): exact; search `updated_at:2026-10-22T06:35:16Z`. README API/raw **200**. “A background-task ledger for AI agents, plus a webhook that wakes the agent that dispatched the task when it finishes.” Explicit Codex CLI / Claude Code support, process supervision, multi-agent ownership, persisted task state, POSIX Python CLI. Installation symlinks to **`~/.local/bin/taskwake`**. This is the strongest collision in this assignment. [README](https://raw.githubusercontent.com/sdpwh7-ux/taskwake/main/README.md), blob SHA `6ba19662d6ee90d0f47beaa76ca351190a0b3676` from [README API](https://api.github.com/repos/sdpwh7-ux/taskwake/readme).
  - [`h4nscs/TaskWake`](https://api.github.com/repos/h4nscs/TaskWake): **200**, exact ignoring case, Java, `fork:false`, null description; created `2025-10-14T14:58:25Z`, updated `2025-10-20T02:45:59Z`. Function not established.
  - [`panzeyu2013/TaskWake`](https://api.github.com/repos/panzeyu2013/TaskWake): **200**, exact ignoring case, TypeScript, `fork:false`, null description; created `2026-08-11T15:47:44Z`, updated `2026-08-12T02:27:01Z`. Function not established.
  - [`Mira190/TaskWake`](https://api.github.com/repos/Mira190/TaskWake): **200**, exact ignoring case, JavaScript, `fork:false`, null description; created `2026-07-17T10:42:48Z`, updated `2026-09-26T04:59:46Z`. Function not established.
  - [`Ravneet-project/taskwake-ai`](https://github.com/Ravneet-project/taskwake-ai): **substring/extended name, not exact**. Search describes React/Node/Express/MongoDB task management, productivity dashboard, calendar, reminders and PWA; updated `2026-10-24T13:05:55Z`. The `-ai` name does not by itself prove an AI implementation. [Search response](https://api.github.com/search/repositories?q=taskwake+in:name&per_page=100).
  - [`SGolan/TaskWakeTimer`](https://github.com/SGolan/TaskWakeTimer): **substring, not exact**; search description calls it exercise code and “a thread-class providing wake service for client threads”; updated `2024-11-15T08:21:53Z`. [Search response](https://api.github.com/search/repositories?q=taskwake+in:name&per_page=100).
- Exact [`/users/taskwake`](https://api.github.com/users/taskwake) and [`/orgs/taskwake`](https://api.github.com/orgs/taskwake): both **404**, `Not Found`. No public account/org returned; registration eligibility remains unknown.

#### Other brand and similarity evidence

- **RunTide Capital** is an existing brand using the exact proposed word plus “Capital,” not an exact package or AI-agent product. Its own search-indexed page describes investing in technology-enabled businesses and says “The tide of innovation calls… Run with us.” Direct fetch timed out; this evidence is **primary-source search snippet only**. [Company site](https://www.runtidecapital.com).
- **Rippletide** is a different, similar `-tide` AI brand. Its indexed `/dev` page calls itself an “Action Runtime for Coding Agents,” enforcing project rules at plan time and runtime for Claude Code. Direct fetch timed out: **primary-source snippet only**, not a verified exact-name collision. [Rippletide](https://www.rippletide.com/dev).
- **Taskade AI** is another distinct task/AI brand. Its indexed first-party page describes AI task management, collaboration and automation. Direct fetch timed out: **primary-source snippet only**. Do not count Taskade as an exact Taskwake match. [Taskade](https://www.taskade.com/ai/tasks).

### Inferences

- Taskwake has substantial practical confusion risk even if its package slots are absent: its existing CLI serves closely overlapping users and workflows and already occupies the natural executable/config/state names. This follows from the existing [README](https://github.com/sdpwh7-ux/taskwake/blob/main/README.md), not from a trademark determination.
- Runtide has weaker functional overlap than that Taskwake project, but established exact software uses and a taken GitHub handle prevent an “unused name” conclusion. [Framework](https://github.com/st0ianmarius/runtide); [playground](https://github.com/treatwashere/runtide); [account](https://github.com/runtide).

### Gaps

- crates.io, Homebrew core/casks, Debian, Fedora and Nix did not yield usable responses. Counts and current package occupancy there remain unknown. No error was promoted to “no match.”
- Successful package lookups cover public exact packages (and Ubuntu substring results), not every binary filename in every package, private registries, npm scoped packages, or unpublished projects.
- Public GitHub search does not cover private/deleted repositories or establish trademark rights. Unknown-function repositories are deliberately not guessed from their names.

## 2. What can be established about domains and trademarks?

### Takeaway

**Neither candidate has verified domain availability or a completed trademark search.** Public registry and trademark entry-point requests failed; software-class 9/42 result counts are unknown. Existing software use above is positive evidence independent of these gaps.

### Cited Findings

#### Domain registration attempts

| Literal lookup | Observed outcome | Meaning |
| --- | --- | --- |
| [`https://rdap.nic.google/domain/runtide.dev`](https://rdap.nic.google/domain/runtide.dev) | `TypeError: getaddrinfo ENOTFOUND rdap.nic.google` | Initial endpoint hostname failed; **not a negative response for runtide.dev**. |
| [`https://rdap.nic.google/domain/taskwake.dev`](https://rdap.nic.google/domain/taskwake.dev) | Same hostname-resolution error | **Not a negative response for taskwake.dev**. |
| [`https://data.iana.org/rdap/dns.json`](https://data.iana.org/rdap/dns.json) | webfetch `Request timed out` | Authoritative bootstrap retrieval unavailable. |
| [`https://www.registry.google/rdap-lookup`](https://www.registry.google/rdap-lookup) | Direct fetch timeout; search returned the registry's “Search the RDAP database” page | No candidate result obtained. |
| [`https://pubapi.registry.google/rdap/domain/runtide.dev`](https://pubapi.registry.google/rdap/domain/runtide.dev) | Timeout, no HTTP status | Corrected Google RDAP pattern discovered via search; registration unknown. |
| [`https://pubapi.registry.google/rdap/domain/taskwake.dev`](https://pubapi.registry.google/rdap/domain/taskwake.dev) | Timeout, no HTTP status | Registration unknown. |

Discovery query **`"dev" "pubapi.registry.google/rdap"`** exposed the concrete endpoint pattern through a [Who.is RDAP example](https://who.is/rdap/applicate.dev), alongside the [IANA .dev entry](https://www.iana.org/domains/root/db/dev.html) and [Google lookup page](https://www.registry.google/rdap-lookup). The unrelated example domain was not queried as part of this matrix. [RDAP.org documentation](https://about.rdap.org) was also discovered, but its direct fetch timed out; no alternative domain query was made through it.

#### Trademark checks: USPTO and EUIPO

| Authority | Literal request/query | Status and counts |
| --- | --- | --- |
| USPTO public trademark search | GET [`https://tmsearch.uspto.gov/`](https://tmsearch.uspto.gov/) | Timeout; webfetch retry `Request timed out`. Search UI inaccessible. **No submitted name/class search and no authoritative count.** |
| USPTO indexed-site corroboration | Web search **`site:uspto.gov "runtide" OR "taskwake"`** | Tool said “No search results found.” This is **0 returned web results**, not 0 trademarks. |
| EUIPO public search entry point | GET [`https://www.euipo.europa.eu/en/search-ip`](https://www.euipo.europa.eu/en/search-ip) | Timeout; webfetch retry `Request timed out`. **No submitted name/class search and no authoritative count.** |
| EUIPO indexed-site corroboration | Web search **`site:euipo.europa.eu "runtide" OR "taskwake"`** | Tool said “No search results found.” This is **0 returned web results**, not 0 trademarks. |

The intended database filters were exact word marks **`RUNTIDE`** and **`TASKWAKE`**, separately within Nice classes **9** and **42**; those filters could **not** be applied. They are a description of the missing check, not fabricated executed query syntax. No live/dead status, owner, application number, registration number or goods/services scope was retrieved.

### Inferences

- Search-index absence, RDAP timeouts, DNS errors and an empty public GitHub account lookup cannot justify “available.” The evidence supports only “unresolved.”
- A first-party RunTide Capital presence warrants distinguishing commercial prior use from registered software-class rights; the company snippet alone establishes neither class 9 nor class 42 protection. [First-party site](https://www.runtidecapital.com).

### Gaps

- `.dev` registration status for both candidates remains unknown. `.sh` was not checked: the assignment allowed `.dev` and/or `.sh`, and the bounded domain work focused on `.dev`. No registrar transaction, account, purchase or DNS-based availability inference was made.
- USPTO/EUIPO exact-name and similarity searches in classes 9/42 remain incomplete. No legal-clearance claim is possible from this session.
- Many direct requests timed out; no response identified an explicit proxy-denial policy or HTTP 403. The cause is undiagnosed. No blocked-host workaround or host-side command was used to obtain data.

## 3. Are the names usable as CLI/XDG identifiers, and what do they suggest in English/French?

### Takeaway

Both strings satisfy the spelling limit: **`runtide` = 7 ASCII letters**, **`taskwake` = 8 ASCII letters**. Both are technically straightforward CLI/path components, but Taskwake's natural binary and XDG locations already have documented use by the existing AI tool. [Existing Taskwake installation/configuration](https://github.com/sdpwh7-ux/taskwake/blob/main/README.md).

### Cited Findings

#### Command and namespace checks

| Form | Runtide | Taskwake |
| --- | --- | --- |
| Short binary | `runtide` — 7 letters; no executable use established from the inspected Runtide READMEs. Exact software-name collisions remain. | `taskwake` — 8 letters; **documented installed binary collision** in `~/.local/bin/taskwake`. |
| Daemon subcommand | `runtide daemon` — readable two-token command; a design possibility, not verified existing functionality. | `taskwake daemon` — readable, but changing a subcommand does not avoid the existing executable-name collision. The examined README does not list a `daemon` command. |
| Preview/development executables | `runtide-next` = 12 characters; `runtide-dev` = 11. | `taskwake-next` = 13 characters; `taskwake-dev` = 12. |
| XDG paths | Plausible `$XDG_CONFIG_HOME/runtide`, `$XDG_STATE_HOME/runtide`, `$XDG_CACHE_HOME/runtide`, `$XDG_RUNTIME_DIR/runtide`. No reservation or exclusive ownership inferred. | Same syntax with `taskwake`, but the existing project expressly uses `$XDG_STATE_HOME/taskwake/tasks` (fallback `~/.local/state/taskwake/tasks`) and `~/.config/taskwake`. |

Lengths and token syntax in the table are direct analysis of the user-supplied strings, not external availability evidence. The suffix forms exceed 8 characters; they fit only if the limit applies to the base product name. Path roles/defaults come from the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/latest/) (**HTTP 200**, version 0.8, publication date 2021-05-08). Existing binary and storage use comes from the [Taskwake README](https://raw.githubusercontent.com/sdpwh7-ux/taskwake/main/README.md) (**HTTP 200**).

Literal web searches **`"runtide daemon" "runtide-next" "runtide-dev"`** and **`"taskwake daemon" "taskwake-next" "taskwake-dev"`** returned relaxed/unrelated results rather than reliable exact-phrase counts. The latter also returned a [Taskwake listing](https://trendcueai.com/item/github-sdpwh7-ux-taskwake-760e15ac?rev=b352288b9b5154ae3a436b4d8704ff6b86de3a200536d23c851e05f9135ddb05), corroborated more strongly by the primary README. Follow-up **`"runtide-next" OR "runtide-dev" OR "taskwake-next" OR "taskwake-dev"`** likewise produced unrelated broad results. **No exact suffix hit was verified; that is inconclusive, not a clean result.** The successful GitHub base-name searches returned no repositories with these suffix names among their 3 and 6 results. [Runtide query](https://api.github.com/search/repositories?q=runtide+in:name&per_page=100); [Taskwake query](https://api.github.com/search/repositories?q=taskwake+in:name&per_page=100).

#### English and French meanings

- Cambridge's indexed English/French material gives **tide → marée** and describes the sea's regular ebbing/flowing. Direct fetch of `/dictionary/english-french/tide` timed out; evidence is **dictionary-publisher search excerpts**, retrieved with **`site:dictionary.cambridge.org "tide" "marée"`**. [Tide](https://dictionary.cambridge.org/us/dictionary/english-french/tide); [marée](https://dictionary.cambridge.org/us/dictionary/french-english/maree).
- Cambridge's indexed entry gives **wake → (s')éveiller, (se) réveiller, sillage**; its English entry also lists the **funeral/gathering** sense. Direct fetch of `/dictionary/english-french/wake` timed out; query **`site:dictionary.cambridge.org "wake" "sillage"`** returned these publisher excerpts. [Wake, French translation](https://dictionary.cambridge.org/us/dictionary/english-french/wake); [wake, English meanings](https://dictionary.cambridge.org/us/dictionary/english/wake); [sillage](https://dictionary.cambridge.org/us/dictionary/french-english/sillage).

#### Additional literal search log

These discovery queries were issued once each; they do not provide exhaustive result counts or substitute for the direct package APIs:

- **`"runtide" "taskwake"`**: mostly RunTide Capital results; useful company lead, not evidence that both terms occur together.
- **`"runtide" software AI`**: RunTide Capital and broader AI/Runway results; no new exact AI-orchestrator match verified from this query.
- **`"taskwake" AI`**: relaxed results including Tasks.AI, Taskade, Tasq.ai and Taskworld; does not negate the direct GitHub Taskwake finding.
- **`Google registry .dev RDAP endpoint`**: Google RDAP lookup page and RDAP documentation discovery.
- **`"runtide" "dev" domain`** and **`"taskwake" "dev" domain`**: mostly general `.dev` pages; no candidate-specific registration evidence. Generic templated “{domain} is available” text on the [Google .dev page](https://www.registry.google/domains/dev) is **not** a candidate result.

### Inferences

- **Runtide:** “run” + “tide” suggests executions in a flow or rhythm; nautical imagery can fit orchestration. It may also read first as running/sport or a general technology brand rather than coding-agent control. For French readers it is an English compound whose meaning depends on English familiarity; a likely intended pronunciation is “run-tide,” not a French dictionary word. These are linguistic impressions, not surveyed audience findings.
- **Taskwake:** “task” + “wake” suggests waking/resuming work or an agent after task completion—the exact semantic territory already claimed by the existing project. The wake/sillage maritime association is positive but secondary; the English funeral sense is a possible unwanted association, not necessarily the dominant reading. French readers familiar with technical English may understand it, but the `skw` consonant sequence can make pronunciation less immediate. [Existing product explanation](https://github.com/sdpwh7-ux/taskwake/blob/main/README.md); [dictionary senses](https://dictionary.cambridge.org/us/dictionary/english/wake).
- The candidate strings contain no spaces, diacritics, separators or shell metacharacters, so neither requires special shell quoting as an ordinary command token. Separate `-next`/`-dev` executable names alone would not isolate state; any eventual preview-state policy would need an explicit namespace decision. [XDG specification](https://specifications.freedesktop.org/basedir/latest/).

### Gaps

- No native-speaker survey or exhaustive French slang/offensive-meaning review was performed. No obvious French insult is apparent to this researcher, but that impression is not a lexical clearance claim. “Run”/“task” interpretations above are ordinary-language analysis rather than separately verified dictionary entries.
- No installed-PATH inspection or binary execution was performed; system-wide command availability remains untested. No distinct shortened alias was proposed or checked beyond the requested base binaries.
- These are evidence notes only. No implementation, naming choice, purchase, account creation, installation, Git write or documentation PR was performed.
