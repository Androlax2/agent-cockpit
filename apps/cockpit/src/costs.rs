//! What Claude Code and opencode replies would cost at API prices (the plan is flat): every reply of the
//! transcripts archived in usage.sqlite, which keeps them after Claude Code deletes old
//! transcripts. Same schema and keys as the Python column, so the archive carries over. opencode's
//! replies join them from its database (archive_opencode), keyed "opencode:<message id>".

use crate::chief::store::{self as chief_store, ChiefDigest};
use crate::chief::view as chief_view;
use crate::paths;
use crate::reviews;
use crate::sessions;
use crate::usage;
use crate::util::{self, money, now, parse_time};
use chrono::{Datelike, Duration, Local, NaiveDate};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const RESCAN_S: f64 = 60.0;
const DIGEST_MIN_REPLIES: i64 = 3;
const FAST_MODE_MULTIPLIER: f64 = 2.0;
/// $ per million tokens: input, output, cache read, cache write (5 min TTL), cache write (1 h TTL).
/// From the Claude API reference bundled with Claude Code (claude-api skill, prices cached
/// 2026-10-06), as in the Python column.
const MODEL_PRICES: &[(&str, [f64; 5])] = &[
    ("claude-fable-5-1", [10.0, 50.0, 0.25, 12.5, 20.0]), ("claude-mythos-5-1", [10.0, 50.0, 0.25, 12.5, 20.0]),
    ("claude-fable-5", [10.0, 50.0, 1.0, 12.5, 20.0]), ("claude-mythos-5", [10.0, 50.0, 1.0, 12.5, 20.0]),
    ("claude-opus-5-5", [4.0, 20.0, 0.20, 5.0, 8.0]),
    ("claude-opus-5", [5.0, 25.0, 0.5, 6.25, 10.0]),
    ("claude-opus-4-8", [5.0, 25.0, 0.5, 6.25, 10.0]), ("claude-opus-4-7", [5.0, 25.0, 0.5, 6.25, 10.0]), ("claude-opus-4-6", [5.0, 25.0, 0.5, 6.25, 10.0]),
    // https://www.anthropic.com/news/claude-opus-4-5; cache multipliers match the other Opus 4 models.
    ("claude-opus-4-5", [5.0, 25.0, 0.5, 6.25, 10.0]),
    ("claude-sonnet-5-5", [2.0, 10.0, 0.20, 2.5, 4.0]), ("claude-sonnet-5", [2.0, 10.0, 0.20, 2.5, 4.0]),
    ("claude-sonnet-4-6", [3.0, 15.0, 0.3, 3.75, 6.0]),
    ("claude-haiku-5-5", [0.10, 0.50, 0.01, 0.125, 0.2]),
    ("claude-haiku-4-5", [1.0, 5.0, 0.1, 1.25, 2.0]),
    // Standard API rates checked 2026-10-20: https://developers.openai.com/api/docs/pricing
    ("gpt-5.3-codex", [1.75, 14.0, 0.175, 0.0, 0.0]),
    ("gpt-5.4", [2.5, 15.0, 0.25, 0.0, 0.0]),
    ("gpt-5.5", [5.0, 30.0, 0.5, 0.0, 0.0]),
    ("gpt-6-astra", [10.0, 50.0, 1.0, 12.5, 12.5]),
    // Fast mode, checked 2026-10-25: https://developers.openai.com/api/docs/pricing?latest-pricing=fast
    ("gpt-6-astra-fast", [20.0, 100.0, 2.0, 25.0, 25.0]),
    ("gpt-6.1-sol", [2.0, 10.0, 0.1, 2.5, 2.5]),
    // https://ai.google.dev/gemini-api/docs/pricing (2026-10-20); cache storage is time-based,
    // not a per-reply token charge, and cannot be recovered from these transcripts.
    ("gemini-3.1-pro-preview", [2.0, 12.0, 0.2, 0.0, 0.0]),
];
/// Haiku 5.5 above 100K prompt tokens.
const HAIKU_5_5_LONG_PROMPT: (i64, [f64; 5]) = (100_000, [0.50, 2.50, 0.05, 0.625, 1.0]);
const SCHEMA: &str = "
create table if not exists replies (
    key text primary key, day text not null, at text not null, model text not null,
    project text not null, session text not null,
    input integer not null, output integer not null, cache_read integer not null,
    cache_write_5m integer not null, cache_write_1h integer not null,
    speed text not null, cost real
);
create index if not exists replies_day on replies (day);
create table if not exists files (path text primary key, mtime real not null, size integer not null);
create table if not exists databases (path text primary key, read_until integer not null);
create table if not exists reply_billing (
    key text primary key, billing text not null check (billing in ('api', 'subscription', 'unknown'))
);
create table if not exists billing_files (path text primary key, mtime real not null, size integer not null);
";
/// archive_opencode reads again the messages updated this long (ms) before the newest it read: a
/// sandbox can commit a message stamped earlier than one another sandbox committed first.
const OPENCODE_REREAD_MS: i64 = 60_000;

/// The price row of a model id (date-suffixed ids included), or None for an unknown model.
fn model_prices(model: &str) -> Option<[f64; 5]> {
    MODEL_PRICES.iter()
        .filter(|(known, _)| model == *known || model.strip_prefix(&format!("{known}-")).is_some_and(|suffix|
            (suffix.len() == 8 || suffix.len() == 10) && suffix.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-')))
        .max_by_key(|(known, _)| known.len())
        .map(|(_, prices)| *prices)
}

fn tokens(usage: &Value, key: &str) -> i64 {
    usage.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// Cache writes by TTL: (5 min, 1 h).
fn cache_writes(usage: &Value) -> (i64, i64) {
    let written = tokens(usage, "cache_creation_input_tokens");
    let by_ttl = usage.get("cache_creation");
    let written_1h = by_ttl.and_then(|t| t.get("ephemeral_1h_input_tokens")).and_then(Value::as_i64).unwrap_or(0);
    let written_5m = match by_ttl.and_then(|t| t.get("ephemeral_5m_input_tokens")) {
        Some(value) => value.as_i64().unwrap_or(0),
        None => written - written_1h,
    };
    (written_5m, written_1h)
}

/// What one reply would cost at API prices, or None when the model has no known price.
pub fn reply_cost(model: &str, usage: &Value) -> Option<f64> {
    let mut prices = model_prices(model)?;
    let (written_5m, written_1h) = cache_writes(usage);
    let prompt = tokens(usage, "input_tokens") + tokens(usage, "cache_read_input_tokens") + tokens(usage, "cache_creation_input_tokens");
    if model.starts_with("claude-haiku-5-5") && prompt > HAIKU_5_5_LONG_PROMPT.0 {
        prices = HAIKU_5_5_LONG_PROMPT.1;
    }
    if model.starts_with("gpt-") && !model.starts_with("gpt-5.3-codex") && prompt > 272_000 {
        prices = [prices[0] * 2.0, prices[1] * 1.5, prices[2] * 2.0, prices[3] * 2.0, prices[4] * 2.0];
    }
    if model.starts_with("gemini-3.1-pro-preview") && prompt > 200_000 {
        prices = [4.0, 18.0, 0.4, 0.0, 0.0];
    }
    let [input, output, read, write_5m, write_1h] = prices;
    let cost = (tokens(usage, "input_tokens") as f64 * input + tokens(usage, "output_tokens") as f64 * output
        + tokens(usage, "cache_read_input_tokens") as f64 * read + written_5m as f64 * write_5m + written_1h as f64 * write_1h) / 1e6;
    Some(if usage.get("speed").and_then(Value::as_str) == Some("fast") { cost * FAST_MODE_MULTIPLIER } else { cost })
}

pub struct Reply {
    key: String, day: String, at: String, model: String, project: String, session: String,
    input: i64, output: i64, cache_read: i64, write_5m: i64, write_1h: i64, speed: String, cost: Option<f64>,
    /// Where the session ran: archive_transcripts names the project from it as the cards do.
    cwd: String,
}

/// The archive row of one transcript line (with its newline, as the key hashes it), or None.
pub fn reply_row(line: &[u8]) -> Option<Reply> {
    if !line.windows(7).any(|window| window == b"\"usage\"") {
        return None;
    }
    let record: Value = serde_json::from_slice(line).ok()?;
    let message = record.get("message")?;
    let usage = message.get("usage").filter(|usage| usage.is_object())?;
    let model = message.get("model").and_then(Value::as_str).unwrap_or("");
    let timestamp = record.get("timestamp").and_then(Value::as_str).unwrap_or("");
    if timestamp.is_empty() || model.is_empty() || model.starts_with('<') {
        return None;
    }
    let day = parse_time(timestamp)?.with_timezone(&Local).date_naive().to_string();
    let message_id = message.get("id").and_then(Value::as_str).unwrap_or("");
    let request_id = record.get("requestId").and_then(Value::as_str).unwrap_or("");
    let key = if !message_id.is_empty() || !request_id.is_empty() {
        format!("{message_id}:{request_id}")
    } else {
        format!("line:{}", sha1_smol::Sha1::from(line).digest())
    };
    let cwd = record.get("cwd").and_then(Value::as_str).unwrap_or("");
    let project = Path::new(cwd).file_name().map(|name| name.to_string_lossy().into_owned()).filter(|name| !name.is_empty()).unwrap_or("?".into());
    let (write_5m, write_1h) = cache_writes(usage);
    Some(Reply {
        key, day, at: timestamp.into(), model: model.into(), project, cwd: cwd.into(),
        session: record.get("sessionId").and_then(Value::as_str).unwrap_or("").into(),
        input: tokens(usage, "input_tokens"), output: tokens(usage, "output_tokens"), cache_read: tokens(usage, "cache_read_input_tokens"),
        write_5m, write_1h, speed: usage.get("speed").and_then(Value::as_str).unwrap_or("standard").into(), cost: reply_cost(model, usage),
    })
}

pub fn open_archive(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let archive = Connection::open(path)?;
    archive.busy_timeout(std::time::Duration::from_secs(10))?;
    archive.execute_batch(SCHEMA)?;
    // Version 2 names a reply's project as the cards do (the repository, a parallel clone's
    // included), not after the folder Claude ran in: every transcript is read once more so the
    // rows already archived get it too.
    if archive.query_row("pragma user_version", [], |row| row.get::<_, i64>(0))? < 2 {
        archive.execute_batch("delete from files; pragma user_version = 2;")?;
    }
    // Version 3 prices opencode's local-model replies at zero, which only its database tells
    // (the provider is not archived): it is read once more from the start.
    if archive.query_row("pragma user_version", [], |row| row.get::<_, i64>(0))? < 3 {
        archive.execute_batch("delete from databases; pragma user_version = 3;")?;
    }
    Ok(archive)
}

fn transcript_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            transcript_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            found.push(path);
        }
    }
}

