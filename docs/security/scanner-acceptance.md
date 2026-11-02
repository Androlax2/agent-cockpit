# Scanner activation and acceptance

Status: source prepared for #382; activation, measured results and enforcement are
pending. Source review and local fixtures do not establish live scanner coverage.
This work does not hold #560 naming integration or authorize release publication.

## Source and credential boundaries

[CodeQL](../../.github/workflows/codeql.yml) analyzes Rust and Actions separately on
ready pull requests, main pushes and a weekly schedule. Draft PRs and privileged
`pull_request_target` contexts are refused. Checkout does not persist credentials.
The analysis job receives only repository read and security-events write permissions;
it references no environment, App key, deploy key or custom token. Fork analysis must
use GitHub's ordinary PR security model, never a privileged checkout workaround.

Rust uses `build-mode: none`. This can still execute build scripts and procedural
macros through rust-analyzer; it is not execution-free analysis. Use disposable hosted
runners without release/proposal credentials. Both the root `crates/` workspace and
legacy `apps/` workspace need measured coverage before any repository-wide claim.
See GitHub's [compiled-language build options](https://docs.github.com/en/code-security/reference/code-scanning/codeql/build-options-for-compiled-languages).

[Scorecard](../../.github/workflows/scorecard.yml) runs only on main pushes and the
weekly schedule with the default read-only token. It uploads JSON as an Actions
artifact and sets `publish_results: false`. It requests no OIDC or security-events
write permission, and does not use an administrator PAT to improve its score.

Actions are pinned to commits. The pinned Scorecard action internally selects the
container tag `ghcr.io/ossf/scorecard-action:v2.5.0`; this is not a verified immutable
image digest. Record that transitive dependency in activation review. Do not invent
a digest or silently substitute a different image.

## Local proof and its limits

Run the focused fixtures from the repository root:

```sh
python3 -m unittest discover -s tools/tests -p 'test_workflow*.py'
```

The new scanner fixtures evaluate the actual job conditions using the repository's
existing condition helper. They cover ready/draft PRs, main/feature/tag refs,
scheduled events and refusal of privileged PR/manual events. They do not emulate
GitHub's scheduler, permissions, YAML parser or scanner engines. A planted permissive
job condition must fail these fixtures. Existing draft-workflow fixtures also cover
the newly registered CodeQL PR job. No live canary or vulnerable code is required to
prove these local guards.

Before delivery, validate YAML/action inputs with actionlint when available. Missing
actionlint is a reported gap, not a passing check. Do not install tools implicitly.

## Activation order and exact approvals

1. Review the new source delta with the existing #382 reviewer. Preserve all naming,
   release-workflow, approval and publication hunks from current main. Obtain exact
   workflow-delivery approval before any push; the previous PR587 push receipt does
   not cover scanner source. Creating local files does not activate hosted workflows.
2. Deliver through a protected ready PR. Inspect both Rust and Actions analyses at
   its exact merge/head source, then the successful main analyses after merge. A
   skipped draft run is not validation. Record run IDs, source SHAs, tool versions,
   categories `/language:rust` and `/language:actions`, extraction warnings, SARIF
   processing status and actual workspace coverage. Unsupported/missing categories
   or incomplete extraction remain unresolved; do not enable enforcement over them.
3. Inspect every Scorecard check in the main-run JSON artifact. Retain its repository,
   commit, timestamp, scanner version, scores, reasons and details. A successful job
   or aggregate score does not mean that high-risk checks passed. Missing checks,
   unavailable data and negative/unscored values remain unknown, not waived.
4. Only after measured analysis and owner disposition, submit the separately reviewed
   CodeQL ruleset payload through exact host approval. Preserve main's five existing
   checks, non-strict freshness, squash-only PR policy and no-bypass rule. Read back
   the new rule and verify missing/in-progress/failed required results cannot merge
   using a separately approved bounded procedure. Do not weaken protection to test it.
5. Dependabot alerts/security updates and all-external-contributor approval are
   separate settings requests with before/after metadata. Existing setup receipts
   cover neither these actions nor scanner enforcement. Preserve any pending request
   ID and reconcile uncertain results before retrying.

No source commit, push, workflow dispatch, settings request, live canary, secret
transfer, release-draft closure or badge publication is authorized by this document.

## Per-check disposition record

Use the risk definitions associated with the actual scanner version and
[upstream check documentation](https://github.com/ossf/scorecard/blob/main/docs/checks.md).
For **every returned check**, record: name, risk, score, reason/details, evidence URL,
owner, remediation or explicit policy disposition, and verification/resume condition.
An absent expected check gets its own unknown row. Do not replace this record with
an aggregate threshold or silently call a policy exception a successful check.

The following are inspection priorities, not measured findings or granted waivers:

| Checks / subject | Required disposition |
| --- | --- |
| Dangerous-Workflow, Token-Permissions | Inspect each reported workflow and credential path; correct unsafe execution or excess permission before acceptance. |
| Branch-Protection, Code-Review | Compare evidence with the approved sole-maintainer policy. Do not add approvals, strict freshness or a merge queue for points; Theo must explicitly dispose of conflicts. |
| Pinned-Dependencies | Inspect action and transitive container references, including Scorecard's image tag; report unresolved mutable dependencies. |
| Vulnerabilities, Dependency-Update-Tool, SAST | Link real alerts/configuration/analysis evidence; absence of readable data is not absence of vulnerabilities. |
| Signed-Releases, Packaging | Source preparation is not a release. Coordinate actual attestation/publication evidence with the release owner; no test release is authorized here. |
| Remaining returned checks | Preserve every reason and assign an owner; do not omit inconvenient or unscored results. |

## Badge and planted live checks

Issue #382 requests a Scorecard badge. This source deliberately leaves public-results
publication and badge rendering pending; no badge or passing score is claimed.
Theo must either explicitly defer that acceptance item or approve a separate reviewed
publication/badge change after measured results and policy disposition. Do not enable
`publish_results` or add `id-token: write` merely to complete the source preparation.

A planted scanner finding is an optional separately approved coverage probe, not a
local guard test. Before any such run, select a documented query from the exact engine
version, pin its expected rule ID, isolate the fixture from product code, specify the
exact ref/run and cleanup plan, and obtain approval. Record a failing finding and its
removal at the same coverage boundary. Do not claim detection without running the real
engine. Tag-refusal and fork-secret tests remain separate bounded procedures; use only
non-sensitive canaries and never print real credential values.
