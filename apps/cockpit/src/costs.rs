//! What Claude Code's replies would cost at API prices (the plan is flat): every reply of the
//! transcripts archived in usage.sqlite, which keeps them after Claude Code deletes old
//! transcripts. Same schema and keys as the Python column, so the archive carries over. opencode's
//! replies join them from its database (archive_opencode), keyed "opencode:<message id>".

use crate::paths;
use crate::reviews;
use crate::sessions;
use crate::usage;
use crate::util::{self, money, now, parse_time};
use chrono::{Datelike, Duration, Local, NaiveDate};
use rusqlite::{Connection, OpenFlags, Transaction, params};
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
    ("claude-sonnet-5-5", [2.0, 10.0, 0.20, 2.5, 4.0]), ("claude-sonnet-5", [2.0, 10.0, 0.20, 2.5, 4.0]),
    ("claude-sonnet-4-6", [3.0, 15.0, 0.3, 3.75, 6.0]),
    ("claude-haiku-5-5", [0.10, 0.50, 0.01, 0.125, 0.2]),
    ("claude-haiku-4-5", [1.0, 5.0, 0.1, 1.25, 2.0]),
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
";

/// The price row of a model id (date-suffixed ids included), or None for an unknown model.
fn model_prices(model: &str) -> Option<[f64; 5]> {
    MODEL_PRICES.iter()
        .filter(|(known, _)| model == *known || model.starts_with(&format!("{known}-")))
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

/// New replies added, a known one's project corrected; returns how many rows changed.
fn insert_replies(transaction: &Transaction, rows: &[Reply]) -> rusqlite::Result<usize> {
    let mut insert = transaction.prepare("insert into replies values (?,?,?,?,?,?,?,?,?,?,?,?,?) on conflict(key) do update set project = excluded.project where project != excluded.project")?;
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
    let cost = reply_cost(model, &usage).or_else(|| message.get("cost").and_then(Value::as_f64).filter(|cost| *cost > 0.0));
    let project = Path::new(cwd).file_name().map(|name| name.to_string_lossy().into_owned()).filter(|name| !name.is_empty()).unwrap_or("?".into());
    Some(Reply {
        key: format!("opencode:{id}"), day: created.with_timezone(&Local).date_naive().to_string(),
        at: created.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), model: model.into(), project, session: session.into(),
        input, output, cache_read, write_5m: cache_write, write_1h: 0, speed: "standard".into(), cost, cwd: cwd.into(),
    })
}

/// Copies opencode's replies into the archive: every assistant message of its database (opencode 2's
/// session_message, the folder from session_v2), read-only (its sandboxes write it), the known ones
/// only corrected. Nothing when opencode never ran. Returns how many rows changed.
pub fn archive_opencode(archive: &mut Connection, database: &Path) -> rusqlite::Result<usize> {
    if !database.exists() {
        return Ok(0);
    }
    let source = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(std::time::Duration::from_secs(1))?;
    let mut statement = source.prepare(
        "select message.id, message.session_id, message.data, session.directory from session_message message
         join session_v2 session on session.id = message.session_id where message.type = 'assistant'")?;
    let messages = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))?;
    let mut projects: HashMap<String, String> = HashMap::new();
    let mut rows = Vec::new();
    for message in messages {
        let (id, session, data, cwd) = message?;
        let Some(mut row) = opencode_reply_row(&id, &session, &data, &cwd) else { continue };
        if !row.cwd.is_empty() {
            row.project = projects.entry(row.cwd.clone()).or_insert_with(|| util::project_name(&util::repository_root(&row.cwd))).clone();
        }
        rows.push(row);
    }
    let transaction = archive.transaction()?;
    let changed = insert_replies(&transaction, &rows)?;
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

#[derive(Default)]
struct State {
    days: HashMap<String, f64>,
    all_time: f64,
    /// Seconds spent with Claude per project (active_seconds): today, and all the archive holds.
    active_today: HashMap<String, f64>,
    active_all_time: HashMap<String, f64>,
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
fn period_starts() -> [f64; 3] {
    let today = Local::now().date_naive();
    let midnight = |day: NaiveDate| day.and_hms_opt(0, 0, 0).and_then(|time| time.and_local_timezone(Local).earliest()).map(|time| time.timestamp() as f64).unwrap_or(0.0);
    [midnight(today), midnight(today - Duration::days(today.weekday().num_days_from_monday() as i64)), midnight(today.with_day(1).unwrap_or(today))]
}

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
            let result = open_archive(&path).and_then(|mut archive| {
                archive_transcripts(&mut archive, &paths::transcripts())?;
                if let Err(error) = archive_opencode(&mut archive, &paths::opencode_database()) {
                    opencode_error = error.to_string();
                }
                Ok((archived_costs(&archive)?, active_seconds(&archive, &today)?, active_seconds(&archive, "0000-00-00")?,
                    time_with_claude(&archive, &starts)?))
            });
            let mut state = shared.lock().unwrap();
            if opencode_error != state.opencode_error && !opencode_error.is_empty() {
                util::log_event("ERROR", &format!("usage archive: opencode's {}: {opencode_error}", paths::opencode_database().display()));
            }
            state.opencode_error = opencode_error;
            match result {
                Ok(((days, _, all_time), active_today, active_all_time, together)) => {
                    state.time_together = [together[0], together[1], together[2]];
                    state.days = days;
                    state.all_time = all_time;
                    state.active_today = active_today;
                    state.active_all_time = active_all_time;
                    state.error.clear();
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
        });
    }

    pub fn totals(&self) -> Totals {
        let state = self.state.lock().unwrap();
        let today = Local::now().date_naive();
        let since = |first: NaiveDate| -> f64 {
            let first = first.to_string();
            state.days.iter().filter(|(day, _)| **day >= first).map(|(_, cost)| cost).sum()
        };
        Totals {
            today: since(today),
            week: since(today - Duration::days(today.weekday().num_days_from_monday() as i64)),
            month: since(today.with_day(1).unwrap_or(today)),
        }
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

    pub fn error(&self) -> String {
        self.state.lock().unwrap().error.clone()
    }
}

