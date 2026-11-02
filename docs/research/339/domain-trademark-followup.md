# #339 / PR #534 — domain and trademark follow-up

Observation date: **2026-11-02**, verified with `date --iso-8601=seconds`:
`2026-11-02T12:23:46+01:00`. All requests below occurred on that date.
Scope: only crewkiln, runlark, runtide, runhelm, crewspan, crewport, taskhelm, taskwake.
Priority: Crewkiln and Runlark. Public unauthenticated HTTP reads; at most two concurrent requests.
No accounts, live/user-desktop interaction, internal-API guessing, cookies exported, purchases or registrations.
No naming decision or legal clearance. Package research belongs to the separate assignment.

## 1. What authoritative registration evidence was obtained?

### Takeaway

**All eight `.dev` requests returned HTTP 404 with RDAP `errorCode:404`.**
This closes the previous timeout-only lookup gap, but means **no returned registration**,
not that a domain is purchasable, unreserved, affordable or legally usable. [IANA bootstrap](https://data.iana.org/rdap/dns.json)
No `.sh` registration result was established; the two finalist web-form attempts returned shells.

### Cited Findings

#### Endpoint provenance

- GET [IANA DNS RDAP bootstrap](https://data.iana.org/rdap/dns.json): **HTTP 200**;
  `publication:"2026-10-28T22:00:04Z"`; the service containing `dev` specifies
  **`https://pubapi.registry.google/rdap/`**. The complete service list has **no `sh` entry**.
- GET [IANA `.sh` delegation](https://www.iana.org/domains/root/db/sh.html): **HTTP 200**;
  `RDAP Base URLs` displays **`-`**; WHOIS server is `whois.nic.sh`; website is
  `http://www.nic.sh/`. This is a missing published RDAP route, not a missing domain.
- GET [`http://www.nic.sh/`](http://www.nic.sh/): **HTTP 200**, final URL
  [`https://nic.sh/`](https://nic.sh/); the registry homepage links `whois-search.htm`.
  No RDAP endpoint was established from this homepage. The WHOIS hostname was not queried directly.
- GET [`https://nic.sh/whois-search.htm`](https://nic.sh/whois-search.htm): **HTTP 200**;
  HTML exposes `<form method="get" id="whoisForm">` with input `name="domain_name"`
  and no action attribute. The finalist URLs below follow that actual public form.

#### Exact `.dev` requests — each made once in this follow-up

Every row returned `Content-Type: application/rdap+json; charset=utf-8`,
HTTP **404**, `errorCode:404`, `title:"Not Found"`; the final column quotes `description[0]`.

| Candidate | Exact GET URL | Literal RDAP description |
| --- | --- | --- |
| **Crewkiln** | [https://pubapi.registry.google/rdap/domain/crewkiln.dev](https://pubapi.registry.google/rdap/domain/crewkiln.dev) | `crewkiln.dev not found` |
| **Runlark** | [https://pubapi.registry.google/rdap/domain/runlark.dev](https://pubapi.registry.google/rdap/domain/runlark.dev) | `runlark.dev not found` |
| Runtide | [https://pubapi.registry.google/rdap/domain/runtide.dev](https://pubapi.registry.google/rdap/domain/runtide.dev) | `runtide.dev not found` |
| Runhelm | [https://pubapi.registry.google/rdap/domain/runhelm.dev](https://pubapi.registry.google/rdap/domain/runhelm.dev) | `runhelm.dev not found` |
| Crewspan | [https://pubapi.registry.google/rdap/domain/crewspan.dev](https://pubapi.registry.google/rdap/domain/crewspan.dev) | `crewspan.dev not found` |
| Crewport | [https://pubapi.registry.google/rdap/domain/crewport.dev](https://pubapi.registry.google/rdap/domain/crewport.dev) | `crewport.dev not found` |
| Taskhelm | [https://pubapi.registry.google/rdap/domain/taskhelm.dev](https://pubapi.registry.google/rdap/domain/taskhelm.dev) | `taskhelm.dev not found` |
| Taskwake | [https://pubapi.registry.google/rdap/domain/taskwake.dev](https://pubapi.registry.google/rdap/domain/taskwake.dev) | `taskwake.dev not found` |

#### Finalist `.sh` form attempts — not executed WHOIS-result evidence

| Exact GET URL | Actual outcome |
| --- | --- |
| [https://nic.sh/whois-search.htm?domain_name=crewkiln.sh](https://nic.sh/whois-search.htm?domain_name=crewkiln.sh) | **HTTP 200**, same form; hidden `id="whois-result"` section with empty `<pre></pre>`; no candidate text. |
| [https://nic.sh/whois-search.htm?domain_name=runlark.sh](https://nic.sh/whois-search.htm?domain_name=runlark.sh) | **HTTP 200**, same hidden empty result container; no candidate text. |

### Inferences

- Both finalists now have equivalent, bounded `.dev` registration evidence; this result does not
  distinguish them or reverse any existing software-collision evidence. [Crewkiln RDAP](https://pubapi.registry.google/rdap/domain/crewkiln.dev); [Runlark RDAP](https://pubapi.registry.google/rdap/domain/runlark.dev)
- The `.sh` form responses do not show that a backend lookup executed. Empty hidden containers
  must not be recorded as WHOIS “not found.” [Crewkiln form request](https://nic.sh/whois-search.htm?domain_name=crewkiln.sh)

### Gaps

- Reservation, premium pricing and registrar eligibility remain unchecked for every `.dev` name.
- No verified `.sh` RDAP endpoint was discovered. The other six `.sh` names were not queried;
  finalist `.sh` registration remains unknown. No registrant records or registrant PII were retained.

## 2. Were USPTO or EUIPO exact/similar software-class searches executed?

### Takeaway

**No candidate/class trademark query executed in either register.** USPTO returned a JavaScript
application shell; EUIPO first reset the connection, then its public search landing page explicitly
blocked the request. These are not zero results. [USPTO](https://tmsearch.uspto.gov/); [EUIPO](https://www.euipo.europa.eu/en/search-ip)

### Cited Findings

| Exact request | Status or literal error | What actually executed |
| --- | --- | --- |
| GET [https://tmsearch.uspto.gov/](https://tmsearch.uspto.gov/) | **HTTP 200**, title `Trademark search`; body contains `<app-root></app-root>` and module `main.js`; **0 HTML forms**. | Portal retrieval only; no wordmark input, class filter or results. |
| GET [https://euipo.europa.eu/eSearch/](https://euipo.europa.eu/eSearch/) | **No HTTP status**; `TypeError`, code `ECONNRESET`; message: `ECONNRESET: The socket connection was closed unexpectedly. For more information, pass \`verbose: true\` in the second argument to fetch()` | Entry-point request failed before a candidate query. |
| GET [https://www.euipo.europa.eu/en/search-ip](https://www.euipo.europa.eu/en/search-ip) | **HTTP 403**; `403 ERROR`; `The request could not be satisfied.`; **`Request blocked.`**; `Generated by cloudfront (CloudFront)` | Entry point blocked; no query. All further EUIPO access stopped. |

The 403 body also says: `We can't connect to the server for this app or website at this time.`
Its source does **not** establish a greyproxy denial, authentication requirement or the precise
CloudFront rule involved. No alternate tool, hostname, search mirror or route was tried afterward.
[Blocked URL](https://www.euipo.europa.eu/en/search-ip)

### Inferences

- The earlier timeouts are superseded only as connectivity observations: USPTO is now an observed
  shell, and EUIPO has an observed reset/403. Trademark coverage has **not** improved into a
  completed search. [USPTO portal](https://tmsearch.uspto.gov/); [EUIPO landing page](https://www.euipo.europa.eu/en/search-ip)

### Gaps

- **Final bounded USPTO feasibility pass, 2026-11-02T12:32:05+01:00:** isolated headless operation
  was authorized; lack of a native browser tool was not treated as lack of installed capability.
  `shutil.which` found no `chromium`, `chromium-browser`, `google-chrome`, `firefox` or `playwright`.
  Default Python had no `playwright`, `selenium` or `pyppeteer`; Node resolution returned
  `MODULE_NOT_FOUND` for `playwright`, `@playwright/test`, `playwright-core`, `puppeteer`, `puppeteer-core`.
- Nevertheless, browser files existed under `/opt/google/chrome` and `~/.cache/ms-playwright/`;
  cache links also named `/home/theo/.cache/agent-browser/lib/python3.14/site-packages/playwright/driver/package`.
  Cache links do not prove importability.
- Safe launch attempted exactly `/home/theo/.cache/ms-playwright/chromium-1248/chrome-linux64/chrome
  --headless --user-data-dir=/tmp/opencode/uspto-browser-check-ojiu6r4u --dump-dom about:blank`
  with a fresh temporary profile, subsequently removed; no browser sandbox/security flags changed.
  **Exit status -6 (SIGABRT)**, empty stdout; literal fatal message **`No usable sandbox!`**
  at `content/browser/zygote_host/zygote_host_impl_linux.cc:130`; also
  `ptrace: Operation not permitted (1)`. Stopped at that safe-launch failure; no alternate launch,
  sandbox bypass, install, download, host request or live browser/profile access was attempted.
- This pass made **no USPTO page navigation or candidate query**; CREWKILN/RUNLARK class-9/42
  counts remain unknown. EUIPO remained entirely stopped, with no further requests or alternatives.
- Exact words **CREWKILN, RUNLARK, RUNTIDE, RUNHELM, CREWSPAN, CREWPORT, TASKHELM, TASKWAKE**
  and similar marks remain unsearched in USPTO/EUIPO, with intended Nice classes **9 and 42**.
  These are missing-search inputs, **not submitted syntax**. Counts, owners, status,
  application/registration numbers and goods/services remain unknown for all eight names.

## 3. What should the coordinator carry forward?

### Takeaway

Update the domain matrix to **RDAP 404 / no returned registration** for all eight `.dev` names;
keep every trademark cell incomplete. Retain the `.sh` limitations explicitly.
[Authoritative routing](https://data.iana.org/rdap/dns.json); [USPTO](https://tmsearch.uspto.gov/); [EUIPO](https://www.euipo.europa.eu/en/search-ip)

### Cited Findings

- This follow-up improves the prior report's `.dev` timeout rows, not its naming decision or
  legal conclusions. Existing source context: [main report](../2026-11-02-339-product-naming.md)
  and [finalist appendix](crewkiln-runlark.md).

### Inferences

- No availability ranking follows from eight identical RDAP negatives; existing collision
  evidence remains the differentiator. [Prior eight-name comparison](../2026-11-02-339-product-naming.md)

### Gaps

- Human follow-up remains: operate the official public trademark search interfaces when reachable,
  prioritizing CREWKILN/RUNLARK; record exact and similar-mark searches, classes 9/42, live/dead or
  pending status, record identifiers, owners and relevant goods/services. No such action was initiated.
- Exact coordinator blocker: `https://www.euipo.europa.eu/en/search-ip` returned **HTTP 403,
  “Request blocked.”** USPTO needs a functioning sandboxed browser; this session's isolated launch failed above.
- The capability blocker is tracked as cockpit item **`1820d58f`**. Neither the browser failure
  nor the CloudFront response is being claimed as a greyproxy denial; no `--no-sandbox` bypass was used.
- If `.sh` matters, a human must obtain actual registry WHOIS/search results; the GET shells here
  provide none. Domain purchase eligibility and legal assessment remain separate unfinished checks.
