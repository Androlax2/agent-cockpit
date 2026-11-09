use std::collections::BTreeMap;

use cockpit_provider_api::{ModelReply, ProviderName, ReplyScan, ReplyTokens, SourceCursor};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row, params};

use crate::error::{invalid, invalid_row};
use crate::prices::{MAX_TOKENS_PER_COUNT, cost_nanos};
use crate::{AccountName, Day, Error};

const MAX_PROJECT_BYTES: usize = 128;

/// What [`record_replies`] did with a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Recorded {
    /// Replies archived that were not before.
    pub added: usize,
    /// Replies already archived whose counts the scan raised: a harness writes a message once
    /// per content block, the later lines with the more complete counts.
    pub raised: usize,
    /// Replies refused for a count past a trillion tokens: the scan's records are the agent's
    /// to write, and no real reply is that large.
    pub rejected: usize,
    /// Replies added whose model has no known price: archived with tokens and no cost.
    pub unpriced: usize,
}

/// One day of one account's replies in one project, as the archive totals them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayCost {
    pub day: Day,
    pub provider: ProviderName,
    pub account: AccountName,
    pub project: String,
    pub replies: u64,
    /// Replies of a model without a known price: counted in `replies` and the tokens, absent
    /// from `cost_nanos`, so a total is never silently short.
    pub unpriced_replies: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Both cache lifetimes together.
    pub cache_write: u64,
    /// At the provider's API prices, in nano-dollars (a billionth of a dollar), exact.
    pub cost_nanos: u64,
}

/// Archives the replies of `scan`, read under `home_scope`'s records, to `account`, and moves
/// that scope's cursors past them in the same transaction: a reply is counted once, however
/// often the daemon restarts. `home_scope` names the folder the scan read (the owner's home, one
/// session's home), since two homes hold records of the same relative name. `project_of` names
/// the project of a reply from where its agent ran.
///
/// One row per provider and reply key: a reply a harness records again, as a resumed
/// conversation replays its history, keeps the account, project and day of its first
/// archiving and only ever raises its counts to the larger.
pub async fn record_replies(
    write: &WriteTransaction<'_>,
    provider: &ProviderName,
    account: &AccountName,
    home_scope: &str,
    scan: &ReplyScan,
    project_of: &(dyn Fn(Option<&str>) -> String + Sync),
) -> Result<Recorded, Error> {
    check_scope(home_scope)?;
    let mut recorded = Recorded::default();
    for reply in merged(&scan.replies) {
        // A project is attribution, not billing: a name that is not a plain one costs the reply
        // its project, never its place in the total.
        let project = attributed_project(project_of(reply.working_dir()));
        if exceeds_limit(reply.tokens()) {
            recorded.rejected += 1;
            continue;
        }
        match archived(write, provider, reply.key()).await? {
            None => {
                let cost = cost_nanos(reply.model(), reply.tokens());
                insert_reply(write, provider, account, &project, &reply, cost).await?;
                recorded.added += 1;
                recorded.unpriced += usize::from(cost.is_none());
            }
            Some(known) => {
                let raised = raise(known.tokens, reply.tokens());
                if raised != known.tokens {
                    update_counts(write, provider, reply.key(), &known.model, raised).await?;
                    recorded.raised += 1;
                }
            }
        }
    }
    for cursor in &scan.cursors {
        write
            .execute(
                "INSERT INTO usage_cost_cursors (provider, home_scope, source, read_to)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (provider, home_scope, source) DO UPDATE SET read_to = excluded.read_to",
                params![
                    provider.as_str(),
                    home_scope,
                    cursor.source(),
                    to_stored(cursor.offset())?
                ],
            )
            .await?;
    }
    Ok(recorded)
}

/// Where each record source of `home_scope` was counted to, for the next scan.
pub async fn cursors(
    connection: &Connection,
    provider: &ProviderName,
    home_scope: &str,
) -> Result<Vec<SourceCursor>, Error> {
    let mut rows = connection
        .query(
            "SELECT source, read_to FROM usage_cost_cursors
             WHERE provider = ?1 AND home_scope = ?2 ORDER BY source",
            params![provider.as_str(), home_scope],
        )
        .await?;
    let mut cursors = Vec::new();
    while let Some(row) = rows.next().await? {
        let read_to: i64 = row.get(1)?;
        cursors.push(
            SourceCursor::new(
                row.get::<String>(0)?,
                u64::try_from(read_to)
                    .map_err(|_| invalid_row(format!("cursor offset {read_to}")))?,
            )
            .map_err(|error| invalid_row(format!("cursor: {error}")))?,
        );
    }
    Ok(cursors)
}

/// Drops the cursors of a home that is gone, once its last scan is recorded. Its replies stay.
pub async fn forget_home(
    write: &WriteTransaction<'_>,
    provider: &ProviderName,
    home_scope: &str,
) -> Result<(), Error> {
    write
        .execute(
            "DELETE FROM usage_cost_cursors WHERE provider = ?1 AND home_scope = ?2",
            params![provider.as_str(), home_scope],
        )
        .await?;
    Ok(())
}

