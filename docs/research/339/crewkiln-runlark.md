# Crewkiln / Runlark naming evidence for #339

Research date: **2026-11-02**. Scope: only these two proposed names for a personal, terminal-first Rust AI coding-agent orchestrator. Evidence notes, not a naming decision or availability/legal-clearance finding. Linked requests below are literal public GETs; web-search strings are quoted verbatim. An HTTP 404, a successful empty search, an incomplete source tree, and a timeout are different outcomes.

## 1. What software, package, and GitHub collisions were found?

### Takeaway

**No exact collision was established for either name in the successfully checked sources.** This round strengthens coverage with the official crates.io index and complete Homebrew core/cask source trees; Nix coverage is partial, while Debian and Fedora remain unresolved. These are bounded negative observations, not proof of unused names. [Crates index](https://github.com/rust-lang/crates.io-index); [Homebrew core tree](https://api.github.com/repos/Homebrew/homebrew-core/git/trees/HEAD?recursive=1); [Homebrew cask tree](https://api.github.com/repos/Homebrew/homebrew-cask/git/trees/HEAD?recursive=1).

### Cited Findings

#### Package matrix

| Source / exact scope | Crewkiln | Runlark |
| --- | --- | --- |
| crates.io official Git index, exact entry | [GET `contents/cr/ew/crewkiln`](https://api.github.com/repos/rust-lang/crates.io-index/contents/cr/ew/crewkiln): **404**, `Not Found`. | [GET `contents/ru/nl/runlark`](https://api.github.com/repos/rust-lang/crates.io-index/contents/ru/nl/runlark): **404**, `Not Found`. |
| Homebrew core source tree | **200**, complete tree; no case-insensitive `crewkiln` substring in any returned path. [Request](https://api.github.com/repos/Homebrew/homebrew-core/git/trees/HEAD?recursive=1). | Same complete tree; no `runlark` path. [Request](https://api.github.com/repos/Homebrew/homebrew-core/git/trees/HEAD?recursive=1). |
| Homebrew cask source tree | **200**, complete tree; no `crewkiln` path. [Request](https://api.github.com/repos/Homebrew/homebrew-cask/git/trees/HEAD?recursive=1). | Same complete tree; no `runlark` path. [Request](https://api.github.com/repos/Homebrew/homebrew-cask/git/trees/HEAD?recursive=1). |
| Arch official, exact `name` filter | [GET `?name=crewkiln`](https://archlinux.org/packages/search/json/?name=crewkiln): **200**, `valid:true`, `count:0`, `results:[]`. | [GET `?name=runlark`](https://archlinux.org/packages/search/json/?name=runlark): **200**, `valid:true`, `count:0`, `results:[]`. |
| Arch AUR, exact RPC info | [GET `info?arg[]=crewkiln`](https://aur.archlinux.org/rpc/v5/info?arg[]=crewkiln): **200**, `resultcount:0`, `results:[]`, `type:"multiinfo"`. | [GET `info?arg[]=runlark`](https://aur.archlinux.org/rpc/v5/info?arg[]=runlark): **200**, same zero-result response. |
| Debian package-name search, all suites/sections | [Search `crewkiln`](https://packages.debian.org/search?keywords=crewkiln&searchon=names&suite=all&section=all): **timeout**, no status/count. | [Search `runlark`](https://packages.debian.org/search?keywords=runlark&searchon=names&suite=all&section=all): **timeout**, no status/count. |
| Ubuntu package-name **substring** search, all suites/sections/architectures | [Search `crewkiln`](https://packages.ubuntu.com/search?keywords=crewkiln&searchon=names&suite=all&section=all): **200**, “You have searched for packages that names contain crewkiln”; “Sorry, your search gave no results.” | [Search `runlark`](https://packages.ubuntu.com/search?keywords=runlark&searchon=names&suite=all&section=all): **200**, equivalent wording, **no displayed results**. |
| Fedora package search | [GET `/search?query=crewkiln`](https://packages.fedoraproject.org/search?query=crewkiln): **timeout**, query behavior/count unverified. | [GET `/search?query=runlark`](https://packages.fedoraproject.org/search?query=runlark): **timeout**, query behavior/count unverified. |
| Nixpkgs source, by-name directory | [GET `pkgs/by-name/cr`](https://api.github.com/repos/NixOS/nixpkgs/contents/pkgs/by-name/cr?ref=548dcefb91f9bc3632c9befc4304f4b080f2ec08): **200**, 108 entries, no `crewkiln` name. | [GET `pkgs/by-name/ru`](https://api.github.com/repos/NixOS/nixpkgs/contents/pkgs/by-name/ru?ref=548dcefb91f9bc3632c9befc4304f4b080f2ec08): **200**, 108 entries, no `runlark` name. |
| npm exact unscoped package | [GET `/crewkiln`](https://registry.npmjs.org/crewkiln): **404**, `{"error":"Not found"}`. | [GET `/runlark`](https://registry.npmjs.org/runlark): **404**, same response. |
| PyPI exact project | [GET `/pypi/crewkiln/json`](https://pypi.org/pypi/crewkiln/json): **404**, `{"message":"Not Found"}`. | [GET `/pypi/runlark/json`](https://pypi.org/pypi/runlark/json): **404**, same response. |

Source-list checks and their limits:

- **crates.io:** the index's [config.json API](https://api.github.com/repos/rust-lang/crates.io-index/contents/config.json) returned **200**, identifying `https://crates.io` as its API and `https://static.crates.io/crates` as its download base. The [parent `cr/ew` directory](https://api.github.com/repos/rust-lang/crates.io-index/contents/cr/ew) returned **200**, **9 entries**, no `crewkiln`; the [parent `ru/nl` directory](https://api.github.com/repos/rust-lang/crates.io-index/contents/ru/nl) returned **200**, **22 entries**, no `runlark`. This corroborates the exact-file 404s with working parent listings. No direct crates.io API request was made in this round; index absence does not establish publication eligibility or reserved-name status.
- **Homebrew core:** recursive tree returned **9,448 entries**, `truncated:false`, SHA **`ffc79088768f59d49d304f3ff00de576b4ea8f69`**. **Casks:** **7,927 entries**, `truncated:false`, SHA **`0b126890223d58bd56c9c842fea03f0ddf424502`**. The case-insensitive path filter `/crewkiln|runlark/i` returned `[]` for both. These are complete source-path checks, including any matching alias paths, not full formula-body or installed-binary scans. [Core](https://api.github.com/repos/Homebrew/homebrew-core/git/trees/HEAD?recursive=1); [casks](https://api.github.com/repos/Homebrew/homebrew-cask/git/trees/HEAD?recursive=1).
- **Nix:** the [recursive HEAD tree](https://api.github.com/repos/NixOS/nixpkgs/git/trees/HEAD?recursive=1) returned **200**, SHA **`548dcefb91f9bc3632c9befc4304f4b080f2ec08`**, **73,642 entries**, **`truncated:true`**; no matching path among returned entries. **That is not a complete-tree negative.** The two complete by-name directory checks above were therefore made at that exact revision. The [raw `pkgs/top-level/all-packages.nix`](https://raw.githubusercontent.com/NixOS/nixpkgs/548dcefb91f9bc3632c9befc4304f4b080f2ec08/pkgs/top-level/all-packages.nix) also returned **200**, 311,140 text characters, no case-insensitive candidate substring. Its [contents API](https://api.github.com/repos/NixOS/nixpkgs/contents/pkgs/top-level/all-packages.nix?ref=548dcefb91f9bc3632c9befc4304f4b080f2ec08) returned **200**, blob SHA `8f78f25b3775c1aa1c99094d8764d9988a3bb163`; the raw text, rather than the API's base64 payload, was searched. Generated attributes, other package sets, aliases and release channels are not exhaustively covered.

#### GitHub exact repository and shared account namespace

| Literal request | Observed outcome |
| --- | --- |
| [`crewkiln in:name`, `per_page=100`](https://api.github.com/search/repositories?q=crewkiln+in:name&per_page=100) | **200**, `total_count:0`, `incomplete_results:false`, `items:[]`. No exact or longer matching repository name returned. |
| [`runlark in:name`, `per_page=100`](https://api.github.com/search/repositories?q=runlark+in:name&per_page=100) | **200**, identical zero-result structure. |
| [`/users/crewkiln`](https://api.github.com/users/crewkiln), [`/orgs/crewkiln`](https://api.github.com/orgs/crewkiln) | Both **404**, `Not Found`. |
| [`/users/runlark`](https://api.github.com/users/runlark), [`/orgs/runlark`](https://api.github.com/orgs/runlark) | Both **404**, `Not Found`. |

The user and organization lookups both matter for the intended account name. These responses establish only that no public account record was returned; they do not establish that GitHub will allow registration or an organization rename.

#### Broad web-search observations

- Literal **`"crewkiln"`** returned relaxed/nearby results, notably **“Arc Crew Kiln”**, an apparel product with spaces. Its first-party snippet describes a pullover sweatshirt; it is **not an established exact concatenated software-name collision**. This is **search-snippet-only** evidence. [Parlez product](https://parlez.co.uk/products/arc_crew_kiln_211025).
- Literal **`"runlark"`** returned unrelated/relaxed running results, including [Roark's running collection](https://roark.com/collections/run-amok) and [Atlanta Run Club](https://www.atlantarunclub.com). No exact Runlark use was verified from these results; this is not an authoritative zero-result search.
- Discovery query **`Fedora package sources pagure API projects rpms search`** returned source-host documentation/discussion and [pagure-cli documentation](https://hackage.haskell.org/package/pagure-cli). It did not produce candidate-specific primary evidence or a verified current alternative source-list endpoint. No guessed Fedora API was used.

### Inferences

- Both candidates have materially better **observed exact-name evidence** than the already-colliding earlier candidates, but that is a statement about this checked sample only. The strongest negatives here are successful complete Homebrew path checks, explicit zero-result registry/search responses, and working crates.io index parent listings—not web-search silence.
- The apparel result can create some search noise for Crewkiln even without an exact software collision. [First-party product snippet](https://parlez.co.uk/products/arc_crew_kiln_211025).

### Gaps

- Debian and Fedora results are unknown. Nix is source-level and partial; no evaluated stable/unstable package-search result was obtained. Homebrew third-party taps, npm scoped packages, private/unpublished repositories and binaries shipped under differently named packages are outside these checks.
- Direct crates.io, formulae.brew.sh and search.nixos.org requests were deliberately not repeated after earlier rounds' reported timeouts; official public source repositories supplied the evidence above. No explicit network-policy denial was returned in this round, and no blocked-host workaround was used.

## 2. What was established about .dev registration and USPTO/EUIPO classes 9/42?

### Takeaway

**Both .dev registration statuses and all requested trademark result counts remain unknown.** Requests timed out before candidate registration or name/class trademark results could be obtained. Search-engine absence is not trademark clearance.

### Cited Findings

| Literal lookup / query | Actual outcome |
| --- | --- |
| [`https://pubapi.registry.google/rdap/domain/crewkiln.dev`](https://pubapi.registry.google/rdap/domain/crewkiln.dev) | `Error: The operation timed out.` No HTTP status or domain record. |
| [`https://pubapi.registry.google/rdap/domain/runlark.dev`](https://pubapi.registry.google/rdap/domain/runlark.dev) | Same timeout; no HTTP status or domain record. |
| [`https://tmsearch.uspto.gov/`](https://tmsearch.uspto.gov/) | Same timeout at the USPTO search entry point. No candidate/class search submitted. |
| [`https://www.euipo.europa.eu/en/search-ip`](https://www.euipo.europa.eu/en/search-ip) | Same timeout at the EUIPO search entry point. No candidate/class search submitted. |
| Web query **`site:uspto.gov "crewkiln" OR "runlark"`** | Tool: “No search results found.” Zero returned **web** results, not zero USPTO records. [Target authority](https://tmsearch.uspto.gov/). |
| Web query **`site:euipo.europa.eu "crewkiln" OR "runlark"`** | Tool: “No search results found.” Zero returned **web** results, not zero EUIPO records. [Target authority](https://www.euipo.europa.eu/en/search-ip). |

The intended unexecuted database checks were word marks **CREWKILN** and **RUNLARK**, separately in Nice classes **9** and **42**, including live/dead status, owners and goods/services. Those are missing-check descriptions, not purported executed query syntax. The Google RDAP endpoint pattern was inherited from the preceding research notes' endpoint discovery; neither timeout is a registry “not found” response.

### Inferences

- No distinction between the two candidates can be drawn from these registration/trademark checks: both are unresolved.
- A 404 from a package registry or GitHub account lookup has no bearing on trademark rights or domain purchase eligibility.

### Gaps

- No domain record, registration dates, registrar result, trademark application/registration number, owner, status or class-specific goods/services record was retrieved.
- Exact and confusingly similar marks remain unsearched in the authoritative databases. No legal or registration availability claim is supported. Responses were timeouts, **not verified JavaScript-only pages, explicit policy denials, HTTP 404s, or successful no-result database searches**.

## 3. What do the names suggest in English/French, and do they conflict with common shell commands?

### Takeaway

**`crewkiln` has 8 ASCII letters; `runlark` has 7.** Neither string occurs in the checked Bash manual or GNU Coreutils command documentation. Crewkiln suggests team craftsmanship/heat; Runlark suggests execution plus a bird or playful activity. These are interpretations of component words, not established meanings of the coined compounds.

### Cited Findings

#### Dictionary evidence

Direct dictionary-page fetches below timed out (`Request timed out`, 10-second fetch limit). The lexical evidence is therefore explicitly **publisher search excerpts**, not fully fetched entries.

| Component | Observed dictionary evidence | Literal discovery query / source |
| --- | --- | --- |
| **crew** | Collins: ship/plane **équipage**; film crew **équipe / équipe de tournage**; gang **bande**. | **`crew run French dictionary translation`** → [Collins crew](https://www.collinsdictionary.com/dictionary/english-french/crew). Direct GET of that URL timed out. |
| **kiln** | Collins describes an oven used to bake pottery/bricks to harden them; indexed translation material explicitly lists French **four**. | **`site:collinsdictionary.com "kiln" "four"`** → [English definition](https://www.collinsdictionary.com/us/dictionary/english/kiln), [French entry](https://www.collinsdictionary.com/us/dictionary/english-french/kiln), [publisher translation list explicitly containing “French: four”](https://www.collinsdictionary.com/us/dictionary/english-chinese_traditional/kiln). Direct GET of [non-US French entry](https://www.collinsdictionary.com/dictionary/english-french/kiln) timed out. |
| **run** | Cambridge excerpt: move quickly **courir**; move smoothly **marcher, rouler**. Reverso excerpt also gives **fonctionner**. | **`crew run French dictionary translation`** → [Cambridge run](https://dictionary.cambridge.org/us/dictionary/english-french/run), [Reverso run](https://dictionary.reverso.net/english-french/run). Cambridge direct GET timed out; Reverso not directly fetched. |
| **lark** | Cambridge explicitly gives **alouette, blague**; its English search result distinguishes **BIRD** and **ACTIVITY** senses. | **`kiln lark French dictionary translation`** → [Cambridge English–French](https://dictionary.cambridge.org/us/dictionary/english-french/lark); **`site:dictionary.cambridge.org kiln lark English French`** → [English lark](https://dictionary.cambridge.org/dictionary/english/lark). Direct English–French GET timed out. |

The broad query **`kiln lark French dictionary translation`** also discovered the Collins French kiln page but did not itself expose its French translation; the later Collins-specific query supplied that evidence. Four initial site-only queries—**`site:dictionary.cambridge.org/dictionary/english-french/kiln`**, **`site:dictionary.cambridge.org/dictionary/english-french/lark`**, **`site:dictionary.cambridge.org/dictionary/english-french/crew`**, **`site:dictionary.cambridge.org/dictionary/english-french/run`**—each failed with **`Web search request failed (HTTP 400)`**, not “no results.”

#### Shell/common-tool evidence

- [GNU Bash 5.3 complete manual](https://www.gnu.org/software/bash/manual/bash.html): **HTTP 200**, **1,040,953 text characters**; case-insensitive full-response search `/crewkiln|runlark/i` returned no match. The separately fetched [Shell Builtin Commands overview](https://www.gnu.org/software/bash/manual/html_node/Shell-Builtin-Commands.html) was also **200**; the complete manual was then checked because the overview links out to multiple builtin sections.
- [GNU Coreutils 9.12 manual table of contents](https://www.gnu.org/software/coreutils/manual/html_node/index.html): **HTTP 200**, **46,536 text characters**, no candidate substring. This checks documented standard GNU utility names, not every command shipped by Linux or macOS.
- [fish current command documentation](https://fishshell.com/docs/current/cmds/): `Error: The operation timed out.` Fish documentation coverage is **unresolved**, not an empty command list.
- Lengths and spelling are direct inspection of the supplied strings: lowercase ASCII, no spaces or shell metacharacters. This supports straightforward command tokens such as `crewkiln` and `runlark`; it does not establish free executable names on any particular machine.

### Inferences

- **Crewkiln:** “crew” fits a group of coding agents; “kiln” supplies a making/refining metaphor. The imagery can also suggest putting the crew itself into a hot oven, an awkward literal reading worth noting. French readers need familiarity with English **crew** and especially **kiln**; the final `ln` cluster may make first-time pronunciation less obvious. These are researcher impressions, not audience testing. [Crew senses](https://www.collinsdictionary.com/dictionary/english-french/crew); [kiln definition](https://www.collinsdictionary.com/us/dictionary/english/kiln).
- **Runlark:** “run” connects to running work, but also sport; “lark” supplies a bird/lighthearted association. The **blague** sense can make it sound playful rather than operationally serious. French readers are unlikely to recover **alouette** from the name without English vocabulary. These are linguistic impressions, not documented reactions. [Run](https://dictionary.cambridge.org/us/dictionary/english-french/run); [lark](https://dictionary.cambridge.org/us/dictionary/english-french/lark).
- Neither compound presents an obvious French insult to this researcher, but no exhaustive slang/offensiveness or native-speaker test was performed. Neither is being claimed as a French dictionary word.

### Gaps

- Bash/Coreutils checks do not cover zsh, every POSIX utility, shell aliases/functions, third-party commands or the user's installed PATH. No host/PATH inspection or candidate binary execution was performed.
- No pronunciation survey, exhaustive phonetic-similarity review or full dictionary entry for either coined compound was obtained. No shortened aliases were proposed or checked.
- Research stopped at this bounded evidence set. No naming choice, implementation, account, installation, purchase, Git write, host approval command or additional agent was involved; the only project file written is this note.
