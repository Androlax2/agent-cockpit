# Automatic subscription fallback

Todo: `1797cdd6`

## Approved intent

When a subscription is exhausted, continue the current task on the other available
subscription. Automatically return to the original model when its quota recovers,
at a safe stopping point. Theo approved sharing conversation history with the
replacement provider and chose Claude ↔ Codex first (Option A), then explicitly
required OpenCode support too.

Cover native Claude Code, native Codex CLI, and OpenCode sessions using those
subscriptions where the provider actually supports subscription authentication.
No Gemini integration or automatic API spending.

## OpenCode connection confirmed by Theo

Theo showed OpenCode's `/connect` account selector with an already connected
Anthropic account labelled `Claude OAuth`. Use that existing connection rather
than requiring a native-CLI handoff. Verify its active connection, billing mode,
account-matched quota and working model catalog at execution time; the label alone
does not prove any particular request will be subscription-billed.

Anthropic's [authentication policy](https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use)
restricts third-party use of subscription authentication. This is a provider
policy risk, not evidence that OpenCode lacks the technical connection. Do not
implement login, copy/intermediate tokens, spoof clients or bypass provider
rejections. An unusable existing connection pauses routing with an explicit reason.

## Existing evidence

- `apps/cockpit/src/usage.rs` reads Claude and Codex quota caches, including
  freshness/error checks. Display percentages are rounded; routing must use the
  original numeric values instead.
- `apps/cockpit/src/app.rs` owns periodic queue, prompt, restart and sleep dispatch.
- `config/bin/.local/bin/cockpit-agents/{claude,codex}.sh` already launch/resume
  both CLIs through their own sandbox launchers.
- `apps/cockpit/src/remote/conversation.rs` exports bounded visible messages for
  Claude and OpenCode, but not Codex.
- Codex session IDs are verified hashes of native thread UUIDs. Its `say` verb is
  deliberately unsupported; do not type blindly into its terminal.
- Codex quota collection uses a separate `CODEX_HOME`. Its available quota must
  not authorize execution under a different account.
- TODO assignments, pending prompts, continuity and team membership refer to
  native session IDs. Starting another CLI alone does not transfer ownership.
- OpenCode V2 exposes `ctx.session.switchModel({sessionID, model})`; its existing
  cockpit plugin runs in a private standalone server. Switch models in that same
  session rather than replacing OpenCode with a native CLI or editing its database.
- The OpenCode launcher currently injects an Anthropic API key when one exists.
  A provider/model being enabled does not establish subscription billing; routing
  must verify the active connection and must never select that API key as fallback.

## Behavior

1. Inspect quota evidence on the existing periodic dispatch path. Only a confirmed
   exhausted applicable quota triggers fallback; low or unknown quota does not.
2. The alternate subscription is eligible only with fresh, successful evidence of
   remaining capacity for the account and model that execution will actually use.
   Never assume a reset timestamp means capacity has returned. Refresh and confirm.
3. Save the original agent, native conversation identity, exact model and effort,
   checkout, active task and current owner before changing anything. If the model
   or account cannot be established, pause with an actionable reason rather than
   silently substituting a default model/account.
4. Transfer at a verified idle boundary, or a provider-specific verified quota-stop
   boundary. Never interrupt a running tool, background job, subagent, approval,
   unanswered user question, or typed but unsubmitted prompt. A cached 100% reading
   alone is not proof that a process has stopped working.
5. Read bounded visible conversation text and task metadata on the host. Pass it as
   historical context, not fresh user authorization, to the alternate sandbox in
   the same checkout. Include the latest user request and an explicit truncation
   notice when history is bounded. Exclude reasoning, credentials and raw tool
   payloads. Existing files remain the source of truth for work already performed.
6. Retire the old process before allowing the replacement to execute task work.
   Preserve the original conversation on disk for return. Verify the replacement's
   authenticated runtime identity before rebinding task ownership to it.
7. While using the alternate subscription, continue monitoring the original model.
   When fresh quota confirms recovery, wait for the alternate's next safe boundary,
   export its new context, and resume the original conversation with its original
   model and effort. Do not switch a completed task back merely to generate work.
8. If neither subscription is confirmed usable, keep the task pending and retry
   after fresh evidence arrives. If the alternate also exhausts its quota, do not
   repeatedly relaunch it. No API-credit escape hatch.

## Ownership, failures and visibility