/// Printed in one go; a reader that went away (`| head`) is not an error.
fn write_out(text: &str) {
    use std::io::Write;
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// claude-sessions-sidebar costs: the API-price estimate per day this month, per month, per model
/// and per project, from the archive (brought up to date first).
pub fn print_costs() -> rusqlite::Result<()> {
    let mut archive = open_archive(&paths::usage_archive())?;
    archive_transcripts(&mut archive, &paths::transcripts())?;
    let opencode = archive_opencode(&mut archive, &paths::opencode_database()).err();
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
        out.push_str(&format!("  {month}     {:>9}\n", money(cost.unwrap_or(0.0))));
    }
    out.push_str("per model\n");
    let mut statement = archive.prepare("select model, sum(cost), sum(output) from replies group by 1 order by 2 desc")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?, row.get::<_, i64>(2)?)))? {
        let (model, cost, output) = row?;
        out.push_str(&format!("  {model:<24} {:>9}  {:.1}M output tokens\n", money(cost.unwrap_or(0.0)), output as f64 / 1e6));
    }
    out.push_str("top projects\n");
    let mut statement = archive.prepare("select project, sum(cost) from replies group by 1 order by 2 desc limit 10")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?)))? {
        let (project, cost) = row?;
        out.push_str(&format!("  {project:<24} {:>9}\n", money(cost.unwrap_or(0.0))));
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

/// The morning digest as ANSI text: the last day with work before today, today so far, the plan's
/// usage, the sessions waiting for you, and the pull requests (failing CI first).
pub fn digest_lines(archive: &Connection, sessions: &[sessions::Session], reviews: &[reviews::Review], today: NaiveDate) -> rusqlite::Result<Vec<String>> {
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
    Ok(lines)
}

/// claude-sessions-sidebar digest: the morning digest (claude-sessions digest shows it).
pub fn print_digest() -> rusqlite::Result<()> {
    let mut archive = open_archive(&paths::usage_archive())?;
    archive_transcripts(&mut archive, &paths::transcripts())?;
    if let Err(error) = archive_opencode(&mut archive, &paths::opencode_database()) {
        util::log_event("ERROR", &format!("digest: opencode's replies left out: {error}"));
    }
    let repositories: Vec<String> = reviews::session_checkouts("").into_keys().collect();
    let (rows, _) = reviews::fetch_reviews(&repositories);
    let lines = digest_lines(&archive, &sessions::reported(), &rows, Local::now().date_naive())?;
    write_out(&(lines.join("\n") + "\n"));
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

    #[test]
    fn only_completed_replies_with_tokens_are_archived() {
        let mut streaming: Value = serde_json::from_str(&opencode_message(1, "claude-opus-5-5", 5, 0.0)).unwrap();
        streaming["time"].as_object_mut().unwrap().remove("completed");
        for data in [streaming.to_string(), opencode_message(1, "claude-opus-5-5", 0, 0.0), "not json".into()] {
            assert!(opencode_reply_row("msg_1", "ses_1", &data, "/x/shop").is_none(), "{data}");
        }
    }

    #[test]
    fn opencodes_replies_join_the_archive_once_and_count_as_time_spent() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        fs::create_dir_all(&directory).unwrap();
        let database = directory.join("opencode.db");
        let source = Connection::open(&database).unwrap();
        // opencode 2.0.26's tables (packages/core/src/session/sql.ts), the columns read here.
        source.execute_batch("create table session_v2 (id text primary key, directory text not null);
            create table session_message (id text primary key, session_id text not null, type text not null, seq integer not null, data text not null);
            insert into session_v2 values ('ses_1', '/x/shop');
            insert into session_message values ('msg_0', 'ses_1', 'user', 0, '{\"text\": \"hi\"}');").unwrap();
        let start = chrono::Local::now().timestamp_millis() - 600_000;
        for (seq, (id, offset)) in [("msg_1", 0), ("msg_2", 120_000)].into_iter().enumerate() {
            source.execute("insert into session_message values (?, 'ses_1', 'assistant', ?, ?)",
                           params![id, seq as i64 + 1, opencode_message(start + offset, "claude-opus-5-5", 1_000_000, 0.0)]).unwrap();
        }
        drop(source);
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 2);
        assert_eq!(archive_opencode(&mut archive, &database).unwrap(), 0);
        assert_eq!(archived_costs(&archive).unwrap().2, 40.0);
        let today = chrono::Local::now().date_naive().to_string();
        assert_eq!(active_seconds(&archive, &today).unwrap().get("shop").map(|seconds| seconds.round()), Some(120.0));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn no_opencode_database_is_nothing_to_archive() {
        let directory = std::env::temp_dir().join(format!("costs-test-{}", util::token_hex(4)));
        let mut archive = open_archive(&directory.join("usage.sqlite")).unwrap();
        assert_eq!(archive_opencode(&mut archive, &directory.join("opencode.db")).unwrap(), 0);
        fs::remove_dir_all(directory).unwrap();
    }
}