/// Prices the archived replies that have no cost yet, for a price table that has grown since
/// they were archived. Returns how many it priced.
pub async fn price_unpriced(write: &WriteTransaction<'_>) -> Result<usize, Error> {
    let mut rows = write
        .query(
            "SELECT provider, reply_key, model, input_tokens, output_tokens, cache_read_tokens,
                cache_write_5m_tokens, cache_write_1h_tokens, is_fast
             FROM usage_cost_replies WHERE cost_nanos IS NULL",
            (),
        )
        .await?;
    let mut priced = Vec::new();
    while let Some(row) = rows.next().await? {
        let model: String = row.get(2)?;
        let tokens = tokens_from(&row, 3)?;
        if let Some(cost) = cost_nanos(&model, tokens) {
            priced.push((row.get::<String>(0)?, row.get::<String>(1)?, cost));
        }
    }
    for (provider, key, cost) in &priced {
        write
            .execute(
                "UPDATE usage_cost_replies SET cost_nanos = ?3 WHERE provider = ?1 AND reply_key = ?2",
                params![provider.as_str(), key.as_str(), to_stored(*cost)?],
            )
            .await?;
    }
    Ok(priced.len())
}

/// The archive's totals for each day from `first` to `last`, both included, by provider,
/// account and project, oldest day first.
pub async fn costs(connection: &Connection, first: Day, last: Day) -> Result<Vec<DayCost>, Error> {
    let mut rows = connection
        .query(
            "SELECT day, provider, account, project, COUNT(*),
                SUM(CASE WHEN cost_nanos IS NULL THEN 1 ELSE 0 END),
                SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                SUM(cache_write_5m_tokens + cache_write_1h_tokens), SUM(cost_nanos)
             FROM usage_cost_replies WHERE day >= ?1 AND day <= ?2
             GROUP BY day, provider, account, project
             ORDER BY day, provider, account, project",
            params![first.to_string(), last.to_string()],
        )
        .await?;
    let mut totals = Vec::new();
    while let Some(row) = rows.next().await? {
        totals.push(day_cost_from(&row)?);
    }
    Ok(totals)
}

/// What each project's replies on `provider`'s `account` from `since` on (Unix seconds,
/// included) cost at the provider's API prices, in nano-dollars, by project. A reply of a model
/// without a known price adds nothing until [`price_unpriced`] prices it. Usage-aware pacing
/// (#412) estimates a project's part of a plan window from these.
pub async fn project_costs_since(
    connection: &Connection,
    provider: &ProviderName,
    account: &AccountName,
    since: i64,
) -> Result<BTreeMap<String, u64>, Error> {
    // The day bound lets the day index narrow the scan; `at` decides.
    let mut rows = connection
        .query(
            "SELECT project, SUM(cost_nanos) FROM usage_cost_replies
             WHERE provider = ?1 AND account = ?2 AND day >= ?3 AND at >= ?4
             GROUP BY project",
            params![
                provider.as_str(),
                account.as_str(),
                Day::containing(since).to_string(),
                since
            ],
        )
        .await?;
    let mut costs = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        let project: String = row.get(0)?;
        let nanos: Option<i64> = row.get(1)?;
        let nanos = u64::try_from(nanos.unwrap_or(0))
            .map_err(|_| invalid_row(format!("a negative cost for project {project}")))?;
        costs.insert(project, nanos);
    }
    Ok(costs)
}

fn day_cost_from(row: &Row) -> Result<DayCost, Error> {
    let count = |position: usize| -> Result<u64, Error> {
        let value: Option<i64> = row.get(position)?;
        u64::try_from(value.unwrap_or(0))
            .map_err(|_| invalid_row(format!("a negative total in column {position}")))
    };
    Ok(DayCost {
        day: Day::parse(&row.get::<String>(0)?)
            .map_err(|error| invalid_row(format!("day: {error}")))?,
        provider: ProviderName::new(row.get::<String>(1)?)
            .map_err(|error| invalid_row(format!("provider: {error}")))?,
        account: AccountName::new(row.get::<String>(2)?)
            .map_err(|error| invalid_row(format!("account: {error}")))?,
        project: row.get(3)?,
        replies: count(4)?,
        unpriced_replies: count(5)?,
        input: count(6)?,
        output: count(7)?,
        cache_read: count(8)?,
        cache_write: count(9)?,
        cost_nanos: count(10)?,
    })
}

/// A scan's replies with those of one key joined into one carrying the larger counts, in key
/// order.
fn merged(replies: &[ModelReply]) -> Vec<ModelReply> {
    let mut by_key: BTreeMap<&str, ModelReply> = BTreeMap::new();
    for reply in replies {
        match by_key.get_mut(reply.key()) {
            None => {
                by_key.insert(reply.key(), reply.clone());
            }
            Some(first) => {
                let tokens = raise(first.tokens(), reply.tokens());
                *first = ModelReply::new(
                    first.key(),
                    first.at(),
                    first.model(),
                    first.working_dir().map(str::to_owned),
                    tokens,
                )
                .expect("the first reply's fields were valid");
            }
        }
    }
    by_key.into_values().collect()
}