- One persisted routing record per logical task/session lineage; one active owner.
  Use a host-owned lock and atomic persistence so two sidebar processes or a
  restart cannot create duplicate workers.
- Persist transition intent before exiting a process. A launch failure leaves
  preserved history and a paused task, not a falsely active replacement. An
  uncertain launch is reconciled against authenticated reports, never retried
  blindly.
- Transfer the active TODO/issue assignment, continuity links and queued prompts
  without replaying consumed prompts or losing new messages. Block queue, restart
  and sleep dispatch from taking over an in-flight routing transition.
- Team members retain only their existing task and role through a host-verified
  replacement. Rotate runtime/epoch binding; do not inherit old approvals or replay
  messages with uncertain delivery. If safe rebinding is impossible, pause that
  member with a reason rather than bypass team authorization.
- User-managed terminals and unsupported agents are not automatically replaced.
  Explicitly API-billed Claude sessions remain untouched.
- Record fallback, return, waiting and failure events through the existing log and
  audit facilities. Audit metadata contains identities/reasons, never history or
  credentials. Use the existing message area; no interface redesign.
- Closing/cancelling the logical task stops automatic relaunches. Manual agent or
  billing changes cancel automatic return instead of fighting the user's choice.

## OpenCode-specific behavior

- Keep the same OpenCode session ID, conversation, agent, sandbox, task ownership
  and team role. Change only the selected provider/model/variant at a safe boundary
  through the V2 session API inside its existing private server.
- Remember the exact original provider/model/variant and active connection identity.
  On confirmed recovery, restore that selection at the next safe boundary. Never
  change the global model default or another session's selection.
- Select only enabled, tool-capable models with a verified subscription connection
  and account-matched available quota. Discover model IDs through the actual model
  catalog; do not hardcode a guessed model ID or treat OAuth alone as proof of
  included subscription usage. If no eligible model exists, keep the session paused.
- For a different subscription, reuse its previously selected model/variant when
  available; otherwise use the first eligible entry in the discovered catalog's
  existing order. This tie-break is a builder decision, not a claim that models
  have equivalent capability. Do not add an automatic paid-API candidate.
- Distinguish confirmed subscription exhaustion from transient 429s, authentication
  errors and model/network failures. Resume the unfinished request exactly once;
  do not replay completed tool calls or a consumed inbox item.
- Resolve the active connection through OpenCode's existing integration API and
  probe quota for that connection inside its private server. Persist session-local
  routing intent through `ctx.storage`; native CLI quota caches cannot establish
  an OpenCode connection's capacity. The plugin neither copies credentials between
  providers nor refreshes them itself. This replaces the initially planned host
  route exchange; it grants no cross-session or team authority.
- A manual model/variant/connection selection cancels the pending automatic return.
  A changed connection requires fresh matching evidence before any automatic switch.
- Same-session model switching does not need a transcript export/import or a team
  membership replacement. Do not apply the native-CLI replacement path to OpenCode.

Sources: [V2 plugin API](https://opencode.ai/v2/docs/build/plugins) and
[V2 session API](https://opencode.ai/v2/docs/api), checked during planning. These
APIs support model switching, not permission to use Claude subscription credentials.

## Builder decisions (not additional user preferences)

- Reuse existing Rust, SQLite/JSON persistence and shell launchers; no dependencies.
- Use raw quota values: `99.6` is not exhausted even though the display rounds to
  `100`. Scope Claude model limits to the selected model, not unrelated models.
- Preserve the existing display's 600-second freshness ceiling. New routing also
  requires matching execution-account evidence and valid future reset metadata
  for applicable windows. Failed or stale refreshes cannot grant capacity.
- Bound transferred visible history to the existing 100-message/256-KiB limits;
  preserve the latest actual user request separately if it fell outside that tail.
- Use the existing refresh cadence, with a bounded refresh when a known reset is
  due. No tight polling or per-redraw network calls.

## Acceptance

Prove Claude → Codex → original Claude model and the reverse through real public
entry points with fake CLIs/accounts, including transferred context and ownership.
Also prove OpenCode fallback and return retain the same session and restore the
exact original provider/model/variant without any API-credit candidate.
Prove waiting/recovery, stale and mismatched-account quotas, model-scoped limits,
busy/approval boundaries, failures during each transition, restart reconciliation,
manual cancellation and team epoch isolation. No real subscription is deliberately
exhausted for tests.
