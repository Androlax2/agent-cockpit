# Torlek: evidence for Theo's choice

Research date: **2026-11-02**. Researcher: Claude Code (Opus 5.5) in a second-opinion pass and
follow-up rounds with Theo, after the eight-name study in
[the main report](../2026-11-02-339-product-naming.md). Theo chose Torlek; this appendix records
what was checked for it, what was rejected on the way, and what remains unverified.
Negative results are bounded observations, not legal clearance or proof that a name is free.

## How Torlek was found

Theo was not keen on the provisional finalists (Crewkiln, Runlark). Crewkiln reads as manufactured:
English often drops the `n` of "kiln", French speakers stumble on the final `-ln`, and a crew in an
oven is an odd image. Runlark is pleasant to say, but "a lark" suggests a prank, which clashes with
approvals and oversight.

Six further rounds followed, each screened on crates.io, npm, PyPI, GitHub repositories and
accounts, and `.dev`/`.com` RDAP; later rounds added a web search for businesses and Theo's browser
checks of TMview and the WIPO Global Brand Database.

- **Real words that fit were already used by AI-agent tools**: Tutti (`nutthouse/tutti`,
  multi-agent orchestration CLI), Weft (`WeaveMindAI/weft`), Orrery (`arvelvale/orrery`, "a
  local-first cockpit for your AI coding agents"), Volery, Paddock (crates.io `paddock`, keywords
  `ai-agents`, `devcontainer`), Musher (`musher-dev/mush`), Nocturne, Ravel, Navette.
- **Vowel-dropped names in the style of herdr were more crowded still**: Flockr, Drovr, Sheprd,
  Roostr, Ostlr, Wardn, Wranglr. Theo agreed the style also reads as a herdr add-on.
