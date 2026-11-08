# cockpit-usage

Owns usage facts and their aggregates (report 18): each provider account's plan windows from the adapters' quota readings, their state as they fill, and the alerts at the owner's thresholds.

Quota windows and alerts are #58, the cost archive #699; time with agent and the digest
(#700) are their own issue.

**Depends on:** `cockpit-provider-api` for the typed facts adapters build, and `cockpit-turso`
(rule `usage -> provider-api/database only` in `cockpit-workspace-rules/tests/crate_graph.rs`).
Provider-specific readers stay in the adapters: this crate sees a `QuotaReading` only.
**Used by:** the daemon, which runs its migrations and records each provider's readings at the
owner's `[usage]` interval (#58), and answers them to the owner's `torlek usage quota`; it also
archives the adapters' model replies here and answers `torlek usage cost` (#699).

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

## Cost archive (#699)

What each reply cost at the provider's published API prices (a plan is flat, so this is what
the same tokens would bill on API credits), per UTC day, provider, account and project. The
adapters read their harness's records into `ModelReply` facts; this crate prices and stores
them and never reads a provider's file format.

- **Exact.** Prices are integers in nano-dollars per token (dollars per million tokens times
  1000), so a reply's cost and every total of them add up exactly. `prices.rs` is the table,
  ported from the legacy cockpit's (cached 2026-10-06): cache reads, the five-minute and
  one-hour cache writes, fast mode at twice the rate, Haiku 5.5's long-prompt rate. A new
  model is a new row.
- **No guessed price.** A reply of a model with no row is archived with its tokens and no
  cost, counted in `unpriced_replies`; `price_unpriced` prices them when a later release
  knows the model.
- **Counted once.** A reply is one row per provider and reply key. A harness writes a reply
  once per content block and a resumed conversation replays its history, so a key seen again
  keeps the account, project and day of its first archiving and only raises its counts to
  the larger.
- **Restart-safe.** Each home's read cursors (`usage_cost_cursors`, per provider and home
  scope) move in the transaction that archives the replies they passed, so a restart reads on
  from where the last transaction ended. `forget_home` drops a removed home's.
- **UTC days.** `Day` is a UTC date, the day providers bill by, whatever the owner's time
  zone.
- **Project is attribution.** A reply whose project name is not a plain one is archived under
  `unknown`; only a count past a trillion tokens refuses a reply.
- **Limits.** The account of a reply is the account of the home it was read in (the owner's
  home is `default`; a session's own home is its launch's account). A session that runs
  unsandboxed writes to the owner's home and is attributed to `default`. Two accounts signed
  in to one unsandboxed harness cannot be told apart.
