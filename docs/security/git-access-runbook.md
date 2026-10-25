# Git access: implementation status and operator gates

## Current status

This is **not deployed protection**. The new `git-access.py` helper is not connected to the launchers. Existing launcher credentials, GitHub protections and running sandboxes are unchanged.

Implemented and tested locally:

- Read-only identity inspection against a private, host-owned registry: exact checkout/gitdir/common-dir paths, local-mirror edges, hostile config/includes/rewrites, linked worktrees and numeric GitHub repository identity.
- No-bypass `~ALL` history/deletion and default-branch PR floor evaluation, with active-rule corroboration and preservation of stronger requirements.
- Dedicated App inspector authentication, repository/permission-narrowed installation tokens, paginated reads, fixed-host/no-redirect transport, and secret-free denials.

Host-confirmed enrollment is now implemented: an explicitly approved App/repository mapping, matching local metadata, a complete fresh protection check and an exact confirmation phrase are required before an atomic owner-only registry update. Confirmation is followed by another fresh check, not reuse of earlier evidence. Concurrent registry changes abort rather than being overwritten. This code has only been fixture-tested for enrollment; no production registry is provisioned or enrolled.

Still unavailable: initial identity bootstrap for an unprotected repository, verified runtime credential roles, rate admission/supervisor, broker writes, launcher integration, actual read-only/socket containment and migration/rollout. A `verified` inspection or successful enrollment is **not** a sandbox lease or Git-write authorization.

Administrative preview is implemented in `git-protection.py`: `tools/agent-github.sh protect PROJECT` (also `make agent-github-protect PROJECT=...`) prints an identity-bound before/after mutation plan. It no longer executes the legacy name-based overwrite. Exit 0 with `mode: preview` means a plan was produced, not that protections were installed or verified. The project must already be in the host registry; preview cannot bootstrap enrollment or assign baseline ownership.

Plans create separate all-branch history and default-PR baselines, never adopt rules by name, preserve stronger owned rules/CI/review parameters and leave unrelated rulesets/classic protection untouched. Missing owned IDs, duplicate/colliding names, unknown bypass data or changed scope require operator review. Preview uses only the dedicated App inspection role; it performs no protection writes or registry updates and releases no credentials.

**Apply code is fixture-tested, not approved for production execution.** A separately approved host invocation may add `--apply` to `tools/agent-github.sh protect PROJECT`. It prints the full plan and requires the exact `APPLY OWNER/REPO NUMERIC_ID` phrase, then obtains fresh App evidence and aborts if the reviewed plan, registry or local identity changed. Each mutation has its own complete recheck and 30-second round deadline. Admin authentication is the host's existing `gh` login, not the inspector App or agent credential: fixed executable/config, agent `GH_*`/`GITHUB_*` variables stripped, credential captured only in host memory. It immediately rechecks numeric repository identity and uses fixed-host/no-redirect/no-retry HTTP for only the planned ruleset POST/PUT. No admin operation is available through inspection, launch or a broker route.

After each write the App independently reads back the requested fields, including preserved stronger rules, and final success requires the existing complete protection evaluator. Creation records only validated numeric IDs in the owner-only registry. Partial creation retains successful protections/IDs and remains read-only; there is no rollback, deletion, name-based adoption or automatic retry. Timeout, malformed response, lost response or failed post-check means **unknown/partial upstream outcome**: reconcile read-only before another reviewed attempt. This is not atomic with GitHub administrator changes. Identity/App approval must already exist in a host-reviewed record; do not fabricate pending records or copied approvals to bypass the still-unimplemented initial-bootstrap workflow.

The legacy token-creation template no longer requests Workflows write; it retains Contents/Issues/Pull requests write and Actions/Commit statuses/Metadata read. This edits a future setup template, **not existing grants or their revocation**, and does not enroll a token for broker/native-read use. GitHub's [documented URL parameter table](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens) lists `statuses` but not `checks`, and documents a fine-grained-token Checks limitation. No invented `checks=read`, broad classic scope or human-read fallback is added. The exact supported read role/Checks route must be proved before rollout; failure leaves it read-only/unavailable, not silently green CI.

Source: [`git-access.py`](../../config/bin/.local/bin/greywall-agents/git-access.py). Tests: [`test_git_access.py`](../../tools/tests/test_git_access.py). Reviewed contracts: [protection spec](../superpowers/specs/2026-10-23-fail-closed-git-protection-design.md) and [broker spec](../superpowers/specs/2026-10-23-git-credential-broker-design.md).

## Safe local inspection

From the repository root, this reads local metadata and returns a JSON decision:

```sh
/usr/bin/python3 -I config/bin/.local/bin/greywall-agents/git-access.py inspect --checkout "$PWD"
```

Exit 1 means read-only/denied; exit 0 means the configured inspector verified identity/protections, **not** that broker/containment rollout gates passed. `--help` lists supported CLI commands. Without an enrolled record/App, inspection never consults your personal GitHub login or exports a credential.

The host admin script also exposes `inspect`/`enroll`; the Makefile has `agent-github-inspect CHECKOUT=...` and an explicit-ID `agent-github-enroll` target. These are host-run entry points, not launcher integration. Inspect denial propagates a nonzero make status while retaining JSON output. See `tools/agent-github.sh enroll --help` for the exact enrollment flags; all of checkout, project key, canonical repository, numeric repository/App IDs and both baseline IDs must be supplied, not derived from a basename or mutable remote.

Do not populate a real registry by copying test fixture approvals. The registry location is fixed under `~/.config/greywall-agents/git-repositories.json`, with owner-only permissions; no command-line/environment registry override is accepted. Registry and keyring access must stay outside sandbox mounts when launcher integration is implemented.