- **Invented two-syllable names that sound the same in English and French** were the productive
  direction. Theo liked several that later failed:
  - **Torvel**: `torvel.dev` was registered on 2026-08-08 and shows a "launching soon" page, and the
    GitHub organization `torvel-app` was created on 2026-08-01.
  - **Torvan**: [Torvan Medical](https://torvanmedical.com/) sells medical devices with built-in
    software.
  - **Sarvel**: a French trademark application, SARVEL no. 5289528, filed on 2026-08-19 in Nice
    classes 9, 35, 37, 38, 42 and 45 and still pending, appeared in TMview and WIPO. Japan
    registration 6380941 covers class 9.
- **Rejected for an existing software business**: [Corvan AI](https://corvan.io/),
  [Dorvan](https://dorvan.it/), [Tolvan Data](https://www.tolvan.com/) and
  [korlan.eu](https://korlan.eu/).
- **Velnar and Torlen** returned marks in both TMview and WIPO; their classes were not recorded.
- **Dorlek** passed every check, including TMview and WIPO, and is the recorded backup. Its nearest
  lookalike is [DORLECO](https://www.crunchbase.com/organization/dorleco), which makes automotive
  control software.

## Checked table for Torlek

Legend as in the main report: **A** = successful absence in the stated scope; **404** = exact
resource not returned; **H** = hit; **P** = partial evidence; **N** = not checked.

| Check (issue #339 scope) | Result | Source and scope |
| --- | --- | --- |
| crates.io | 404 | [API](https://crates.io/api/v1/crates/torlek), re-read 2026-11-02 |
| Homebrew core / casks | 404 / 404 | [formula](https://formulae.brew.sh/api/formula/torlek.json), [cask](https://formulae.brew.sh/api/cask/torlek.json) JSON API |
| Arch official | A exact, 0 results | [packages JSON](https://archlinux.org/packages/search/json/?name=torlek); `name=tor` returned 1 as a control |
| AUR | A exact name search, 0 results | [RPC](https://aur.archlinux.org/rpc/v5/search/torlek?by=name) |
| Debian | A substring, "no results" | [packages.debian.org](https://packages.debian.org/search?keywords=torlek&searchon=names&suite=all&section=all); [sources API](https://sources.debian.org/api/search/torlek/) returned `exact:null, other:[]` |
| Ubuntu | A substring, "no results" | [packages.ubuntu.com](https://packages.ubuntu.com/search?keywords=torlek&searchon=names&suite=all&section=all), all suites |
| Fedora | A default query, zero in every release | [packages.fedoraproject.org](https://packages.fedoraproject.org/search?query=torlek); the page offered an empty "Did you mean" with no suggestion text |
| Nix | P | `pkgs/by-name/to/torlek` returned 404 (`pkgs/by-name/to/tor` returned 200 as a control), and GitHub code search for `torlek` in `NixOS/nixpkgs` returned 0. The official search index was not re-queried, so this is weaker than the main report's Nix evidence |
| npm | 404 | [registry](https://registry.npmjs.org/torlek) |
| PyPI | 404 | [JSON API](https://pypi.org/pypi/torlek/json) |
| GitHub repositories | A exact | Repository-name search returned 2 results, neither named exactly `torlek` (`Torlek/de.idnow.example`, `Torlek/Test`) |
| GitHub account / organization | H user | `torlek` is a personal account created in 2013 with 7 public repositories; `torlek-dev`, `torlek-cli` and `torlekhq` returned 404. The project would need another organization name |
| `.dev` registration | RDAP 404 | [Google RDAP](https://pubapi.registry.google/rdap/domain/torlek.dev); no registration returned, not purchase availability. `torlek.app` was also 404 |
| `.sh` registration | N | The registry's web form does not run a lookup without JavaScript (see the [domain follow-up](domain-trademark-followup.md)). Theo does not plan to buy a domain, so this was not pursued |
| `.com` | H, not software | Registered 2026-08-14 with parking nameservers; the site is a residential HVAC company in the San Francisco Bay Area |
| USPTO | A, no results | Theo's browser search, 2026-11-02 (the sandboxed browser cannot reach it) |
| EUIPO and national offices (TMview) | A, no results | Theo's browser search, 2026-11-02 |
| WIPO Global Brand Database | A, no results | Theo's browser search, 2026-11-02 |
| Businesses (web search) | No exact match | No business or software product with the exact name was found. The nearest names are Tork (a hygiene brand) and the Torx software companies |
| Meaning (EN / FR) | None found | No entry on [en.wiktionary](https://en.wiktionary.org/api/rest_v1/page/definition/torlek) (404) or [fr.wiktionary](https://fr.wiktionary.org/w/api.php?action=query&titles=torlek&format=json) (missing). No native-speaker or slang check was done |
| Shell builtin / common tool | A | `type torlek` reported not found and there is no bash builtin of that name; no PATH-wide or distro file inventory was done |

**Pronunciation:** EN "TOR-lek", FR "tor-lek". The same in both languages, spelled the way it
sounds, 6 ASCII letters. It works as `torlek`, `torlek daemon`, `torlek-next`/`torlek-dev` and
`~/.config/torlek/`.

## Corrections to earlier statements

The [issue comment](https://github.com/Androlax2/agent-cockpit/issues/339#issuecomment-6578710906)
described the `torlek` GitHub account as having 2 empty repositories. That count came from the
repository-name search; the account itself lists 7 public repositories. The comment also listed
TMview and WIPO before USPTO had been checked; Theo checked USPTO afterwards and it returned no
results.

## Limits that remain

- **No legal clearance.** Searches returned no marks, but similar-mark and phonetic analysis was
  not done. A professional clearance is needed before a public launch.
- **Domains.** `.dev`/`.app` returned no registration and `.sh` is unchecked. Theo decided not to
  buy a domain, so documentation and installers must not assume a product domain.
- **Nix** has only the path and code-search evidence above. COPR,
  third-party taps and scoped npm packages were not checked.
- **No native-speaker or slang check** in English, French or other languages.
