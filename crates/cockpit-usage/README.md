# cockpit-usage

Owns usage facts and their aggregates (report 18): each provider account's plan windows from the adapters' quota readings, their state as they fill, and the alerts at the owner's thresholds.

Quota windows and alerts are #58; the cost archive (#699) and time with agent and the digest
(#700) are their own issues.

**Depends on:** `cockpit-provider-api` for the typed facts adapters build, and `cockpit-turso`
(rule `usage -> provider-api/database only` in `cockpit-workspace-rules/tests/crate_graph.rs`).
Provider-specific readers stay in the adapters: this crate sees a `QuotaReading` only.
**Used by:** the daemon, which runs its migrations and records each provider's readings at the
owner's `[usage]` interval (#58), and answers them to the owner's `torlek usage quota`.

## Accounts and readings

Windows are kept per provider and `AccountName`: an owner may hold two accounts on one
provider's subscription. The name is the owner's (account routing, #59, gives them), never
the provider's own account identifier.

`record_reading` replaces an account's windows with a new reading and refuses one older than
the reading recorded, so a late refresh never overwrites a newer one. `record_failure` keeps
the last reading and makes it stale.

## Window states

`quotas` judges each window at the time asked, under a `QuotaPolicy` (alert thresholds, how
long a reading stays current):

| State | When |
| --- | --- |
| `stale` | no current reading: older than the policy allows, a failed refresh after it, or the window's reset already past |
| `blocked` | the provider refuses ordinary use on the account, whatever the percentages say |
| `exhausted` | used at or past 100 % |
| `low` | used at or past the lowest alert threshold |
| `available` | otherwise |

Only a current reading can say a window has room: an old one, an error or an elapsed reset
never implies new capacity.

## Alerts

The defaults are report 18's 80 % and 95 %. A threshold fires once per window instance (reset
times within a minute of each other are the same instance), and the fired thresholds are
stored with the reading, so a restarted daemon does not repeat them. A reading that jumps past
several thresholds fires the highest only, and a threshold the owner adds below one already
fired stays silent. A window a reading leaves out, or reports with its reset already past,
keeps its fired thresholds. A window without a reset time re-arms a threshold once a reading
shows it below. `record_reading` returns the alerts that fired; delivering them is
the caller's: the daemon publishes them on the change feed, in the same transaction, for
`torlek-notify`.
