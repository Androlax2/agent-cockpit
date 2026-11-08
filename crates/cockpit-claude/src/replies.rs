//! The model replies of Claude Code's transcripts (`<home>/.claude/projects/**/*.jsonl`) for
//! the cost archive (#699). A transcript is append-only, so a scan resumes at the byte after
//! the last complete line it counted. The transcripts are written by the agent, which may be
//! sandboxed: nothing here follows a link out of the home folder, and a line yields counts
//! and names only.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use cockpit_provider_api::{ModelReply, ProviderError, ReplyScan, ReplyTokens, SourceCursor};

/// Most replies one scan returns: the first scan of a long history comes in several, so the
/// daemon never holds a whole archive in memory.
const MAX_REPLIES_PER_SCAN: usize = 200_000;
/// Deepest folder nesting below `projects`: a project folder, a session folder, its
/// `subagents` folder, with room for what a later Claude Code adds.
const MAX_DEPTH: usize = 6;
const TRANSCRIPT_FOLDERS: [&str; 2] = [".claude", "projects"];

pub(crate) fn scan(home: &Path, cursors: &[SourceCursor]) -> Result<ReplyScan, ProviderError> {
    let mut scan = ReplyScan::default();
    let Some(projects) = plain_folder(home, &TRANSCRIPT_FOLDERS)? else {
        return Ok(scan);
    };
    let counted: HashMap<&str, u64> = cursors
        .iter()
        .map(|cursor| (cursor.source(), cursor.offset()))
        .collect();
    let mut transcripts = Vec::new();
    find_transcripts(&projects, 0, &mut transcripts, &mut scan.problems);
    transcripts.sort();
    for transcript in transcripts {
        if scan.replies.len() >= MAX_REPLIES_PER_SCAN {
            break;
        }
        let Some(source) = transcript
            .strip_prefix(home)
            .ok()
            .and_then(Path::to_str)
            .map(str::to_owned)
        else {
            scan.problems.push(format!(
                "{} is not a UTF-8 path under the home folder",
                transcript.display()
            ));
            continue;
        };
        let counted_to = counted.get(source.as_str()).copied().unwrap_or(0);
        match read_from(&transcript, &source, counted_to) {
            Ok(Some(read)) => {
                scan.replies.extend(read.replies);
                scan.cursors.push(read.cursor);
            }
            Ok(None) => {}
            Err(problem) => scan.problems.push(problem),
        }
    }
    Ok(scan)
}

/// `names` below `home`, only if each is a folder of its own and not a link: a sandboxed
/// agent writes its home and could point `.claude` anywhere. `None` when one is missing.
fn plain_folder(home: &Path, names: &[&str]) -> Result<Option<PathBuf>, ProviderError> {
    let mut folder = home.to_path_buf();
    for name in names {
        folder.push(name);
        match fs::symlink_metadata(&folder) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ProviderError {
                    reason: format!("cannot look at {}: {error}", folder.display()),
                });
            }
        }
    }
    Ok(Some(folder))
}

fn find_transcripts(
    folder: &Path,
    depth: usize,
    found: &mut Vec<PathBuf>,
    problems: &mut Vec<String>,
) {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(format!("cannot list {}: {error}", folder.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && depth < MAX_DEPTH {
            find_transcripts(&path, depth + 1, found, problems);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            found.push(path);
        }
    }
}

struct Read {
    replies: Vec<ModelReply>,
    cursor: SourceCursor,
}

/// The replies of `transcript` from byte `counted_to`, or `None` when nothing came after it.
/// A transcript shorter than the count was replaced, so it is read again from its start: the
/// archive keeps one row per reply key, and a replayed reply changes nothing.
fn read_from(transcript: &Path, source: &str, counted_to: u64) -> Result<Option<Read>, String> {
    let problem = |error: &dyn std::fmt::Display| format!("cannot read {source}: {error}");
    let length = fs::symlink_metadata(transcript)
        .map_err(|error| problem(&error))?
        .len();
    if length == counted_to {
        return Ok(None);
    }
    let start = if length < counted_to { 0 } else { counted_to };
    let mut file = File::open(transcript).map_err(|error| problem(&error))?;
    if !file.metadata().map_err(|error| problem(&error))?.is_file() {
        return Err(problem(&"it is not a file"));
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|error| problem(&error))?;
    let mut reader = BufReader::new(file);
    let mut replies = Vec::new();
    let mut offset = start;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| problem(&error))?;
        // A line without its newline is still being written: it is counted by the next scan.
        if read == 0 || line.last() != Some(&b'\n') {
            break;
        }
        replies.extend(reply_of(&line, source, offset));
        offset += read as u64;
    }
    if offset == counted_to {
        return Ok(None);
    }
    let cursor = SourceCursor::new(source, offset).map_err(|error| problem(&error))?;
    Ok(Some(Read { replies, cursor }))
}