/// Copies every reply of the transcripts into the archive; files unchanged since the last pass
/// are skipped. Returns how many replies were new.
pub fn archive_transcripts(archive: &mut Connection, transcripts: &Path) -> rusqlite::Result<usize> {
    let mut known: HashMap<String, (f64, i64)> = HashMap::new();
    {
        let mut statement = archive.prepare("select path, mtime, size from files")?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, (row.get::<_, f64>(1)?, row.get::<_, i64>(2)?))))?;
        for row in rows {
            let (path, stamp) = row?;
            known.insert(path, stamp);
        }
    }
    let mut files = Vec::new();
    transcript_files(transcripts, &mut files);
    let mut added = 0;
    let mut projects: HashMap<String, String> = HashMap::new();
    for path in files {
        let Ok(metadata) = fs::metadata(&path) else { continue };
        let mtime = metadata.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let size = metadata.len() as i64;
        let key = path.to_string_lossy().into_owned();
        if known.get(&key) == Some(&(mtime, size)) {
            continue;
        }
        let Ok(file) = util::open_untrusted(&path) else { continue };
        let mut reader = BufReader::new(file);
        let mut rows = Vec::new();
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
            if let Some(mut row) = reply_row(&line) {
                if !row.cwd.is_empty() {
                    row.project = projects.entry(row.cwd.clone()).or_insert_with(|| util::project_name(&util::repository_root(&row.cwd))).clone();
                }
                rows.push(row);
            }
            line.clear();
        }
        let transaction = archive.transaction()?;
        added += insert_replies(&transaction, &rows)?;
        transaction.execute("insert or replace into files values (?,?,?)", params![key, mtime, size])?;
        transaction.commit()?;
    }
    Ok(added)
}

/// New replies added, a known one's project corrected and its missing price filled; returns how
/// many rows changed.
fn insert_replies(transaction: &Transaction, rows: &[Reply]) -> rusqlite::Result<usize> {
    let mut insert = transaction.prepare("insert into replies values (?,?,?,?,?,?,?,?,?,?,?,?,?) on conflict(key) do update
        set project = excluded.project, cost = coalesce(replies.cost, excluded.cost)
        where project != excluded.project or (replies.cost is null and excluded.cost is not null)")?;
    let mut changed = 0;
    for row in rows {
        changed += insert.execute(params![row.key, row.day, row.at, row.model, row.project, row.session, row.input, row.output,
                                          row.cache_read, row.write_5m, row.write_1h, row.speed, row.cost])?;
    }
    Ok(changed)
}

/// One assistant message of opencode 2's database (session_message.data) as an archive row: a reply
/// that completed, with tokens, or None. Priced like a Claude reply from its tokens (opencode's own
/// cost is 0 on a subscription); a model without a known price keeps opencode's cost when it has one.
/// A model of the "local" provider runs on this machine: it costs nothing at any API price.
pub fn opencode_reply_row(id: &str, session: &str, data: &str, cwd: &str) -> Option<Reply> {
    let message: Value = serde_json::from_str(data).ok()?;
    if message.pointer("/time/completed").is_none() {
        return None;
    }
    let model = message.pointer("/model/id").and_then(Value::as_str).filter(|model| !model.is_empty())?;
    let created = chrono::DateTime::from_timestamp_millis(message.pointer("/time/created").and_then(Value::as_i64)?)?;
    let count = |pointer: &str| message.pointer(pointer).and_then(Value::as_i64).unwrap_or(0);
    let (input, output, cache_read, cache_write) =
        (count("/tokens/input"), count("/tokens/output") + count("/tokens/reasoning"), count("/tokens/cache/read"), count("/tokens/cache/write"));
    if input + output + cache_read + cache_write == 0 {
        return None;
    }
    let usage = serde_json::json!({"input_tokens": input, "output_tokens": output, "cache_read_input_tokens": cache_read,
                                   "cache_creation_input_tokens": cache_write});
    let cost = if message.pointer("/model/providerID").and_then(Value::as_str) == Some("local") {
        Some(0.0)
    } else {
        reply_cost(model, &usage).or_else(|| message.get("cost").and_then(Value::as_f64).filter(|cost| *cost > 0.0))
    };
    let project = Path::new(cwd).file_name().map(|name| name.to_string_lossy().into_owned()).filter(|name| !name.is_empty()).unwrap_or("?".into());
    Some(Reply {
        key: format!("opencode:{id}"), day: created.with_timezone(&Local).date_naive().to_string(),
        at: created.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), model: model.into(), project, session: session.into(),
        input, output, cache_read, write_5m: cache_write, write_1h: 0, speed: "standard".into(), cost, cwd: cwd.into(),
    })
}