Registry version 1 has `repositories` plus a host-reviewed `inspector_apps` list. Each App entry binds App/client/installation/owner IDs, `owner_login`, `approved_repository_ids`, key epoch, exact permission map and explicit setup approval; Administration write requires its own explicit approval flag. Repository records reference App/installation IDs, **not copied approval flags**: every inspection resolves the current global profile, so revoking setup/repository approval blocks existing enrollments even with a cached auth token. The operator must approve actual installation access and IDs before this profile exists. Enrollment preserves other records; it cannot provision an App, broaden repository access, grant permissions or reassign a project to another repository. Profiles hold references/approval metadata, never keys or tokens.

## Controlled App trial: approval required before setup

Initial trial record (2026-10-23): Theo approved creating private `Androlax2/cockpit-inspector-trial` with a generated README only (repository ID `1430990603`, owner `39646949`, default branch `main`). GitHub's rulesets GET returned HTTP 403: "Upgrade to GitHub Pro or make this repository public to enable this feature." At that point no App, key, token, installation or ruleset existed, and the trial stopped for an explicit visibility decision. This entitlement result under the host setup identity was not App permission proof; no production repository changed.

Subsequent approved trial actions: Theo chose public visibility for **this README-only trial only**, after a host command verified its ID/content. Active, empty-bypass test baselines are `25720025` (all-branch history) and `25720027` (default PR). Test App `5441353` / client `Iv23liqaSlHsDhJTpHbW`, installation `174470802`, was created/installed only on this trial with Metadata read + Administration read. Its key was validated and stored host-side; no secret entered the project/chat.

**Live permission result:** the actual App authenticated successfully with exactly those read grants, but GitHub omitted `bypass_actors` from **both** ruleset detail responses. The new evaluator correctly returned `read_only` / `protection_uninspectable`. This is not successful protection proof, and no registry enrollment or rollout was enabled. Do not weaken the missing-field check. Administration write, if considered for a further trial, requires explicit separate approval and carries protection-mutation/repository-deletion risk; no production permission approval follows from a test.

**Subsequent test-only approval/result:** Theo explicitly approved Administration read/write on this App's **trial-only** installation. Repeating the same hash-pinned inspector authenticated exact Metadata read + Administration write grants, read explicit empty bypass lists for both active baselines and passed the evaluator (7 installation inspection requests + 1 token mint, plus a preliminary App identity read). No registry was written and rollout stayed disabled. Administration read was insufficient in this trial; Administration write supplied visibility. Production grants/installations remain a separate approval with the settings-mutation/deletion risk, never an automatic consequence of this proof.

Further live steps need the relevant separate approvals. None is authorized merely by choosing the App architecture or approving implementation:

1. Register/install a dedicated test inspector App on an explicitly approved disposable repository, not all repositories or an unrelated GitHub account's projects.
2. Begin with the reviewed candidate **Metadata read + Administration read**. Confirm App/client/installation/owner/repository IDs and keep its signing key host-only. The implementation's proposed keyring service is `cockpit-github-inspector`, keyed by `app`/client ID; no real key is stored by tests.
3. Mint a repository-narrowed installation token under approved authority and request complete baseline/rule details, including `bypass_actors`. Omission is unknown, not an empty bypass list; a successful personal-login read is not proof of App visibility.
4. If read grants cannot expose bypass details, stop read-only and report the result. **Administration write needs a separate permission approval** with the explicit risk that a compromised inspector could modify protections. Never automatically elevate grants, add the App to bypass lists or fall back to personal credentials.
5. Production App installation/grants and repository enrollment require reviewed proof and separate approval. A test installation/token does not authorize production rollout or protection changes.

Do not paste a key/token into chat, a command argument, a repository file or `.handoff/`. The signer reads its private key host-side and passes it to OpenSSL through an anonymous file descriptor. Curl receives authorization through stdin, disables curl configuration and does not follow redirects. Tests use synthetic keys and fake HTTP responses only.

## Remaining rollout acceptance

- Run the full test suite and obtain independent security review before deployment. The author's self-review is not independent certification.
- Prove real Greywall recursive read-only project/metadata mounts and per-session socket isolation. Metadata pinning alone permits index/ref updates and is not that proof. No PATH shim or missing token substitutes for filesystem enforcement.
- Implement the shared principal ledger: inspection requests share their App installation bucket, forwarding/read PATs share the appropriate user pool, and App-token mint overhead is counted separately. No assumed free quota or one allowance per token.
- Keep approved timing: 300-second scheduled checks, 600-second hard local admission ceiling, 30-second complete inspection deadline; **each mutative upstream request gets its own fresh inspection**. First failed scheduled check contains immediately, not at expiry. Suspend/resume and supervisor death invalidate/contain as specified.
- Preserve necessary GitHub reads/issues/comments/reviews/PRs/Projects and authorized merges. No generic authenticated mutation tunnel, bypass, review dismissal, forged check/status, weaker CI or raw write-token fallback.
- Review each affected CI job's append-only/new-branch replacement and retained protected branches. Keep job purpose, required checks and repeated-run/deployment behavior; do not disable jobs silently. Stack/fork/queue compatibility must be proven before affected cutover.
- Inventory active/sleeping sessions, clones/worktrees/common directories, submodules, external writers and previously exposed tokens. Revoke legacy credentials on GitHub and confirm the result; deleting a local secret or restarting a session is not revocation. Quarantine out-of-process Docker/automation writers without destroying working trees.

Rollback is enforced read-only, never unconditional credential export or removal of stronger GitHub protections. An interrupted/forwarded write may have completed remotely; reconcile using read-only data rather than replaying it or claiming it was rolled back.
