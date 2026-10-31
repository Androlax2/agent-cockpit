# 18. Usage, quota and accounts

Status: proposed; account routing simplification is a Theo decision (report 15)

## Decision

A `cockpit-usage` capability owns plan windows, quota alerts, cost at API prices, time spent
per project, provider incident status and the daily digest. Provider adapters supply the raw
facts; `cockpit-usage` stores and aggregates; clients render.

## What it keeps from today

- Plan usage windows (5 hours, 7 days, per model) with 80 % / 95 % alerts; Codex quota with
  its own sign-in; refresh and cache intervals as configuration, not constants in scripts.
- Cost archive: every reply with model, tokens and price; files and databases scanned
  incrementally (Claude transcripts, OpenCode database) by a daemon worker, not by the TUI.
- Time with agent per project: gaps under 5 minutes joined; overlapping sessions merged for
  the global figure, never summed.
- Billing evidence records (credits vs subscription per session) from the adapters.
- Provider status pages polled at a bounded interval; incidents shown as warnings.
- Daily digest: yesterday/today, usage, waiting sessions, PRs, Chief section; first open of the
  day in the TUI, also on the phone.

## Accounts and routing (change)

Today's subscription fallback routes sessions between accounts with gates inside hooks,
generation tokens and native evidence collection. Target:

- An **account** is a credential reference with a provider, kind (subscription, API credits)
  and observed quota state.
- Routing happens at **launch** and at a **quota-stop restart** only: pick the account with
  capacity, resume the same conversation under it, record the switch. No gates inside a turn.
- `cockpit-sessions` records which account a session runs on; `cockpit-usage` attributes
  cost to it; `cockpit-credentials` injects the secret as a proxy placeholder as today.

Cost of the change: a session that exhausts its quota mid-turn stops and is restarted on
another account instead of being handed over live. Benefit: the routing store, phase machine
and hook gates disappear.

## Rules

- Quota is admission input (report 10): available CPU does not mean another model request
  may run.
- Usage data never grants or denies authority; it informs scheduling and the owner.
- All provider-specific readers live in the adapters; `cockpit-usage` sees typed facts only.

## Validate

- Cost figures match the provider's own billing for a sample week.
- Quota-stop restart resumes the same conversation on the second account within one minute.

## Sources

- Current implementation: `apps/crates/cockpit-core/src/{usage,costs,billing,status,subscription_fallback}.rs`
