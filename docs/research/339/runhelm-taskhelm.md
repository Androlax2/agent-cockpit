# Runhelm / Taskhelm naming evidence for #339

Research date: **2026-11-02**. Scope: two candidate names for a personal, terminal-first Rust AI coding-agent orchestrator. Evidence only; no naming decision, implementation, registration, purchase, installation, account access, or Git writes. HTTP requests were unauthenticated public reads. Search-engine excerpts are identified separately from direct HTTP observations.

## 1. Are the exact package, repository, organization, and binary names already used?

### Takeaway

**Taskhelm has a direct, current executable/package collision in almost the same product space**, plus an older unrelated Python task manager. **Runhelm has an exact GitHub organization and repository collision.** Neither candidate can be characterized as a clean namespace. Several package registries were unreachable in this session; those results are unknown, not empty.

### Cited Findings

#### Base-name lookup matrix

In this table, `timeout` means the tool returned **`Error: The operation timed out.`**, without an HTTP response status or a result count. `404` means the specific public package/identity endpoint returned not found, not that the name can be registered. GitHub `in:name` is a name search, not an equality operator: exact matches were determined by inspecting returned repository names case-insensitively. Counts are observations at lookup time.

| Source and literal lookup | Runhelm | Taskhelm |
| --- | --- | --- |
| crates.io, `q=<name>&per_page=100` | [GET](https://crates.io/api/v1/crates?q=runhelm&per_page=100): timeout; count unknown | [GET](https://crates.io/api/v1/crates?q=taskhelm&per_page=100): timeout; count unknown |
| Homebrew core, exact formula slug | [GET](https://formulae.brew.sh/api/formula/runhelm.json): timeout; count unknown | [GET](https://formulae.brew.sh/api/formula/taskhelm.json): timeout; count unknown |
| Homebrew casks, exact token | [GET](https://formulae.brew.sh/api/cask/runhelm.json): timeout; count unknown | [GET](https://formulae.brew.sh/api/cask/taskhelm.json): timeout; count unknown |
| Arch official, `q=<name>` | [GET](https://archlinux.org/packages/search/json/?q=runhelm): **200**, `valid:true`, `count:0`, `results:[]` | [GET](https://archlinux.org/packages/search/json/?q=taskhelm): **200**, `valid:true`, `count:0`, `results:[]` |
| AUR current RPC, search name `by=name` | [GET](https://aur.archlinux.org/rpc/v5/search/runhelm?by=name): **200**, `resultcount:0`, `results:[]` | [GET](https://aur.archlinux.org/rpc/v5/search/taskhelm?by=name): **200**, `resultcount:0`, `results:[]` |
| Debian, `keywords=<name>&searchon=names&suite=all&section=all` | [GET](https://packages.debian.org/search?keywords=runhelm&searchon=names&suite=all&section=all): timeout; count unknown | [GET](https://packages.debian.org/search?keywords=taskhelm&searchon=names&suite=all&section=all): timeout; count unknown |
| Ubuntu, same all-suite name search | [GET](https://packages.ubuntu.com/search?keywords=runhelm&searchon=names&suite=all&section=all): **200**, “names contain runhelm”; “Sorry, your search gave no results” | [GET](https://packages.ubuntu.com/search?keywords=taskhelm&searchon=names&suite=all&section=all): **200**, “names contain taskhelm”; “Sorry, your search gave no results” |
| Fedora, `query=<name>` | [GET](https://packages.fedoraproject.org/search?query=runhelm): timeout; count unknown | [GET](https://packages.fedoraproject.org/search?query=taskhelm): timeout; count unknown |
| Nix packages, `channel=unstable&query=<name>` | [GET](https://search.nixos.org/packages?channel=unstable&query=runhelm): timeout; count unknown | [GET](https://search.nixos.org/packages?channel=unstable&query=taskhelm): timeout; count unknown |
| npm exact package metadata | [GET](https://registry.npmjs.org/runhelm): **404**, `{"error":"Not found"}` | [GET](https://registry.npmjs.org/taskhelm): **200**, exact package `taskhelm`, latest **0.4.1**, published binary **`taskhelm`** |
| PyPI exact normalized project | [GET](https://pypi.org/pypi/runhelm/json): **404**, `{"message":"Not Found"}` | [GET](https://pypi.org/pypi/taskhelm/json): **200**, exact project `taskhelm`, latest **0.3.2** |
| GitHub repository search, `<name> in:name`, `per_page=100` | [GET](https://api.github.com/search/repositories?q=runhelm+in:name&per_page=100): **200**, `total_count:1`, `incomplete_results:false`; **1 exact** | [GET](https://api.github.com/search/repositories?q=taskhelm+in:name&per_page=100): **200**, `total_count:3`, `incomplete_results:false`; **3 exact**, case-insensitive |
| GitHub shared user/org login lookup | [GET](https://api.github.com/users/runhelm): **200**, `login:runhelm`, **`type:Organization`**, `public_repos:1` | [GET](https://api.github.com/users/taskhelm): **404**, `message:"Not Found"`; no public identity returned |

Direct-fetch retries with WebFetch were attempted for the Runhelm crates query and Taskhelm core-formula, Debian, Fedora, and Nix URLs. Every retry returned **`Request timed out`**; no status or count was obtained. The source coverage remains unavailable. No response explicitly identified a policy-denied host/path, so the timeouts must not be relabelled as confirmed sandbox blocks.

#### Concrete collisions and source identities

- **Runhelm GitHub:** the sole returned repository was [`runhelm/runhelm`](https://github.com/runhelm/runhelm). Its [repository API](https://api.github.com/repos/runhelm/runhelm) returned **200**, `private:false`, `archived:false`, language `TypeScript`, created `2026-04-18T18:23:49Z`, default branch `main`, description and homepage both null. The [organization](https://api.github.com/users/runhelm) was created `2026-04-18T18:10:32Z`. This confirms exact namespace occupation but does **not** establish a relationship to `runhelm.io` or `runhelm.app`.
- **Taskhelm GitHub:** all three returned names were exact case-insensitive matches: [`Kaka-123-D/TaskHelm`](https://github.com/Kaka-123-D/TaskHelm), [`aur-archive/taskhelm`](https://github.com/aur-archive/taskhelm), and [`Nidhishd/taskhelm`](https://github.com/Nidhishd/taskhelm). The [search response](https://api.github.com/search/repositories?q=taskhelm+in:name&per_page=100) describes the first as “a local-first AI engineering manager that turns tasks into isolated workspaces, web workbenches, and runnable dev environments.” The other two descriptions were null.
- **Taskhelm npm / executable:** the [registry response](https://registry.npmjs.org/taskhelm) returned `name:"taskhelm"`, `dist-tags.latest:"0.4.1"`, modified `2026-05-14T12:01:52.585Z`, repository `git+https://github.com/Kaka-123-D/TaskHelm.git`, and latest-version `bin:{"taskhelm":"bin/taskhelm.js"}`. This is direct evidence of an existing exact short command, not merely a repository label or substring.
- **Taskhelm product overlap:** the [README](https://github.com/Kaka-123-D/TaskHelm/blob/main/README.md), also retrieved via the [GitHub contents API](https://api.github.com/repos/Kaka-123-D/TaskHelm/readme) (**200**, blob SHA `2e787bffa7fab089be538203ca611f43af0df247`), calls it a “Local-first visual workbench for parallel git-worktree work.” It manages task branches, worktrees, ports, and dev servers for single operators. It documents `taskhelm`, `taskhelm-cli`, state `~/.taskhelm/taskhelm.db`, and environment names `TASKHELM_DB`, `TASKHELM_HOME`, `TASKHELM_PORT`. Current documentation emphasizes the workbench; the repository description's “AI engineering manager” is not proof that every older AI feature remains implemented.
- **Taskhelm Python:** the [PyPI metadata](https://pypi.org/pypi/taskhelm/json) identifies Bryce Harrington as author and describes “a graphic shell that sits on Task Warrior.” Release keys are `0.2.1` and `0.3.2`; the latter's source distribution was uploaded `2012-03-15T12:02:00.056595Z`. It is a separate pre-existing task-management use, not the 2026 npm workbench.
- **Historical AUR is not current AUR:** the [archived PKGBUILD API](https://api.github.com/repos/aur-archive/taskhelm/contents/PKGBUILD) returned **200**, blob SHA `195a1afb7e3b372f9ec5dadd8af061ff649622a5`. Its content defines `pkgname=taskhelm`, `pkgver=0.4.1`, description “A graphical shell that sits on top of TaskWarrior,” and Python 2 dependencies. The [file](https://github.com/aur-archive/taskhelm/blob/master/PKGBUILD) is historical package evidence; it does not contradict the current AUR RPC's zero results.

#### Development/coexistence suffix checks

All four literal suffixes were checked in npm, PyPI, and GitHub repository-name search. Each GitHub result below had `total_count:0`, `incomplete_results:false`, `items:[]`. Each npm response was `404 {"error":"Not found"}`; each PyPI response was `404 {"message":"Not Found"}`.

| Literal candidate | npm exact endpoint | PyPI exact endpoint | GitHub query |
| --- | --- | --- | --- |
| `runhelm-next` | [404](https://registry.npmjs.org/runhelm-next) | [404](https://pypi.org/pypi/runhelm-next/json) | [`runhelm-next in:name`, 200 / 0](https://api.github.com/search/repositories?q=runhelm-next+in:name&per_page=100) |
| `runhelm-dev` | [404](https://registry.npmjs.org/runhelm-dev) | [404](https://pypi.org/pypi/runhelm-dev/json) | [`runhelm-dev in:name`, 200 / 0](https://api.github.com/search/repositories?q=runhelm-dev+in:name&per_page=100) |
| `taskhelm-next` | [404](https://registry.npmjs.org/taskhelm-next) | [404](https://pypi.org/pypi/taskhelm-next/json) | [`taskhelm-next in:name`, 200 / 0](https://api.github.com/search/repositories?q=taskhelm-next+in:name&per_page=100) |
| `taskhelm-dev` | [404](https://registry.npmjs.org/taskhelm-dev) | [404](https://pypi.org/pypi/taskhelm-dev/json) | [`taskhelm-dev in:name`, 200 / 0](https://api.github.com/search/repositories?q=taskhelm-dev+in:name&per_page=100) |

Additional literal web queries (search tool gives no HTTP statuses or reliable total-result count):

- `"runhelm" "daemon" OR "runhelm-next" OR "runhelm-dev"` returned a gaming bestiary containing “Armorer Runhelm” and an unrelated daemon term, not an exact program/suffix match. No program collision inferred from this noisy result.
- `"taskhelm" "daemon" OR "taskhelm-next" OR "taskhelm-dev"` returned the existing [TaskHelm repository](https://github.com/Kaka-123-D/TaskHelm), npm packages, and CLI documentation, but no verified exact hyphenated suffix or `taskhelm daemon` command in the displayed excerpts.
- `"runhelm daemon" OR "taskhelm daemon"` returned related Helm and TaskHelm results; none of the displayed primary-source excerpts established either exact two-word command. In particular [`helmai-dev/cli`](https://github.com/helmai-dev/cli) documents **`helm daemon start`**, not `runhelm daemon` or `taskhelm daemon`.

### Inferences

- An executable named `taskhelm` would compete on PATH with a real npm-distributed command. A `daemon` subcommand cannot solve a collision in the first word. This follows directly from the published [npm `bin` field](https://registry.npmjs.org/taskhelm).
- Lack of an npm/PyPI Runhelm package is much narrower evidence than a clean product identity: the exact GitHub org/repo already exists. The suffix probes describe those particular registries only; they do not reserve or clear the namespace.
- Historical Taskhelm use deserves reporting even though the active AUR search is empty. Treating `aur-archive/taskhelm` as an active AUR package would be incorrect.

### Gaps

- crates.io, Homebrew core and casks, Debian, Fedora, and Nix did not yield usable results. Exact and substring counts there remain unknown. Nix stable channels were not separately probed.
- No system-wide executable inventory was performed; public research cannot enumerate private shell aliases, functions, unpublished programs, or arbitrary package contents.
- The suffix names were not exhaustively checked against every distribution. GitHub search can omit private, unindexed, or otherwise unavailable repositories; a missing public login does not prove GitHub will allow registration.

## 2. What domain, product-brand, and trademark evidence changes the collision picture?

### Takeaway

TaskHelm is also an exact AI-coding-related SaaS brand, independent of the older Python package and apparently distinct from the npm workbench. Runhelm is used in domains for existing Helm products, including an AI executive-team service. **Domain registration and USPTO/EUIPO clearance remain unresolved**, because the authoritative lookup endpoints timed out.

### Cited Findings

#### Product and similar-brand evidence

The literal web searches `"Runhelm"` and `"Taskhelm"` returned the following primary-source pages. These are **search-tool excerpts of primary websites**, not successful direct HTTP fetches. The tool did not expose origin response status or a dependable overall result count.

- [`runhelm.io`](https://runhelm.io/) titles itself **“Helm — An AI executive team with you at the helm.”** Its displayed copy describes five specialists running operations, debating decisions, and shipping work, with “Agents work. You approve. Nothing leaves without you.” This is domain-string use of **runhelm** and adjacent AI orchestration positioning; the displayed product brand is **Helm**, not necessarily a registered word mark RUNHELM. A direct WebFetch returned `Request timed out`.
- [`runhelm.app`](https://runhelm.app/) titles itself **“Helm | Operations software for arcades, LAN centers, esports venues.”** It advertises a live floor board, player screens, crew tools, cloud catalog and station agents. This is another software use of the **runhelm** domain label, not an exact branded word-mark finding. Its [changelog](https://runhelm.app/changelog) was surfaced with “Updated October 11, 2026.”
- [`taskhelm.com`](https://taskhelm.com/) titles itself **“TaskHelm — MCP Project Task Manager for AI.”** It explicitly supports Claude Code, Cursor, Codex, Windsurf and other MCP clients, preserving tasks, rules, and memory for one developer across projects. This is **exact product-name usage in the proposed audience**, not a substring hit. A direct WebFetch returned `Request timed out`.
- The [TaskHelm privacy page](https://taskhelm.com/privacy), surfaced by the initial exact-name search, states “Last updated: July 10, 2026” and “TaskHelm is operated by Serna Media Inc.” No evidence collected establishes common ownership with `Kaka-123-D/TaskHelm`; do not conflate them.
- **Similar AI brand, not exact candidate:** [`neumie/helm`](https://github.com/neumie/helm) surfaced in the daemon query and describes “a local orchestration cockpit for software work,” with a Node daemon, desktop terminals, a `helm` CLI, human checkpoints, and Claude Code/Codex execution in worktrees. This is exceptionally close category overlap with the shared **Helm** stem, but it is not a Runhelm/Taskhelm exact-name match.
- **Similar AI brand, not exact candidate:** [`helmai-dev/cli`](https://github.com/helmai-dev/cli) documents a Helm background agent-runner daemon with `helm daemon start/stop/install/status`, Claude Agent SDK/Codex SDK execution, and local `~/.helm/environments/...` state. This likewise is **Helm**, not either exact candidate.
- **Established developer-tool stem:** the primary [Helm website](https://helm.sh/) identifies Helm as “The package manager for Kubernetes.” Literal query: `site:helm.sh "package manager"`. This is a shared-stem/search-confusion consideration, not an exact `runhelm` or `taskhelm` package finding.

#### Domain registration checks

| Check | Literal query / URL | Result |
| --- | --- | --- |
| Discover authoritative RDAP routing | [IANA bootstrap](https://data.iana.org/rdap/dns.json) | Direct fetch timeout; WebFetch retry `Request timed out`; no HTTP status |
| Discover .dev endpoint via primary source | Search `".dev" "rdap" "google" domain`; [IANA .dev record](https://www.iana.org/domains/root/db/dev.html) | Search excerpt lists **`https://pubapi.registry.google/rdap/`** as RDAP base; origin HTTP status not exposed |
| `runhelm.dev` exact RDAP | [GET](https://pubapi.registry.google/rdap/domain/runhelm.dev) | `Error: The operation timed out.`; **registration unknown**, no status/count |
| `taskhelm.dev` exact RDAP | [GET](https://pubapi.registry.google/rdap/domain/taskhelm.dev) | `Error: The operation timed out.`; **registration unknown**, no status/count |
| Supplemental web visibility | `"runhelm.dev" OR "runhelm.sh" OR "taskhelm.dev" OR "taskhelm.sh"` | Results included TaskHelm software and Pelorus script references; no authoritative registration record for any of these four labels |

Important false-positive distinction: the [Pelorus installation documentation](https://pelorus.readthedocs.io/en/v1.3.0/Install.html) says “pass this to runhelm.sh”; a [Pelorus issue](https://github.com/konveyor/pelorus/issues/127) shows `./runhelm.sh -n pelorus-1`. Here **`runhelm.sh` is a shell-script filename**, not evidence that the domain `runhelm.sh` is registered. That reinforces the “run Helm” developer association but must not be entered as domain ownership.

Only `.dev` received an authoritative registry lookup attempt, satisfying the requested `.dev and/or .sh` scope at the attempt level. No `.sh` WHOIS/RDAP conclusion is available. No DNS absence or absent web page was used to infer registrability.

#### USPTO / EUIPO software classes 9 and 42

Intended public-register searches: exact word marks **RUNHELM** and **TASKHELM**, including live and dead/pending records, with attention to classes **9** (software goods) and **42** (software/technology services). These are the intended filters, **not a claim that a register accepted or executed them**.

| Register / check | URL or literal query | Observation |
| --- | --- | --- |
| USPTO public trademark search entry point | [https://tmsearch.uspto.gov/](https://tmsearch.uspto.gov/) | Direct GET timeout; no HTTP status, no usable search UI, **no register result count** for either candidate or class |
| EUIPO public eSearch entry point | [https://euipo.europa.eu/eSearch/](https://euipo.europa.eu/eSearch/) | Direct GET timeout; no HTTP status, no usable search UI, **no register result count** for either candidate or class |
| Supplemental indexed Runhelm search | `"runhelm" (site:uspto.gov OR site:euipo.europa.eu)` | Search tool: **“No search results found. Please try a different query.”** This is zero displayed web-search results, **not** zero trademark records |
| Supplemental indexed Taskhelm search | `"taskhelm" (site:uspto.gov OR site:euipo.europa.eu)` | Same no-results message; **not** a trademark-register negative |

Primary guidance discovered through search:

- Literal query `site:uspto.gov trademark search classes 9 42` surfaced the USPTO [goods and services guidance](https://www.uspto.gov/trademarks/basics/goods-and-services) and [classification guidance](https://www.uspto.gov/trademarks/laws/how-satisfy-requirements-multiple-class-application-or-multiple-class), including class 9 computers/software. This guidance is classification context, not candidate clearance.
- Literal query `site:euipo.europa.eu eSearch trademarks search` surfaced EUIPO's [trade mark search guidance](https://www.euipo.europa.eu/en/trade-marks/before-applying/availability), stating TMview can search registered marks free of charge, plus the [eSearch FAQ](https://www.euipo.europa.eu/en/help-centre/searches/faq-esearch-plus). Discovery of those pages did not execute a candidate search or class filter.

### Inferences

- TaskHelm has strong practical confusion evidence even without a trademark record: an exact npm executable in local development orchestration and an exact MCP product for AI coding agents. Renaming a development build `taskhelm-next` would not remove the parent-brand overlap.
- Runhelm's exact org/repo occupation plus `runhelm.io` AI-team positioning make it more than a hypothetical shared-stem problem. However, this research does not prove who owns the domains, whether the GitHub organization is related, or any enforceable trademark right.
- The Helm stem is already crowded in both developer infrastructure and coding-agent orchestration. This is a discoverability/category-positioning inference, not a legal conclusion.

### Gaps

- **No completed USPTO/EUIPO register search, no class-9/class-42 record count, no mark status or owner verified for either candidate.** Search-engine non-results cannot replace the unavailable register checks. No registration or legal availability claim is supported.
- `.dev` registration status remains unknown; `.sh` received supplemental visibility research only. Even a future RDAP 404 would need separate registrar/reservation/pricing confirmation before purchase availability could be asserted.
- Direct HTTP retrieval of `taskhelm.com` and `runhelm.io` failed. Their product claims above are attributed to search-returned first-party text, not independently tested service functionality or validated customer counts.

## 3. Do the names fit the CLI, XDG, and English/French constraints?

### Takeaway

Both supplied base strings satisfy the requested **at most eight ASCII letters** and are syntactically uncomplicated command/path components. Their lexical fit is reasonable, but the technical simplicity does not remove the concrete namespace collisions documented above.

### Cited Findings

- Cambridge defines English **helm** as the handle or wheel controlling a ship/boat's direction and uses **“at the helm”** for controlling an organization. Literal query `site:dictionary.cambridge.org helm English French`; sources: [English definition](https://dictionary.cambridge.org/us/dictionary/english/helm), [English–French translation](https://dictionary.cambridge.org/us/dictionary/english-french/helm), which gives **barre**. These were dictionary-publisher search excerpts; origin HTTP statuses were not exposed.
- English **task** maps to French **tâche**, supported by the dictionary example in [Cambridge English–French](https://dictionary.cambridge.org/dictionary/english-french/task). Literal query: `site:dictionary.cambridge.org/dictionary/english-french/task "tâche"`.
- Literal query `site:dictionary.cambridge.org/dictionary/english-french/run "exécuter"` returned no results. The broader query `site:dictionary.cambridge.org "run" "task" French` returned related pages rather than a direct translation entry. Consequently a sourced lexical inventory of **run** is not claimed here.
- The [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/) surfaced for `site:specifications.freedesktop.org "XDG_STATE_HOME" "XDG_RUNTIME_DIR"`; its excerpt identifies both variables as base directories. Direct retrieval of the older [`basedir-spec/latest/`](https://specifications.freedesktop.org/basedir-spec/latest/) URL timed out; WebFetch of the surfaced `/basedir/` URL produced no usable output. Full spec text and HTTP status were therefore not obtained in this session.
- Existing TaskHelm documents a private `~/.taskhelm` namespace and `TASKHELM_*` environment variables in its [README](https://github.com/Kaka-123-D/TaskHelm/blob/main/README.md). This is existing application-state naming evidence, not proof that it currently uses XDG paths.

### Inferences

The following is a direct string/ergonomics assessment of the supplied names and proposed forms, not an executed command or claim of registry availability:

| Property | Runhelm | Taskhelm |
| --- | --- | --- |
| Base string | `runhelm`: **7 ASCII letters** | `taskhelm`: **8 ASCII letters** |
| Short binary | `runhelm` | `taskhelm` — already published by npm |
| Daemon form | `runhelm daemon`: straightforward binary + argument | `taskhelm daemon`: straightforward syntax, but reuses the collided binary |
| Next build | `runhelm-next`: 12 characters | `taskhelm-next`: 13 characters |
| Dev build | `runhelm-dev`: 11 characters | `taskhelm-dev`: 12 characters |
| Example config component | `$XDG_CONFIG_HOME/runhelm/` | `$XDG_CONFIG_HOME/taskhelm/` |
| Example state component | `$XDG_STATE_HOME/runhelm/` | `$XDG_STATE_HOME/taskhelm/` |
| Example runtime socket | `$XDG_RUNTIME_DIR/runhelm/daemon.sock` | `$XDG_RUNTIME_DIR/taskhelm/daemon.sock` |

- Both bases contain only lowercase ASCII letters, no spaces, accents, leading dash, slash, or shell punctuation. Both are easy to embed in the suggested path components. The suffixed binaries intentionally exceed eight letters/characters; if the eight-letter constraint applies to every development executable, neither suffix form meets it.
- XDG naming has no central reservation registry. Suitable path spelling is not proof of collision-free use. Separate `-next`/`-dev` directory components could distinguish builds syntactically, but isolation would require implementation and is not verified here. The full runtime path, not just the short project name, determines Unix-socket length suitability.
- English **Runhelm** evokes running/executing plus steering. To developers it can also parse as **“run Helm”**, strengthened by the real Pelorus script and established Kubernetes tool. **Taskhelm** more directly evokes steering/managing tasks, which helps explain its existing task-manager and AI-workbench uses.
- For French readers these are English-style compounds, not transparent native French phrases. `task` → `tâche` and `helm` → `barre` support an explanation such as “piloter les tâches,” but the names themselves do not communicate that without English familiarity. Runhelm may feel slightly shorter to say; Taskhelm's `skh` boundary may require more articulation. These are linguistic judgments, not pronunciation-user-test results.
- I identified no obvious English/French obscene or derogatory reading from the two strings. That is a limited reviewer observation, **not** an exhaustive dictionary, slang, geographic, or native-speaker clearance.

### Gaps

- No native-speaker testing, phonetic survey, or exhaustive French slang/name search was conducted. Exact compounds are not established here as ordinary dictionary words.
- XDG examples are proposed namespace shapes only. No paths, sockets, services, aliases, or binaries were created or executed, and no machine-wide collision test was performed.
- Research completion means that findings and unavailable checks are documented. It does **not** mean either name is available, trademark-cleared, or selected. The strongest concrete handoff evidence is Taskhelm's exact npm command/category collision and Runhelm's exact GitHub namespace occupation plus nearby AI branding.