/// Copies opencode's replies into the archive: the assistant messages of its database (opencode 2's
/// session_message, the folder from session_v2) updated since the last pass, read-only (its
/// sandboxes write it), the known ones only corrected. Reading every message each minute parsed
/// over a gigabyte of JSON (todo d3cfe632). Nothing when opencode never ran. Returns how many rows
/// changed.
pub fn archive_opencode(archive: &mut Connection, database: &Path) -> rusqlite::Result<usize> {
    if !database.exists() {
        return Ok(0);
    }
    let key = database.to_string_lossy().into_owned();
    let read_until: Option<i64> = archive.query_row("select read_until from databases where path = ?", [&key], |row| row.get(0)).optional()?;
    let source = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(std::time::Duration::from_secs(1))?;
    let mut statement = source.prepare(
        "select message.id, message.session_id, message.data, session.directory, message.time_updated from session_message message
         join session_v2 session on session.id = message.session_id where message.type = 'assistant' and message.time_updated >= ?")?;
    let since = read_until.map_or(i64::MIN, |read_until| read_until - OPENCODE_REREAD_MS);
    let messages = statement.query_map([since], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                                                          row.get::<_, String>(3)?, row.get::<_, i64>(4)?)))?;
    let mut projects: HashMap<String, String> = HashMap::new();
    let mut rows = Vec::new();
    let mut newest = read_until;
    for message in messages {
        let (id, session, data, cwd, updated) = message?;
        newest = newest.max(Some(updated));
        let Some(mut row) = opencode_reply_row(&id, &session, &data, &cwd) else { continue };
        if !row.cwd.is_empty() {
            row.project = projects.entry(row.cwd.clone()).or_insert_with(|| util::project_name(&util::repository_root(&row.cwd))).clone();
        }
        rows.push(row);
    }
    let transaction = archive.transaction()?;
    let changed = insert_replies(&transaction, &rows)?;
    if let Some(newest) = newest {
        transaction.execute("insert or replace into databases values (?, ?)", params![key, newest])?;
    }
    transaction.commit()?;
    Ok(changed)
}

/// Price only previously unpriced replies from saved tokens: the source history may be gone.
pub fn backfill_costs(archive: &mut Connection) -> rusqlite::Result<usize> {
    let rows = {
        let mut statement = archive.prepare(
            "select key, model, input, output, cache_read, cache_write_5m, cache_write_1h, speed from replies where cost is null")?;
        statement.query_map([], |row| {
            let written_5m: i64 = row.get(5)?;
            let written_1h: i64 = row.get(6)?;
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, serde_json::json!({
                "input_tokens": row.get::<_, i64>(2)?, "output_tokens": row.get::<_, i64>(3)?,
                "cache_read_input_tokens": row.get::<_, i64>(4)?, "cache_creation_input_tokens": written_5m + written_1h,
                "cache_creation": {"ephemeral_5m_input_tokens": written_5m, "ephemeral_1h_input_tokens": written_1h},
                "speed": row.get::<_, String>(7)?,
            })))
        })?.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let transaction = archive.transaction()?;
    let mut changed = 0;
    {
        let mut update = transaction.prepare("update replies set cost = ? where key = ? and cost is null")?;
        for (key, model, usage) in rows {
            if let Some(cost) = reply_cost(&model, &usage) {
                changed += update.execute(params![cost, key])?;
            }
        }
    }
    transaction.commit()?;
    Ok(changed)
}

/// A gap between two replies of a session counts as time spent only under this: longer, the
/// session sat waiting (a night, a meeting), and that is not time with Claude.
const ACTIVE_GAP_S: f64 = 300.0;

/// Seconds spent with Claude per project since `first_day` (YYYY-MM-DD): the gaps between
/// consecutive replies of each session, each counted when under ACTIVE_GAP_S.
pub fn active_seconds(archive: &Connection, first_day: &str) -> rusqlite::Result<HashMap<String, f64>> {
    let mut statement = archive.prepare(
        "select project, sum(case when gap < ?2 then gap else 0 end) from (
             select project, (julianday(at) - julianday(lag(at) over (partition by session order by at))) * 86400.0 as gap
             from replies where day >= ?1
         ) group by project")?;
    let rows = statement.query_map(params![first_day, ACTIVE_GAP_S], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?.unwrap_or(0.0))))?;
    rows.collect()
}

/// Seconds with Claude since each of `since` (Unix times), all sessions together: each session's
/// active stretches (gaps under ACTIVE_GAP_S between its replies) merged where they overlap, so
/// three sessions working through the same hour count one hour.
pub fn time_with_claude(archive: &Connection, since: &[f64]) -> rusqlite::Result<Vec<f64>> {
    let Some(earliest) = since.iter().copied().reduce(f64::min) else { return Ok(Vec::new()) };
    let first_day = chrono::DateTime::from_timestamp(earliest as i64 - 86_400, 0).map(|time| time.date_naive().to_string()).unwrap_or_default();
    let mut statement = archive.prepare(
        "select (julianday(previous) - 2440587.5) * 86400.0, (julianday(at) - 2440587.5) * 86400.0 from (
             select at, lag(at) over (partition by session order by at) as previous from replies where day >= ?1
         ) where previous is not null and (julianday(at) - julianday(previous)) * 86400.0 < ?2")?;
    let mut stretches: Vec<(f64, f64)> = statement.query_map(params![first_day, ACTIVE_GAP_S], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    stretches.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in stretches {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    Ok(since.iter().map(|from| merged.iter().map(|(start, end)| (end - start.max(*from)).max(0.0)).sum()).collect())
}

/// What each session's priced replies on `day` would cost at API prices, by session id.
pub fn session_costs(archive: &Connection, day: &str) -> rusqlite::Result<HashMap<String, f64>> {
    let mut statement = archive.prepare("select session, sum(cost) from replies where day = ? and cost is not null group by session")?;
    statement.query_map([day], |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)))?.collect()
}

/// Cost per day, output tokens of replies without a known price per model, and the all-time cost.
pub fn archived_costs(archive: &Connection) -> rusqlite::Result<(HashMap<String, f64>, HashMap<String, i64>, f64)> {
    let mut days = HashMap::new();
    let mut statement = archive.prepare("select day, sum(cost) from replies where cost is not null group by day")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)))? {
        let (day, cost) = row?;
        days.insert(day, cost);
    }
    let mut unpriced = HashMap::new();
    let mut statement = archive.prepare("select model, sum(output) from replies where cost is null group by model")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
        let (model, output) = row?;
        unpriced.insert(model, output);
    }
    let all_time = days.values().sum();
    Ok((days, unpriced, all_time))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Billing {
    Api,
    Subscription,
    Unknown,
}

#[derive(Clone, Copy, Default)]
pub struct BillingTotals {
    pub api: Totals,
    pub subscription: Totals,
    pub unknown: Totals,
    pub recorded_replies: i64,
    pub unpriced_replies: i64,
}