fn raise(known: ReplyTokens, seen: ReplyTokens) -> ReplyTokens {
    ReplyTokens {
        input: known.input.max(seen.input),
        output: known.output.max(seen.output),
        cache_read: known.cache_read.max(seen.cache_read),
        cache_write_5m: known.cache_write_5m.max(seen.cache_write_5m),
        cache_write_1h: known.cache_write_1h.max(seen.cache_write_1h),
        is_fast: known.is_fast || seen.is_fast,
    }
}

fn exceeds_limit(tokens: ReplyTokens) -> bool {
    [
        tokens.input,
        tokens.output,
        tokens.cache_read,
        tokens.cache_write_5m,
        tokens.cache_write_1h,
    ]
    .iter()
    .any(|count| *count > MAX_TOKENS_PER_COUNT)
}

const UNKNOWN_PROJECT: &str = "unknown";

/// The project a record is archived under: a name that is not a plain one is attribution lost,
/// never a record refused.
pub(crate) fn attributed_project(named: String) -> String {
    let is_plain = !named.is_empty()
        && named.len() <= MAX_PROJECT_BYTES
        && !named.chars().any(char::is_control);
    if is_plain {
        named
    } else {
        UNKNOWN_PROJECT.to_owned()
    }
}

fn check_scope(home_scope: &str) -> Result<(), Error> {
    if home_scope.is_empty() || home_scope.chars().any(char::is_control) {
        return Err(invalid(
            "home scope",
            "a home's name is not empty and holds no control character",
        ));
    }
    Ok(())
}

struct Archived {
    model: String,
    tokens: ReplyTokens,
}

async fn archived(
    connection: &Connection,
    provider: &ProviderName,
    key: &str,
) -> Result<Option<Archived>, Error> {
    let mut rows = connection
        .query(
            "SELECT model, input_tokens, output_tokens, cache_read_tokens,
                cache_write_5m_tokens, cache_write_1h_tokens, is_fast
             FROM usage_cost_replies WHERE provider = ?1 AND reply_key = ?2",
            params![provider.as_str(), key],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Archived {
        model: row.get(0)?,
        tokens: tokens_from(&row, 1)?,
    }))
}

/// The six token columns of a row from `first` on: five counts, then the fast flag.
fn tokens_from(row: &Row, first: usize) -> Result<ReplyTokens, Error> {
    let count = |position: usize| -> Result<u64, Error> {
        let value: i64 = row.get(position)?;
        u64::try_from(value).map_err(|_| invalid_row(format!("a negative token count {value}")))
    };
    Ok(ReplyTokens {
        input: count(first)?,
        output: count(first + 1)?,
        cache_read: count(first + 2)?,
        cache_write_5m: count(first + 3)?,
        cache_write_1h: count(first + 4)?,
        is_fast: row.get::<i64>(first + 5)? == 1,
    })
}

async fn insert_reply(
    write: &WriteTransaction<'_>,
    provider: &ProviderName,
    account: &AccountName,
    project: &str,
    reply: &ModelReply,
    cost: Option<u64>,
) -> Result<(), Error> {
    let tokens = reply.tokens();
    write
        .execute(
            "INSERT INTO usage_cost_replies (provider, reply_key, account, project, day, at, model,
                input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens,
                cache_write_1h_tokens, is_fast, cost_nanos)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                provider.as_str(),
                reply.key(),
                account.as_str(),
                project,
                Day::containing(reply.at()).to_string(),
                reply.at(),
                reply.model(),
                to_stored(tokens.input)?,
                to_stored(tokens.output)?,
                to_stored(tokens.cache_read)?,
                to_stored(tokens.cache_write_5m)?,
                to_stored(tokens.cache_write_1h)?,
                i64::from(tokens.is_fast),
                cost.map(to_stored).transpose()?,
            ],
        )
        .await?;
    Ok(())
}

async fn update_counts(
    write: &WriteTransaction<'_>,
    provider: &ProviderName,
    key: &str,
    model: &str,
    tokens: ReplyTokens,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE usage_cost_replies SET input_tokens = ?3, output_tokens = ?4,
                cache_read_tokens = ?5, cache_write_5m_tokens = ?6, cache_write_1h_tokens = ?7,
                is_fast = ?8, cost_nanos = ?9
             WHERE provider = ?1 AND reply_key = ?2",
            params![
                provider.as_str(),
                key,
                to_stored(tokens.input)?,
                to_stored(tokens.output)?,
                to_stored(tokens.cache_read)?,
                to_stored(tokens.cache_write_5m)?,
                to_stored(tokens.cache_write_1h)?,
                i64::from(tokens.is_fast),
                cost_nanos(model, tokens).map(to_stored).transpose()?,
            ],
        )
        .await?;
    Ok(())
}

fn to_stored(value: u64) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| invalid("count", format!("{value} does not fit the archive")))
}
