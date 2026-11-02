# Package availability follow-up — #339 / PR #534

Observation date **2026-11-02**, verified by `date -u` at **2026-11-02T11:23:46Z**.
All table times are request-start **UTC on that date**; statuses are actual HTTP responses.
Scope: only `crewkiln`, `runlark`, `runtide`, `runhelm`, `crewspan`, `crewport`, `taskhelm`, `taskwake`.
Read the prior [report](../2026-11-02-339-product-naming.md) and its four candidate appendices.
At most **two simultaneous network requests**; public GETs plus two read-only Nix search POSTs.
No explicit policy denial or timeout occurred in this follow-up. Earlier timeouts remain historical failures, not negatives.
**No new exact package collision found in these checks. No name is selected or claimed available.**

## 1. What do Debian and Fedora return for all eight names?

### Takeaway

Debian returned successful empty **package-name substring** searches for all eight; Fedora returned zero displayed package results for all eight, with a spelling suggestion for Runtide. Literal source URLs and outcomes follow.

### Cited Findings

Debian responses explicitly say names “contain” the candidate “in all suites, all sections, and all architectures,” followed by **“Sorry, your search gave no results.”** These are successful search negatives, not 404s.

| Name | UTC start | Literal Debian GET | HTTP / result |
| --- | --- | --- | --- |
| crewkiln | 11:24:14.957 | [Search](https://packages.debian.org/search?keywords=crewkiln&searchon=names&suite=all&section=all) | 200 / empty substring results |
| runlark | 11:24:34.691 | [Search](https://packages.debian.org/search?keywords=runlark&searchon=names&suite=all&section=all) | 200 / empty substring results |
| runtide | 11:24:34.964 | [Search](https://packages.debian.org/search?keywords=runtide&searchon=names&suite=all&section=all) | 200 / empty substring results |
| runhelm | 11:24:37.510 | [Search](https://packages.debian.org/search?keywords=runhelm&searchon=names&suite=all&section=all) | 200 / empty substring results |
| crewspan | 11:24:38.017 | [Search](https://packages.debian.org/search?keywords=crewspan&searchon=names&suite=all&section=all) | 200 / empty substring results |
| crewport | 11:24:40.566 | [Search](https://packages.debian.org/search?keywords=crewport&searchon=names&suite=all&section=all) | 200 / empty substring results |
| taskhelm | 11:24:40.853 | [Search](https://packages.debian.org/search?keywords=taskhelm&searchon=names&suite=all&section=all) | 200 / empty substring results |
| taskwake | 11:24:43.253 | [Search](https://packages.debian.org/search?keywords=taskwake&searchon=names&suite=all&section=all) | 200 / empty substring results |

Fedora uses its default `query` search, **not a verified exact-name or substring operator**. Each response shows release counts of zero for Fedora 43/44/45/Rawhide and EPEL 8/9/10.2/10.3/10.4; “Show related packages” is unchecked. This is the website's indexed search scope, not every historical Fedora release or COPR.

| Name | UTC start | Literal Fedora GET | HTTP / result |
| --- | --- | --- | --- |
| crewkiln | 11:24:14.957 | [Search](https://packages.fedoraproject.org/search?query=crewkiln) | 200 / “No results found!” |
| runlark | 11:24:34.692 | [Search](https://packages.fedoraproject.org/search?query=runlark) | 200 / “No results found!” |
| runtide | 11:24:35.243 | [Search](https://packages.fedoraproject.org/search?query=runtide) | 200 / zero release counts, empty results area |
| runhelm | 11:24:37.780 | [Search](https://packages.fedoraproject.org/search?query=runhelm) | 200 / “No results found!” |
| crewspan | 11:24:38.300 | [Search](https://packages.fedoraproject.org/search?query=crewspan) | 200 / “No results found!” |
| crewport | 11:24:40.791 | [Search](https://packages.fedoraproject.org/search?query=crewport) | 200 / “No results found!” |
| taskhelm | 11:24:41.123 | [Search](https://packages.fedoraproject.org/search?query=taskhelm) | 200 / “No results found!” |
| taskwake | 11:24:43.523 | [Search](https://packages.fedoraproject.org/search?query=taskwake) | 200 / “No results found!” |

Runtide's same URL was re-read at **11:24:50.087**, HTTP **200**: **“Did you mean runtime?”** with no package cards and all nine release counts zero. The suggestion is not a Runtide collision and was not followed. [Source](https://packages.fedoraproject.org/search?query=runtide).

### Inferences

- Replace Debian's prior `U` cells with **A substring** and Fedora's with **A default query**, retaining Runtide's suggestion qualification. These findings supersede unavailable observations, not existing software collisions.

### Gaps

- Neither service was searched for files shipped inside differently named packages. Debian source-name/contents modes, old archive snapshots, Fedora COPR and related-package expansion were not checked. Fedora's exact tokenization/case/fuzzy semantics remain unverified.

## 2. What closes crates.io and Homebrew gaps for the six rejected names?

### Takeaway

All six exact crates.io records returned **404**; complete official Homebrew core/cask source-path lists contained **none of the eight strings**. Registry/resource absence does not establish publication eligibility. Sources below preserve the different scopes.

### Cited Findings

The exact crates API pattern was already documented for Runtide/Taskwake in the prior appendix; the other four names were substituted into that same discovered endpoint. Every response was ``{"errors":[{"detail":"crate `<name>` does not exist"}]}`` with the actual candidate substituted.

| Name | UTC start | Exact crates.io GET | HTTP / outcome |
| --- | --- | --- | --- |
| runtide | 11:25:11.213 | [Record](https://crates.io/api/v1/crates/runtide) | 404 / crate does not exist |
| taskwake | 11:25:11.213 | [Record](https://crates.io/api/v1/crates/taskwake) | 404 / crate does not exist |
| runhelm | 11:26:35.731 | [Record](https://crates.io/api/v1/crates/runhelm) | 404 / crate does not exist |
| taskhelm | 11:26:35.731 | [Record](https://crates.io/api/v1/crates/taskhelm) | 404 / crate does not exist |
| crewport | 11:26:36.092 | [Record](https://crates.io/api/v1/crates/crewport) | 404 / crate does not exist |
| crewspan | 11:26:36.092 | [Record](https://crates.io/api/v1/crates/crewspan) | 404 / crate does not exist |

Earlier in this follow-up, four inherited search URLs also succeeded. These are **API search** negatives, not asserted substring semantics; each returned `crates:[]`, `meta.total:0`, `next_page:null`, `prev_page:null`.

| UTC start | Literal crates.io GET | HTTP |
| --- | --- | --- |
| 11:25:13.616 | [runhelm search](https://crates.io/api/v1/crates?q=runhelm&per_page=100) | 200 |
| 11:25:13.621 | [taskhelm search](https://crates.io/api/v1/crates?q=taskhelm&per_page=100) | 200 |
| 11:25:13.982 | [crewport search](https://crates.io/api/v1/crates?page=1&per_page=10&q=crewport) | 200 |
| 11:25:13.982 | [crewspan search](https://crates.io/api/v1/crates?page=1&per_page=10&q=crewspan) | 200 |

Homebrew lists were fetched **once each**, using official repository URLs established by the prior finalist appendix. Every returned path was searched case-insensitively for each of the eight candidate substrings; **zero matches**, including alias paths, in both complete lists.

| UTC start | Literal GET | HTTP / entries / completeness | Observed tree SHA |
| --- | --- | --- | --- |
| 11:25:01.312 | [Homebrew core](https://api.github.com/repos/Homebrew/homebrew-core/git/trees/HEAD?recursive=1) | 200 / 9,450 / `truncated:false` | `184a6c38682139b48cdb80d5f3db0df843cae326` |
| 11:25:01.312 | [Homebrew casks](https://api.github.com/repos/Homebrew/homebrew-cask/git/trees/HEAD?recursive=1) | 200 / 7,927 / `truncated:false` | `ad5b60969be8ed94f29a1ab27954e2128c027d3b` |

**Existing exact collision retained:** prior Taskhelm evidence records npm package **0.4.1** with `bin:{"taskhelm":"bin/taskhelm.js"}`, plus PyPI project **0.3.2**. Those sources were **not re-fetched here**; new distro/crate negatives do not negate them. [npm](https://registry.npmjs.org/taskhelm); [PyPI](https://pypi.org/pypi/taskhelm/json); [prior appendix](runhelm-taskhelm.md).

### Inferences

- For the six rejected rows, crates.io can become **404 exact API** and both Homebrew columns **A complete paths**. The Homebrew check also refreshes the two finalist rows.
- Existing non-package collisions remain relevant, especially the documented exact `taskwake` installed CLI. This is prior evidence, not a fresh finding. [Taskwake README](https://github.com/sdpwh7-ux/taskwake/blob/main/README.md).

### Gaps

- Homebrew coverage is source **paths**, not every formula body, cask `name` display string, installed binary, third-party tap or generated API list. No direct `formulae.brew.sh` retry was needed; its earlier timeouts are not retroactively successful requests.
- No reservation/publication eligibility checks. Finalist direct crates.io APIs were not retried: their previously documented official-index 404 plus working-parent evidence remains the precise scope. [Prior evidence](crewkiln-runlark.md).

## 3. Was evaluated Nix package evidence feasible, and what remains?

### Takeaway

**Yes: the official search backend returned zero candidate matches in both evaluated stable 26.05 and unstable indexes.** This is stronger than the earlier truncated Nixpkgs source tree, but still bounded to the service's indexed fields and revisions. [Stable POST endpoint](https://search.nixos.org/backend/latest-52-nixos-26.05/_search); [unstable POST endpoint](https://search.nixos.org/backend/latest-52-nixos-unstable/_search).

### Cited Findings

| UTC start | Discovery request | HTTP / actual outcome |
| --- | --- | --- |
| 11:24:50.087 | [Nix search HTML](https://search.nixos.org/packages?channel=unstable&query=crewkiln) | 200 / empty body plus “This site cannot work without javascript enabled”; **no search results in HTML** |
| 11:25:24.375 | [Linked JS bundle](https://search.nixos.org/static/js/index.ed2927e0.js) | 200 / 180,478 characters; discovered backend, schema, query fields and search method |
| 11:25:41.106 | [Same bundle, configuration inspection](https://search.nixos.org/static/js/index.ed2927e0.js) | 200 / confirmed stable `nixos-26.05`, rolling `nixos-unstable`, default 26.05 |

The bundle explicitly constructs `/backend/latest-<schema>-<branch>/_search`, uses POST and schema **52**, and supplies the public website's Basic-auth configuration. The requests used that published website configuration, **not a personal account or newly created account**. No authentication/access error preceded them. [Discovery source](https://search.nixos.org/static/js/index.ed2927e0.js).

Each read-only JSON POST used `size:1`, `track_total_hits:true`, `_source:["type","package_attr_name","package_pname","package_programs"]`, and `query:{"term":{"type":"package"}}`. Its `aggs` object had exactly the eight candidate names as keys; for each name `n`, the value was:

```json
{"filter":{"bool":{"should":[{"wildcard":{"package_attr_name":"*<n>*"}},{"wildcard":{"package_pname":"*<n>*"}},{"wildcard":{"package_programs":"*<n>*"}}],"minimum_should_match":1}}}
```

`<n>` above denotes literal substitution of each supplied lowercase name, **not literal angle brackets sent to the server**. This tests **substring wildcard matches in indexed attribute/name/program terms**, not just exact attribute equality. No case-insensitive option was sent; do not claim exhaustive case-variant coverage.

| UTC start | Exact POST URL | HTTP / package documents / backend execution | Concrete response index |
| --- | --- | --- | --- |
| 11:26:10.122 | [Stable](https://search.nixos.org/backend/latest-52-nixos-26.05/_search) | 200 / 144,495 (`relation:eq`) / 117 ms | `nixos-52-26.05-9bb52223f4f6a0cc606fd0852d8784f896c137d5` |
| 11:26:10.123 | [Unstable](https://search.nixos.org/backend/latest-52-nixos-unstable/_search) | 200 / 151,829 (`relation:eq`) / 124 ms | `nixos-52-unstable-1c89083e451e891152365b799595cf7790a43325` |

Both responses: `timed_out:false`; shards `total:1`, `successful:1`, `skipped:0`, `failed:0`. Each of the eight aggregation keys returned **`doc_count:0`** in each index. Successful nonempty controls came from the same responses: stable sample `haskellPackages.hix` / pname `hix`; unstable `iio-sensor-proxy` / program `monitor-sensor`. [Stable](https://search.nixos.org/backend/latest-52-nixos-26.05/_search); [unstable](https://search.nixos.org/backend/latest-52-nixos-unstable/_search).

### Inferences

- All eight Nix cells can become **A indexed substring, stable 26.05 + unstable**, replacing `U`/partial-source-only evidence while preserving the field/case limits above.
- The HTML-only response is **JS-required**, not a negative; the later JSON POST responses are the successful zero-count evidence. No local Nix evaluation or installation was performed.

### Gaps

- These are the official service's evaluated-package search indexes, not an independent evaluation of every platform, overlay, flake, historical channel, alias or package output. Indexed `package_programs` is not an exhaustive installed-file inventory. Live aliases may advance beyond the concrete indexes recorded above.
- Remaining package limits: publication reservations; normalized/case variants; private/unpublished packages; third-party repositories/taps; exhaustive executable-file ownership. Existing exact software/package collisions still stand.
- Domains and trademarks belong to the separate researcher. No conclusion about those checks, no naming choice, and no change to the prior ranking or #339 acceptance is made here.