/// Only exact reply evidence separates costs; historical or conflicting evidence stays unknown.
pub fn archived_billing_totals(archive: &Connection) -> rusqlite::Result<BillingTotals> {
    let (recorded_replies, unpriced_replies) = archive.query_row(
        "select count(*), count(*) - count(cost) from replies", [], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut days: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut statement = archive.prepare(
        "select coalesce(b.billing, 'unknown'), r.day, sum(r.cost) from replies r
         left join reply_billing b on b.key = r.key where r.cost is not null group by 1, 2")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, f64>(2)?)))? {
        let (billing, day, cost) = row?;
        days.entry(billing).or_default().insert(day, cost);
    }
    Ok(BillingTotals {
        api: totals_of(&days.remove("api").unwrap_or_default()),
        subscription: totals_of(&days.remove("subscription").unwrap_or_default()),
        unknown: totals_of(&days.remove("unknown").unwrap_or_default()),
        recorded_replies,
        unpriced_replies,
    })
}

#[derive(Default)]
struct State {
    days: HashMap<String, f64>,
    all_time: f64,
    billing: BillingTotals,
    /// Seconds spent with Claude per project (active_seconds): today, and all the archive holds.
    active_today: HashMap<String, f64>,
    active_all_time: HashMap<String, f64>,
    /// What each session's replies today would cost at API prices, by session id.
    session_today: HashMap<String, f64>,
    /// Seconds with Claude, all sessions together (time_with_claude): today, this week, this month.
    time_together: [f64; 3],
    error: String,
    /// The last failure to read opencode's database, logged once: it never holds Claude's totals back.
    opencode_error: String,
    scanned_at: f64,
    scanning: bool,
}

/// The archive brought up to date every minute in a thread, so the column never waits on it.
#[derive(Clone, Default)]
pub struct Costs {
    state: Arc<Mutex<State>>,
}

/// When today, this week (from Monday) and this month began, local time, as Unix times.
pub fn period_starts() -> [f64; 3] {
    let today = Local::now().date_naive();
    let midnight = |day: NaiveDate| day.and_hms_opt(0, 0, 0).and_then(|time| time.and_local_timezone(Local).earliest()).map(|time| time.timestamp() as f64).unwrap_or(0.0);
    [midnight(today), midnight(today - Duration::days(today.weekday().num_days_from_monday() as i64)), midnight(today.with_day(1).unwrap_or(today))]
}

/// The cost of today, this week (from Monday) and this month, from the cost per day.
pub fn totals_of(days: &HashMap<String, f64>) -> Totals {
    let today = Local::now().date_naive();
    let since = |first: NaiveDate| -> f64 {
        let first = first.to_string();
        days.iter().filter(|(day, _)| **day >= first).fold(0.0, |total, (_, cost)| total + cost)
    };
    Totals {
        today: since(today),
        week: since(today - Duration::days(today.weekday().num_days_from_monday() as i64)),
        month: since(today.with_day(1).unwrap_or(today)),
    }
}

#[derive(Clone, Copy, Default)]
pub struct Totals {
    pub today: f64,
    pub week: f64,
    pub month: f64,
}

impl Costs {
    pub fn tick(&self) {
        {
            let mut state = self.state.lock().unwrap();
            if state.scanning || now() - state.scanned_at < RESCAN_S {
                return;
            }
            state.scanning = true;
        }
        let shared = self.state.clone();
        std::thread::spawn(move || {
            let path = paths::usage_archive();
            let today = Local::now().date_naive().to_string();
            let starts = period_starts();
            let mut opencode_error = String::new();
            let mut billing_error = String::new();
            let result = open_archive(&path).and_then(|mut archive| {
                archive_transcripts(&mut archive, &paths::transcripts())?;
                if let Err(error) = archive_opencode(&mut archive, &paths::opencode_database()) {
                    opencode_error = error.to_string();
                }
                backfill_costs(&mut archive)?;
                if let Err(error) = crate::billing::import(&mut archive) {
                    billing_error = format!("billing metadata: {error}");
                }
                Ok((archived_costs(&archive)?, archived_billing_totals(&archive)?, active_seconds(&archive, &today)?, active_seconds(&archive, "0000-00-00")?,
                    time_with_claude(&archive, &starts)?, session_costs(&archive, &today)?))
            });
            let mut state = shared.lock().unwrap();
            if opencode_error != state.opencode_error && !opencode_error.is_empty() {
                util::log_event("ERROR", &format!("usage archive: opencode's {}: {opencode_error}", paths::opencode_database().display()));
            }
            state.opencode_error = opencode_error;
            match result {
                Ok(((days, _, all_time), billing, active_today, active_all_time, together, session_today)) => {
                    state.session_today = session_today;
                    state.time_together = [together[0], together[1], together[2]];
                    state.days = days;
                    state.all_time = all_time;
                    state.billing = billing;
                    state.active_today = active_today;
                    state.active_all_time = active_all_time;
                    state.error = billing_error;
                }
                Err(error) => {
                    let message = format!("usage archive: {error}");
                    if message != state.error {
                        util::log_event("ERROR", &format!("usage archive {}: {error}", path.display()));
                    }
                    state.error = message;
                }
            }
            state.scanned_at = now();
            state.scanning = false;
            crate::app::refresh::background_changed();
        });
    }

    pub fn totals(&self) -> Totals {
        totals_of(&self.state.lock().unwrap().days)
    }

    pub fn billing_totals(&self) -> BillingTotals {
        self.state.lock().unwrap().billing
    }

    pub fn is_loading(&self) -> bool {
        self.state.lock().unwrap().scanned_at == 0.0
    }

    pub fn all_time(&self) -> f64 {
        self.state.lock().unwrap().all_time
    }

    /// Seconds with Claude, all sessions together (time_with_claude): today, this week, this month.
    pub fn time_together(&self) -> Totals {
        let [today, week, month] = self.state.lock().unwrap().time_together;
        Totals { today, week, month }
    }

    /// Seconds spent with Claude on a project (active_seconds): today, and in all.
    pub fn time_spent(&self, project: &str) -> (f64, f64) {
        let state = self.state.lock().unwrap();
        (state.active_today.get(project).copied().unwrap_or(0.0), state.active_all_time.get(project).copied().unwrap_or(0.0))
    }

    /// What the session's replies today would cost at API prices; None when none was priced.
    pub fn session_today(&self, session: &str) -> Option<f64> {
        self.state.lock().unwrap().session_today.get(session).copied()
    }

    pub fn error(&self) -> String {
        self.state.lock().unwrap().error.clone()
    }
}

/// The days a session report covers, and the window the digest compares with the one before it.
pub const REPORT_DAYS: i64 = 7;
const REPORT_TOP_SESSIONS: usize = 15;
const DIGEST_FLAGGED_SESSIONS: usize = 3;
/// Every reply re-reads the whole context: past this average, cache reads dominate a session's
/// cost (43% of all spending in October 2026 came from the 15 sessions above it), and a fresh
/// session or a compaction does the same work for less.
const LARGE_CONTEXT_TOKENS: i64 = 400_000;
/// Cache writes cost 12-20 times a cache read: this share of the prompt means the cache kept being
/// rebuilt (a model switch, edits above the cached prefix, pauses past its TTL).
const CACHE_CHURN_SHARE: f64 = 0.10;
const CACHE_CHURN_MIN_REPLIES: i64 = 10;
/// Sessions this long carry a context that grew all along; splitting the work keeps each one small.
const LONG_SESSION_REPLIES: i64 = 1000;

/// One session's usage in a report window: replies, their API-price estimate, and their tokens.
pub struct SessionUsage {
    pub session: String,
    pub project: String,
    pub replies: i64,
    pub cost: f64,
    pub unpriced_replies: i64,
    /// Every prompt token of every reply: fresh input, cache reads and cache writes.
    pub prompt_tokens: i64,
    pub cache_write_tokens: i64,
}

impl SessionUsage {
    pub fn average_context(&self) -> i64 {
        self.prompt_tokens / self.replies.max(1)
    }

    /// Why the session spent more than its work needed, empty when nothing stands out.
    pub fn waste_flags(&self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        if self.average_context() >= LARGE_CONTEXT_TOKENS {
            flags.push("large context");
        }
        if self.replies >= CACHE_CHURN_MIN_REPLIES && self.prompt_tokens > 0
            && self.cache_write_tokens as f64 / self.prompt_tokens as f64 >= CACHE_CHURN_SHARE {
            flags.push("cache churn");
        }
        if self.replies >= LONG_SESSION_REPLIES {
            flags.push("long session");
        }
        flags
    }
}

/// Each session's usage on the days from `first_day` up to, not including, `end_day` (YYYY-MM-DD),
/// the most expensive first.
pub fn session_usage(archive: &Connection, first_day: &str, end_day: &str) -> rusqlite::Result<Vec<SessionUsage>> {
    let mut statement = archive.prepare(
        "select session, min(project), count(*), coalesce(sum(cost), 0), count(*) - count(cost),
                sum(input + cache_read + cache_write_5m + cache_write_1h), sum(cache_write_5m + cache_write_1h)
         from replies where day >= ?1 and day < ?2 group by session order by 4 desc")?;
    let rows = statement.query_map([first_day, end_day], |row| Ok(SessionUsage {
        session: row.get(0)?, project: row.get(1)?, replies: row.get(2)?, cost: row.get(3)?, unpriced_replies: row.get(4)?,
        prompt_tokens: row.get(5)?, cache_write_tokens: row.get(6)?,
    }))?;
    rows.collect()
}

/// The estimate of the sessions with a waste flag, and of all of them.
fn flagged_and_total(sessions: &[SessionUsage]) -> (f64, f64) {
    sessions.iter().fold((0.0, 0.0), |(flagged, total), usage|
        (flagged + if usage.waste_flags().is_empty() { 0.0 } else { usage.cost }, total + usage.cost))
}

fn share_text(part: f64, whole: f64) -> String {
    if whole > 0.0 { format!("{:.0}%", part / whole * 100.0) } else { "–".into() }
}

fn session_title(session: &str) -> String {
    let title = sessions::read_title(session);
    if title.is_empty() { "(untitled)".into() } else { title }
}

/// The session report as text: the last REPORT_DAYS days up to `today`, the most expensive sessions
/// with their waste flags, and how much went to flagged sessions against the window before.
pub fn session_report(archive: &Connection, today: NaiveDate, title_of: impl Fn(&str) -> String) -> rusqlite::Result<String> {
    let end = (today + Duration::days(1)).to_string();
    let first = today - Duration::days(REPORT_DAYS - 1);
    let current = session_usage(archive, &first.to_string(), &end)?;
    let previous = session_usage(archive, &(first - Duration::days(REPORT_DAYS)).to_string(), &first.to_string())?;
    let mut out = format!("sessions since {first} · token-based estimates at API prices, not your bill\n");
    if current.is_empty() {
        out.push_str("  No recorded usage\n");
        return Ok(out);
    }
    out.push_str(&format!("  {:<18}  {:<44}  {:>9}  {:>7}  {:>7}  flags\n", "project", "session", "estimate", "replies", "context"));
    for usage in current.iter().take(REPORT_TOP_SESSIONS) {
        let mut flags = usage.waste_flags().join(", ");
        if usage.unpriced_replies > 0 {
            flags.push_str(&format!("{}{} unpriced replies left out", if flags.is_empty() { "" } else { ", " }, usage.unpriced_replies));
        }
        out.push_str(&format!("  {:<18}  {:<44}  {:>9}  {:>7}  {:>6}K  {flags}\n",
                              usage.project.chars().take(18).collect::<String>(), title_of(&usage.session).chars().take(44).collect::<String>(),
                              money(usage.cost), usage.replies, usage.average_context() / 1000));
    }
    if current.len() > REPORT_TOP_SESSIONS {
        let rest = &current[REPORT_TOP_SESSIONS..];
        out.push_str(&format!("  and {} more sessions · {}\n", rest.len(), money(rest.iter().map(|usage| usage.cost).sum())));
    }
    let (flagged, total) = flagged_and_total(&current);
    let (previous_flagged, previous_total) = flagged_and_total(&previous);
    out.push_str(&format!("flagged sessions  {} of {} ({}) · the {REPORT_DAYS} days before: {}\n",
                          money(flagged), money(total), share_text(flagged, total), share_text(previous_flagged, previous_total)));
    Ok(out)
}

/// cockpit-sidebar sessions: the session report, from the archive brought up to date first.
pub fn print_sessions() -> rusqlite::Result<()> {
    let mut archive = open_archive(&paths::usage_archive())?;
    archive_transcripts(&mut archive, &paths::transcripts())?;
    let opencode = archive_opencode(&mut archive, &paths::opencode_database()).err();
    backfill_costs(&mut archive)?;
    let mut out = session_report(&archive, Local::now().date_naive(), session_title)?;
    if let Some(error) = opencode {
        out.push_str(&format!("opencode's replies left out: {error}\n"));
    }
    write_out(&out);
    Ok(())
}

/// Printed in one go; a reader that went away (`| head`) is not an error.
fn write_out(text: &str) {
    use std::io::Write;
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// cockpit-sidebar costs: the API-price estimate per day this month, per month, per model
/// and per project, from the archive (brought up to date first).
pub fn print_costs() -> rusqlite::Result<()> {
    let mut archive = open_archive(&paths::usage_archive())?;
    archive_transcripts(&mut archive, &paths::transcripts())?;
    let opencode = archive_opencode(&mut archive, &paths::opencode_database()).err();
    backfill_costs(&mut archive)?;
    if let Err(error) = crate::billing::import(&mut archive) { eprintln!("billing metadata: {error}"); }
    let (days, unpriced, all_time) = archived_costs(&archive)?;
    let first = Local::now().date_naive().with_day(1).unwrap().to_string();
    let mut out = String::new();
    out.push_str("this month\n");
    let mut month_days: Vec<_> = days.iter().filter(|(day, _)| **day >= first).collect();
    month_days.sort_by(|a, b| a.0.cmp(b.0));
    for (day, cost) in month_days {
        out.push_str(&format!("  {day}  {:>9}\n", money(*cost)));
    }
    out.push_str("per month\n");
    let mut statement = archive.prepare("select substr(day, 1, 7), sum(cost) from replies group by 1 order by 1")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?)))? {
        let (month, cost) = row?;
        out.push_str(&format!("  {month}     {:>9}\n", cost.map(money).unwrap_or_else(|| "unpriced".into())));
    }
    out.push_str("per model\n");
    let mut statement = archive.prepare("select model, sum(cost), sum(output) from replies group by 1 order by 2 desc")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?, row.get::<_, i64>(2)?)))? {
        let (model, cost, output) = row?;
        out.push_str(&format!("  {model:<24} {:>9}  {:.1}M output tokens\n", cost.map(money).unwrap_or_else(|| "unpriced".into()), output as f64 / 1e6));
    }
    out.push_str("top projects\n");
    let mut statement = archive.prepare("select project, sum(cost) from replies group by 1 order by 2 desc limit 10")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?)))? {
        let (project, cost) = row?;
        out.push_str(&format!("  {project:<24} {:>9}\n", cost.map(money).unwrap_or_else(|| "unpriced".into())));
    }
    let first_day: Option<String> = archive.query_row("select min(day) from replies", [], |row| row.get(0))?;
    out.push_str(&format!("all time      {:>9}  (since {})\n", money(all_time), first_day.unwrap_or_else(|| "None".into())));
    let mut unpriced: Vec<_> = unpriced.into_iter().collect();
    unpriced.sort();
    for (model, output) in unpriced {
        out.push_str(&format!("no price for {model}: {output} output tokens left out\n"));
    }
    if let Some(error) = opencode {
        out.push_str(&format!("opencode's replies left out: {error}\n"));
    }
    write_out(&out);
    Ok(())
}