/// The reply a transcript line records, if it records one that cost tokens: an assistant
/// message with a `usage` object, a model and a timestamp.
fn reply_of(line: &[u8], source: &str, offset: u64) -> Option<ModelReply> {
    if !line.windows(7).any(|window| window == b"\"usage\"") {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(line).ok()?;
    let message = record.get("message")?;
    let usage = message.get("usage").filter(|usage| usage.is_object())?;
    let model = message.get("model")?.as_str()?;
    // `<synthetic>` replies are the harness's own text, never a model call.
    if model.is_empty() || model.starts_with('<') {
        return None;
    }
    let at = parse_utc_seconds(record.get("timestamp")?.as_str()?)?;
    let tokens = tokens_of(usage);
    let total = tokens.input
        + tokens.output
        + tokens.cache_read
        + tokens.cache_write_5m
        + tokens.cache_write_1h;
    if total == 0 {
        return None;
    }
    // Claude Code writes one line per content block of a message, each with the message's
    // ids, so the ids name the reply. Without either, the line's own place does.
    let id = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let (message_id, request_id) = (id(message.get("id")), id(record.get("requestId")));
    let key = if message_id.is_empty() && request_id.is_empty() {
        format!("line:{source}:{offset}")
    } else {
        format!("{message_id}:{request_id}")
    };
    let working_dir = record
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_owned);
    ModelReply::new(key, at, model, working_dir, tokens).ok()
}

fn tokens_of(usage: &serde_json::Value) -> ReplyTokens {
    let count =
        |value: Option<&serde_json::Value>| value.and_then(serde_json::Value::as_u64).unwrap_or(0);
    let written = count(usage.get("cache_creation_input_tokens"));
    let by_lifetime = usage.get("cache_creation");
    let written_1h =
        count(by_lifetime.and_then(|lifetimes| lifetimes.get("ephemeral_1h_input_tokens")));
    let written_5m =
        match by_lifetime.and_then(|lifetimes| lifetimes.get("ephemeral_5m_input_tokens")) {
            Some(five_minutes) => five_minutes.as_u64().unwrap_or(0),
            None => written.saturating_sub(written_1h),
        };
    ReplyTokens {
        input: count(usage.get("input_tokens")),
        output: count(usage.get("output_tokens")),
        cache_read: count(usage.get("cache_read_input_tokens")),
        cache_write_5m: written_5m,
        cache_write_1h: written_1h,
        is_fast: usage.get("speed").and_then(serde_json::Value::as_str) == Some("fast"),
    }
}

/// `2026-11-02T10:00:00.123Z`, or with a `+HH:MM` offset, as Unix seconds. Claude Code writes
/// UTC with a `Z`; a timestamp in any other shape is not a reply this scan can place in a day.
fn parse_utc_seconds(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let (clock, offset_seconds) = if let Some(clock) = rest.strip_suffix('Z') {
        (clock, 0)
    } else {
        let sign_at = rest.rfind(['+', '-'])?;
        let (clock, offset) = rest.split_at(sign_at);
        let (hours, minutes) = offset[1..].split_once(':')?;
        let seconds = hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60;
        (
            clock,
            if offset.starts_with('-') {
                -seconds
            } else {
                seconds
            },
        )
    };
    let whole_clock = clock.split('.').next()?;
    let mut clock_parts = whole_clock.splitn(3, ':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    let is_valid = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (0..24).contains(&hour)
        && (0..60).contains(&minute)
        && (0..=60).contains(&second);
    if !is_valid {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
            - offset_seconds,
    )
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_utc_timestamp_is_its_unix_seconds() {
        assert_eq!(parse_utc_seconds("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_utc_seconds("2026-11-02T10:00:00.123Z"),
            Some(1_793_613_600)
        );
    }

    #[test]
    fn an_offset_timestamp_is_the_same_instant_in_utc() {
        assert_eq!(
            parse_utc_seconds("2026-11-02T12:00:00+02:00"),
            parse_utc_seconds("2026-11-02T10:00:00Z")
        );
        assert_eq!(
            parse_utc_seconds("2026-11-02T05:00:00-05:00"),
            parse_utc_seconds("2026-11-02T10:00:00Z")
        );
    }

    #[test]
    fn a_timestamp_out_of_range_is_not_a_time() {
        assert_eq!(parse_utc_seconds("2026-13-02T10:00:00Z"), None);
        assert_eq!(parse_utc_seconds("yesterday"), None);
    }
}
