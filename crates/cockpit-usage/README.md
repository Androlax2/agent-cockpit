# cockpit-usage

Owns usage facts and their aggregates (report 18): each provider account's plan windows from the adapters' quota readings, their state as they fill, and the alerts at the owner's thresholds.

Quota windows and alerts are #58, the cost archive #699, time with the agent and the daily
digest data #700.

**Depends on:** `cockpit-provider-api` for the typed facts adapters build, and `cockpit-turso`
(rule `usage -> provider-api/database only` in `cockpit-workspace-rules/tests/crate_graph.rs`).
Provider-specific readers stay in the adapters: this crate sees a `QuotaReading` only.
**Used by:** the daemon, which runs its migrations and records each provider's readings at the
owner's `[usage]` interval (#58), and answers them to the owner's `torlek usage quota`; it also
archives the adapters' model replies here and answers `torlek usage cost` (#699); it records
the sessions' turns and the owner's approvals here and answers `torlek usage digest` (#700).

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

## Time with the agent and the daily digest (#700)

What the Status panel (#85) and the digest (#87) show, as data: a client renders a
`DayDigest` without asking anything else. Time comes from turn records the caller hands in as
each turn ends (`record_turn`), never from polling; approvals (`record_approval`) and provider
incidents (`record_incident`) are recorded as they happen. Each is idempotent under its key,
so a replay after a restart counts nothing twice (a turn recorded again keeps the later end;
an incident keeps its earliest start and first resolution). A time before the year 2000 or past
9999, and a turn over `MAX_TURN_SECONDS` (a week), are refused: a wrong clock must not fill
every later day with time. Which events feed them is the daemon's (see below).

- **Stretches.** A project's turns, with a gap under five minutes (`JOIN_GAP_SECONDS`) joined
  and the gap counted as time; a gap of exactly five minutes is not joined.
- **Overlap is counted once.** Overlapping sessions of one project are one stretch. The day's
  own figure joins every project's turns, so two projects worked at once are not summed: the
  projects' figures can add up to more than the day's.
- **UTC days**, as the cost archive: a stretch over midnight is split, each day has the
  seconds that fell on it. A turn is counted (`turns`, `sessions`) on the day it began.
- **Per day, always.** `digests(first, last)` answers every day of the range, up to a leap
  year, a quiet day as zeros: sessions that began a turn, turns, approvals, time, replies and
  cost (the cost archive's, with the replies of unpriced models counted apart), each project
  with anything on the day, and the incidents under way on it.
- **Incidents** are listed on each day they were under way, or began on. One never recorded
  as resolved stays under way, so the owner sees a lost resolution rather than none.
- **Open turns.** A caller that sees a turn begin and, later, end (the daemon) uses
  `turn_began` and `turn_ended` instead of `record_turn`: the open turn is stored
  (`usage_open_turns`, one per session, `open_turns` lists them), so a restart between the two
  loses nothing, and `turn_ended` records the finished turn and drops the open one in the same
  transaction. A turn begun twice keeps its first start; ended with nothing open, it records
  nothing and says so; ended before it began, it lasts no time. A turn the archive refuses is
  dropped with its open row, so one bad turn is not retried forever.
- **Limits.** A turn is recorded when it ends: one still running, or lost to a crash, has no
  time yet. A turn's project is the project name the caller gives, and a name that is not a
  plain one is `unknown`, as for cost.

### What feeds it, in the daemon

- **Turns** (`cockpit-daemon/src/turns.rs`): after each commit a pass compares the sessions'
  latest agent reports with the open turns. A session that runs and whose agent's latest report
  for its current launch says `working` opens a turn at that report's time; any other report, or
  the session no longer running, ends it (at the report's time while the session runs, at the
  session's last change when it stopped, restarted, failed or ended). A pass that has nothing to
  record takes no write, so it does not wake itself. The project is the checkout's, resolved as
  for cost; a working folder in no project is `unknown`.
- **Approvals** (`approvals.rs`): the owner's approval of a run is counted in the approval's own
  transaction, keyed `run-<request>`, under the request's project. A declined request is not an
  approval, and is not counted.
- **Incidents** have no source yet: `record_incident` is the shape a provider status feed will
  fill.
- **Limits.** A turn that begins and ends between two passes is missed, and a turn still
  running shows no time until it ends, so today's figure lags a running agent.