/// What the digest says about the Chief: its answers since `since_ms`, or why they could not be read.
pub enum ChiefSection {
    Answers { since_ms: i64, digest: ChiefDigest },
    Unavailable(String),
}

/// The name of a session in the digest, `<label> · <project>`, or the end of its id when it has no label.
fn digest_session_name(sessions: &[sessions::Session], session_id: &str) -> String {
    match sessions.iter().find(|session| session.session == session_id && !session.label.is_empty()) {
        Some(session) if session.project.is_empty() => session.label.clone(),
        Some(session) => format!("{} · {}", session.label, session.project),
        None => chief_view::fallback_session_name(session_id),
    }
}

fn chief_digest_lines(sessions: &[sessions::Session], chief: &ChiefSection, today: NaiveDate) -> Vec<String> {
    let (bold, dim, yellow, magenta, reset) = ("\x1b[1m", "\x1b[2m", "\x1b[33m", "\x1b[35m", "\x1b[0m");
    let (since_ms, digest) = match chief {
        ChiefSection::Unavailable(reason) => return vec![format!("  {yellow}Chief: unavailable ({}){reset}", chief_view::one_line(reason, 80)), String::new()],
        ChiefSection::Answers { since_ms, digest } => (*since_ms, digest),
    };
    let since = chief_view::since_label(since_ms, today);
    let mut lines = Vec::new();
    if digest.answered == 0 && digest.escalated == 0 && digest.overruled == 0 {
        lines.push(format!("  {bold}Chief{reset}: nothing answered since {since}"));
    } else {
        lines.push(format!("  {bold}Chief{reset} answered {}, escalated {}, overruled {} since {since}", digest.answered, digest.escalated, digest.overruled));
        for answer in &digest.lines {
            let name = digest_session_name(sessions, &answer.session_id);
            lines.push(format!("    {}", chief_view::answer_summary_line(&name, &answer.question, &answer.answer, chief_view::DIGEST_LINE_WIDTH - 4)));
        }
        if digest.more > 0 {
            lines.push(format!("    {dim}+{} more{reset}", digest.more));
        }
    }
    if digest.new_habits > 0 {
        lines.push(format!("  {bold}{magenta}New habits{reset}: {}", digest.new_habits));
        for habit in &digest.habits {
            lines.push(format!("    {}", chief_view::habit_summary_line(&habit.text, habit.project.as_deref(), chief_view::DIGEST_LINE_WIDTH - 4)));
        }
    }
    lines.push(String::new());
    lines
}

/// The morning digest as ANSI text: the last day with work before today, today so far, the plan's
/// usage, the sessions waiting for you, the pull requests (failing CI first), and what the Chief answered.
pub fn digest_lines(archive: &Connection, sessions: &[sessions::Session], reviews: &[reviews::Review], today: NaiveDate, chief: &ChiefSection) -> rusqlite::Result<Vec<String>> {
    let (bold, dim, reset) = ("\x1b[1m", "\x1b[2m", "\x1b[0m");
    let (red, green, yellow, blue, magenta) = ("\x1b[31m", "\x1b[32m", "\x1b[33m", "\x1b[34m", "\x1b[35m");
    let mut lines = vec![String::new(), format!("  {bold}{blue}Claude · {}{reset}", today.format("%A %d %B")), String::new()];
    let last_day: Option<String> = archive.query_row("select max(day) from replies where day < ?", [today.to_string()], |row| row.get(0))?;
    if let Some(last_day) = last_day {
        let (total, count): (Option<f64>, i64) = archive.query_row(
            "select sum(cost), count(distinct session) from replies where day = ?", [&last_day], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let label = if last_day == (today - Duration::days(1)).to_string() {
            "Yesterday".to_string()
        } else {
            NaiveDate::parse_from_str(&last_day, "%Y-%m-%d").map(|day| day.format("%A %d %b").to_string()).unwrap_or(last_day.clone())
        };
        lines.push(format!("  {bold}{label}{reset}  {magenta}{}{reset} {dim}at API prices · {count} session{}{reset}",
                           money(total.unwrap_or(0.0)), if count != 1 { "s" } else { "" }));
        let (mut short_count, mut short_cost) = (0, 0.0);
        let mut statement = archive.prepare(
            "select session, project, sum(cost), count(*) from replies where day = ? group by session order by 3 desc")?;
        let rows = statement.query_map([&last_day], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<f64>>(2)?, row.get::<_, i64>(3)?)))?;
        for row in rows {
            let (session, project, cost, replies) = row?;
            if replies < DIGEST_MIN_REPLIES {
                short_count += 1;
                short_cost += cost.unwrap_or(0.0);
                continue;
            }
            let title = sessions::read_title(&session);
            let title = if title.is_empty() { "(untitled)".to_string() } else { title };
            lines.push(format!("    {:<18}  {:<44}  {:>8}  {dim}{replies} replies{reset}",
                               project.chars().take(18).collect::<String>(), title.chars().take(44).collect::<String>(), money(cost.unwrap_or(0.0))));
        }
        if short_count > 0 {
            lines.push(format!("    {dim}and {short_count} short session{} · {}{reset}", if short_count != 1 { "s" } else { "" }, money(short_cost)));
        }
        lines.push(String::new());
    }
    let week_start = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let sum_from = |sql: &str, day: String| -> rusqlite::Result<f64> {
        Ok(archive.query_row(sql, [day], |row| row.get::<_, Option<f64>>(0))?.unwrap_or(0.0))
    };
    let today_cost = sum_from("select sum(cost) from replies where day = ?", today.to_string())?;
    let week_cost = sum_from("select sum(cost) from replies where day >= ?", week_start.to_string())?;
    lines.push(format!("  {bold}Today so far{reset}  {magenta}{}{reset}   {dim}this week{reset} {magenta}{}{reset}", money(today_cost), money(week_cost)));
    let report_start = today - Duration::days(REPORT_DAYS - 1);
    let recent = session_usage(archive, &report_start.to_string(), &(today + Duration::days(1)).to_string())?;
    if !recent.is_empty() {
        let previous = session_usage(archive, &(report_start - Duration::days(REPORT_DAYS)).to_string(), &report_start.to_string())?;
        let (flagged, total) = flagged_and_total(&recent);
        let (previous_flagged, previous_total) = flagged_and_total(&previous);
        lines.push(format!("  {bold}Last {REPORT_DAYS} days{reset}  {magenta}{}{reset} {dim}·{reset} {} in flagged sessions {dim}(the {REPORT_DAYS} days before: {}){reset}",
                           money(total), share_text(flagged, total), share_text(previous_flagged, previous_total)));
        for usage in recent.iter().filter(|usage| !usage.waste_flags().is_empty()).take(DIGEST_FLAGGED_SESSIONS) {
            lines.push(format!("    {:<18}  {:<44}  {:>8}  {yellow}{}{reset}", usage.project.chars().take(18).collect::<String>(),
                               session_title(&usage.session).chars().take(44).collect::<String>(), money(usage.cost), usage.waste_flags().join(", ")));
        }
        lines.push(format!("    {dim}cockpit-sidebar sessions lists them all{reset}"));
    }
    let windows = usage::windows();
    if !windows.is_empty() {
        let parts: Vec<String> = windows.iter().filter_map(|window| {
            let percentage = window.percentage?;
            let left = usage::time_left(window);
            Some(format!("{} {percentage}%", window.label) + &if left.is_empty() { String::new() } else { format!(" {dim}({left} left){reset}") })
        }).collect();
        lines.push(format!("  {bold}Plan{reset}  {}", parts.join("   ")));
    }
    lines.push(String::new());
    let waiting: Vec<_> = sessions.iter().filter(|session| session.state == "needs-you" || session.state == "waiting").collect();
    if !waiting.is_empty() {
        lines.push(format!("  {bold}Waiting for you{reset}"));
        for session in waiting {
            let mark = if session.state == "needs-you" { format!("{red}!{reset}") } else { format!("{yellow}○{reset}") };
            lines.push(format!("    {mark} {:<18} {dim}{}{reset}", session.project, sessions::visible_task(session).chars().take(50).collect::<String>()));
        }
        lines.push(String::new());
    }
    if !reviews.is_empty() {
        let order = |ci: &str| match ci { "failing" => 0, "pending" => 1, "passing" => 2, _ => 3 };
        let mut sorted: Vec<_> = reviews.iter().collect();
        sorted.sort_by_key(|row| order(&row.ci));
        lines.push(format!("  {bold}Pull requests{reset}"));
        for row in sorted.into_iter().take(10) {
            let mark = match row.ci.as_str() { "failing" => format!("{red}✗{reset}"), "pending" => format!("{yellow}●{reset}"), "passing" => format!("{green}✓{reset}"), _ => " ".into() };
            let review = match row.review.as_str() {
                "APPROVED" => format!("{green}approved{reset}"), "CHANGES_REQUESTED" => format!("{red}changes{reset}"),
                "REVIEW_REQUIRED" => format!("{yellow}review{reset}"), _ => String::new(),
            };
            lines.push(format!("    {mark} {}#{}  {}  {review}", row.repository, row.number, row.title.chars().take(50).collect::<String>()));
        }
        lines.push(String::new());
    }
    lines.extend(chief_digest_lines(sessions, chief, today));
    Ok(lines)
}

/// When the last digest printed, or 24 hours ago when no digest did (or its note is unreadable or from the future).
fn chief_digest_since(now_ms: i64) -> i64 {
    fs::read_to_string(paths::chief_digest_at()).ok().and_then(|text| text.trim().parse::<i64>().ok())
        .filter(|since_ms| (1..=now_ms).contains(since_ms)).unwrap_or(now_ms - chief_view::DEFAULT_LOOKBACK_MS)
}

fn chief_section(since_ms: i64) -> ChiefSection {
    let answers = chief_store::open_read_only(&paths::team_database()).and_then(|connection| chief_store::answers_since(&connection, since_ms));
    match answers {
        Ok(digest) => ChiefSection::Answers { since_ms, digest },
        Err(error) => ChiefSection::Unavailable(error.message),
    }
}

/// Up to when the digest showed Theo his new habits: all of them when it listed them all, else only
/// down to the oldest it listed, so the ones it counted but left out stay new.
fn habits_seen_after_digest(digest: &chief_store::ChiefDigest, printed_at_ms: i64) -> Option<i64> {
    if digest.new_habits == 0 { return None; }
    if digest.new_habits as usize <= digest.habits.len() { return Some(printed_at_ms); }
    digest.habits.last().map(|oldest_listed| oldest_listed.created_at_ms - 1)
}

/// cockpit-sidebar digest: the morning digest (cockpit digest shows it).
pub fn print_digest() -> rusqlite::Result<()> {
    let mut archive = open_archive(&paths::usage_archive())?;
    archive_transcripts(&mut archive, &paths::transcripts())?;
    if let Err(error) = archive_opencode(&mut archive, &paths::opencode_database()) {
        util::log_event("ERROR", &format!("digest: opencode's replies left out: {error}"));
    }
    backfill_costs(&mut archive)?;
    if let Err(error) = crate::billing::import(&mut archive) { eprintln!("billing metadata: {error}"); }
    let repositories: Vec<String> = reviews::session_checkouts("").into_keys().collect();
    let (rows, _) = reviews::fetch_reviews(&repositories);
    // Taken before the answers are read: an answer given while the digest builds shows twice rather than never.
    let printed_at_ms = chrono::Utc::now().timestamp_millis();
    let chief = chief_section(chief_digest_since(printed_at_ms));
    let lines = digest_lines(&archive, &sessions::reported(), &rows, Local::now().date_naive(), &chief)?;
    write_out(&(lines.join("\n") + "\n"));
    // Answers that could not be read were not shown, so the next digest still has to cover them.
    if let ChiefSection::Answers { digest, .. } = &chief {
        let note = paths::chief_digest_at();
        let written = note.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::write(&note, printed_at_ms.to_string()));
        if let Err(error) = written { util::log_event("ERROR", &format!("digest: cannot save when it printed: {error}")); }
        if let Some(seen_ms) = habits_seen_after_digest(digest, printed_at_ms) {
            if let Err(error) = crate::chief::save_habits_seen(seen_ms) { util::log_event("ERROR", &format!("digest: cannot save that it showed the habits: {error}")); }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_reply_costs_its_tokens_at_the_models_prices() {
        let usage = json!({"input_tokens": 1_000_000, "output_tokens": 1_000_000});
        assert_eq!(reply_cost("claude-opus-5-5-20260101", &usage), Some(24.0));
        assert_eq!(reply_cost("claude-unknown", &usage), None);
        let fast = json!({"output_tokens": 1_000_000, "speed": "fast"});
        assert_eq!(reply_cost("claude-opus-5-5", &fast), Some(40.0));
    }

    #[test]
    fn previously_unpriced_models_include_input_output_and_cache_in_the_estimate() {
        let usage = json!({"input_tokens": 100_000, "output_tokens": 1_000_000, "cache_read_input_tokens": 100_000});
        for (model, expected) in [
            ("claude-opus-4-5", 25.55), ("gpt-5.3-codex", 14.1925),
            ("gpt-5.4", 15.275), ("gpt-5.5", 30.55),
            ("gpt-6-astra", 51.1), ("gpt-6-astra-fast", 102.2), ("gpt-6.1-sol", 10.21),
            ("gemini-3.1-pro-preview", 12.22),
        ] {
            let cost = reply_cost(model, &usage).unwrap_or_else(|| panic!("no price for {model}"));
            assert!((cost - expected).abs() < 1e-9, "{model}: {cost} != {expected}");
        }
    }

    #[test]
    fn long_prompts_use_the_full_request_long_context_rate() {
        for (model, threshold, at_threshold, above_threshold) in [
            ("gpt-5.4", 272_000, 15.68, 23.860005),
            ("gpt-5.5", 272_000, 31.36, 47.72001),
            ("gpt-6-astra", 272_000, 52.72, 80.44002),
            ("gpt-6.1-sol", 272_000, 10.544, 16.088004),
            ("gemini-3.1-pro-preview", 200_000, 12.4, 18.800004),
        ] {
            for (input, expected) in [(threshold, at_threshold), (threshold + 1, above_threshold)] {
                let cost = reply_cost(model, &json!({"input_tokens": input, "output_tokens": 1_000_000})).unwrap();
                assert!((cost - expected).abs() < 1e-9, "{model}, {input}: {cost} != {expected}");
            }
        }
    }

    #[test]
    fn only_dated_snapshots_inherit_a_models_price() {
        let usage = json!({"output_tokens": 1_000_000});
        assert_eq!(reply_cost("gpt-5.4-2026-03-05", &usage), Some(15.0));
        for model in ["gpt-5.4-mini", "gpt-5.4-pro", "gpt-5.5-fast", "gpt-6.1-sol-unknown", "big-pickle"] {
            assert_eq!(reply_cost(model, &usage), None, "{model}");
        }
    }

    #[test]
    fn a_line_without_ids_is_keyed_by_its_sha1_like_the_python_column() {
        let line = br#"{"timestamp":"2026-10-18T10:00:00Z","message":{"model":"claude-opus-5-5","usage":{"output_tokens":5}}}"#;
        let mut with_newline = line.to_vec();
        with_newline.push(b'\n');
        let row = reply_row(&with_newline).unwrap();
        assert_eq!(row.key, format!("line:{}", sha1_smol::Sha1::from(&with_newline[..]).digest()));
        assert_eq!(row.project, "?");
    }

    #[test]
    fn the_archive_keeps_each_reply_once() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        fs::create_dir_all(directory.join("project")).unwrap();
        let reply = r#"{"timestamp":"2026-10-18T10:00:00Z","requestId":"r1","sessionId":"s","cwd":"/x/shop","message":{"id":"m1","model":"claude-opus-5-5","usage":{"output_tokens":1000000}}}"#;
        fs::write(directory.join("project/a.jsonl"), format!("{reply}\n")).unwrap();
        fs::write(directory.join("project/b.jsonl"), format!("{reply}\n")).unwrap();
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_transcripts(&mut archive, &directory).unwrap(), 1);
        assert_eq!(archive_transcripts(&mut archive, &directory).unwrap(), 0);
        let (days, _, all_time) = archived_costs(&archive).unwrap();
        assert_eq!((days.len(), all_time), (1, 20.0));
        fs::remove_dir_all(directory).unwrap();
    }

    /// An opencode 2 assistant message as its database keeps it (session_message.data).
    fn opencode_message(created_ms: i64, model: &str, output: i64, cost: f64) -> String {
        json!({"agent": "build", "model": {"id": model, "providerID": "anthropic"}, "content": [], "cost": cost,
               "tokens": {"input": 0, "output": output, "reasoning": 0, "cache": {"read": 0, "write": 0}},
               "time": {"created": created_ms, "completed": created_ms + 1000}}).to_string()
    }

    #[test]
    fn an_opencode_reply_is_priced_from_its_tokens_like_a_claude_one() {
        // On a subscription opencode's own cost is 0: the tokens say what it would cost.
        let row = opencode_reply_row("msg_1", "ses_1", &opencode_message(1_760_781_600_000, "claude-opus-5-5", 1_000_000, 0.0), "/x/shop").unwrap();
        assert_eq!((row.key.as_str(), row.cost, row.project.as_str(), row.session.as_str()), ("opencode:msg_1", Some(20.0), "shop", "ses_1"));
        assert_eq!(row.at, "2025-10-18T10:00:00.000Z");
    }

    #[test]
    fn a_model_without_a_known_price_keeps_opencodes_cost() {
        let row = opencode_reply_row("msg_1", "ses_1", &opencode_message(1_760_781_600_000, "gpt-7", 1000, 0.42), "/x/shop").unwrap();
        assert_eq!(row.cost, Some(0.42));
        let unpriced = opencode_reply_row("msg_2", "ses_1", &opencode_message(1_760_781_600_000, "gpt-7", 1000, 0.0), "/x/shop").unwrap();
        assert_eq!(unpriced.cost, None);
    }

    fn local_message(created_ms: i64) -> String {
        let mut message: Value = serde_json::from_str(&opencode_message(created_ms, "qwen38", 1000, 0.0)).unwrap();
        message["model"]["providerID"] = json!("local");
        message.to_string()
    }

    #[test]
    fn a_local_models_reply_costs_nothing() {
        let row = opencode_reply_row("msg_1", "ses_1", &local_message(1_760_781_600_000), "/x/shop").unwrap();
        assert_eq!(row.cost, Some(0.0));
    }

    #[test]
    fn an_archive_from_before_version_3_prices_its_local_replies_from_opencodes_database() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let (database, source) = opencode_database(&directory);
        add_opencode_reply(&source, "msg_1", 5, &local_message(1_760_781_600_000));
        {
            let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
            archive_opencode(&mut archive, &database).unwrap();
            archive.execute_batch("update replies set cost = null; update databases set read_until = 10 * 60000; pragma user_version = 2;").unwrap();
        }
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 1);
        assert_eq!(archive.query_row("select cost from replies where key = 'opencode:msg_1'", [], |row| row.get::<_, Option<f64>>(0)).unwrap(), Some(0.0));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn only_completed_replies_with_tokens_are_archived() {
        let mut streaming: Value = serde_json::from_str(&opencode_message(1, "claude-opus-5-5", 5, 0.0)).unwrap();
        streaming["time"].as_object_mut().unwrap().remove("completed");
        for data in [streaming.to_string(), opencode_message(1, "claude-opus-5-5", 0, 0.0), "not json".into()] {
            assert!(opencode_reply_row("msg_1", "ses_1", &data, "/x/shop").is_none(), "{data}");
        }
    }

    /// An opencode database with opencode 2.0.26's tables (packages/core/src/session/sql.ts), the
    /// columns read here, and one session in /x/shop with a user message.
    fn opencode_database(directory: &Path) -> (PathBuf, Connection) {
        fs::create_dir_all(directory).unwrap();
        let database = directory.join("opencode.db");
        let source = Connection::open(&database).unwrap();
        source.execute_batch("create table session_v2 (id text primary key, directory text not null);
            create table session_message (id text primary key, session_id text not null, type text not null, seq integer not null,
                                          time_updated integer not null, data text not null);
            insert into session_v2 values ('ses_1', '/x/shop');
            insert into session_message values ('msg_0', 'ses_1', 'user', 0, 0, '{\"text\": \"hi\"}');").unwrap();
        (database, source)
    }

    fn add_opencode_reply(source: &Connection, id: &str, updated_ms: i64, data: &str) {
        source.execute("insert into session_message values (?, 'ses_1', 'assistant', (select count(*) from session_message), ?, ?)",
                       params![id, updated_ms, data]).unwrap();
    }

    #[test]
    fn opencodes_replies_join_the_archive_once_and_count_as_time_spent() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let (database, source) = opencode_database(&directory);
        // Both replies on one day, and that day the one checked: ten minutes ago is yesterday just
        // after midnight.
        let day_of = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).unwrap().with_timezone(&chrono::Local).date_naive();
        let mut start = chrono::Local::now().timestamp_millis() - 600_000;
        if day_of(start) != day_of(start + 120_000) {
            start -= 300_000;
        }
        for (id, offset) in [("msg_1", 0), ("msg_2", 120_000)] {
            add_opencode_reply(&source, id, start + offset + 1000, &opencode_message(start + offset, "claude-opus-5-5", 1_000_000, 0.0));
        }
        drop(source);
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 2);
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 0);
        assert_eq!(archived_costs(&archive).unwrap().2, 40.0);
        let day = day_of(start).to_string();
        assert_eq!(active_seconds(&archive, &day).unwrap().get("shop").map(|seconds| seconds.round()), Some(120.0));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn no_opencode_database_is_nothing_to_archive() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &directory.join("opencode.db")).unwrap(), 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn opencode_messages_updated_before_the_last_pass_are_not_read_again() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let (database, source) = opencode_database(&directory);
        let newest = 10 * OPENCODE_REREAD_MS;
        add_opencode_reply(&source, "msg_1", newest, &opencode_message(1_760_781_600_000, "claude-opus-5-5", 1000, 0.0));
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 1);
        add_opencode_reply(&source, "msg_old", newest - OPENCODE_REREAD_MS - 1, &opencode_message(1_760_781_600_000, "claude-opus-5-5", 1000, 0.0));
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_opencode_reply_still_streaming_at_one_pass_joins_the_archive_once_completed() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let (database, source) = opencode_database(&directory);
        let mut streaming: Value = serde_json::from_str(&opencode_message(1_760_781_600_000, "claude-opus-5-5", 1000, 0.0)).unwrap();
        streaming["time"].as_object_mut().unwrap().remove("completed");
        add_opencode_reply(&source, "msg_1", 1_760_781_600_000, &streaming.to_string());
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 0);
        source.execute("update session_message set data = ?, time_updated = ? where id = 'msg_1'",
                       params![opencode_message(1_760_781_600_000, "claude-opus-5-5", 1000, 0.0), 1_760_781_600_000 + 10 * OPENCODE_REREAD_MS]).unwrap();
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    fn session_with(replies: i64, prompt_tokens: i64, cache_write_tokens: i64) -> SessionUsage {
        SessionUsage { session: "s".into(), project: "shop".into(), replies, cost: 1.0, unpriced_replies: 0, prompt_tokens, cache_write_tokens }
    }

    #[test]
    fn each_waste_flag_starts_at_its_threshold() {
        for (usage, expected) in [
            (session_with(10, 10 * (LARGE_CONTEXT_TOKENS - 1), 0), vec![]),
            (session_with(10, 10 * LARGE_CONTEXT_TOKENS, 0), vec!["large context"]),
            (session_with(10, 1_000_000, 99_999), vec![]),
            (session_with(10, 1_000_000, 100_000), vec!["cache churn"]),
            (session_with(CACHE_CHURN_MIN_REPLIES - 1, 1_000_000, 500_000), vec![]),
            (session_with(LONG_SESSION_REPLIES, LONG_SESSION_REPLIES, 0), vec!["long session"]),
        ] {
            assert_eq!(usage.waste_flags(), expected, "{} replies, {} prompt, {} written", usage.replies, usage.prompt_tokens, usage.cache_write_tokens);
        }
    }

    #[test]
    fn a_session_without_replies_has_no_context_to_flag() {
        assert!(session_with(0, 0, 0).waste_flags().is_empty());
    }

    /// One reply of `session` on `day`, `context` prompt tokens read from the cache, priced `cost`.
    fn add_reply(archive: &Connection, key: &str, day: &str, session: &str, context: i64, cost: Option<f64>) {
        archive.execute("insert into replies values (?1, ?2, ?2 || 'T10:00:00Z', 'claude-opus-5-5', 'shop', ?3, 0, 1000, ?4, 0, 0, 'standard', ?5)",
                        params![key, day, session, context, cost]).unwrap();
    }

    fn report_archive() -> (PathBuf, Connection) {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        (directory, archive)
    }

    #[test]
    fn the_session_report_ranks_this_weeks_sessions_and_flags_the_wasteful_one() {
        let (directory, archive) = report_archive();
        add_reply(&archive, "a", "2026-10-25", "big", 500_000, Some(30.0));
        add_reply(&archive, "b", "2026-10-19", "small", 50_000, Some(10.0));
        add_reply(&archive, "old", "2026-10-18", "older", 50_000, Some(99.0));
        let report = session_report(&archive, NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(), |session| format!("title of {session}")).unwrap();
        let big = report.lines().position(|line| line.contains("title of big")).unwrap();
        let small = report.lines().position(|line| line.contains("title of small")).unwrap();
        assert!(big < small, "{report}");
        assert!(report.lines().nth(big).unwrap().contains("$30.00") && report.lines().nth(big).unwrap().ends_with("large context"), "{report}");
        assert!(!report.lines().nth(small).unwrap().contains("context,") && !report.contains("title of older"), "{report}");
        assert!(report.contains("flagged sessions  $30.00 of $40.00 (75%) · the 7 days before: 0%"), "{report}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_session_report_names_unpriced_replies_instead_of_counting_them_free() {
        let (directory, archive) = report_archive();
        add_reply(&archive, "priced", "2026-10-25", "s", 0, Some(1.0));
        add_reply(&archive, "unpriced", "2026-10-25", "s", 0, None);
        let report = session_report(&archive, NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(), |_| "t".into()).unwrap();
        assert!(report.contains("1 unpriced replies left out"), "{report}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_week_without_replies_is_no_recorded_usage_not_a_zero_bill() {
        let (directory, archive) = report_archive();
        let report = session_report(&archive, NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(), |_| "t".into()).unwrap();
        assert!(report.contains("No recorded usage") && !report.contains("$0.00"), "{report}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_pass_over_an_opencode_database_without_replies_skips_none_of_the_later_ones() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let (database, source) = opencode_database(&directory);
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 0);
        let read_until: Option<i64> = archive.query_row("select read_until from databases", [], |row| row.get(0)).optional().unwrap();
        assert_eq!(read_until, None, "a database with no assistant message leaves nothing to resume from");
        add_opencode_reply(&source, "msg_1", 5, &opencode_message(1_760_781_600_000, "claude-opus-5-5", 1000, 0.0));
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
