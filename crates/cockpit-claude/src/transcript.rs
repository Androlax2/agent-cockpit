//! The context sizes of a compaction (#782) and at a turn's end (#479), read from the
//! transcript a hook payload names (`transcript_path`). The `PreCompact`, `PostCompact` and
//! `Stop` payloads carry no sizes, and Claude Code writes the transcript asynchronously, so a
//! size the transcript does not hold yet is `None`, never a guess.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of the transcript's end is read: the last model call and the compaction
/// boundary sit in it, while a whole transcript can be many megabytes.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// The last main-chain model call's context: its input, cache read and cache creation tokens,
/// the formula report 686 measured with. It is the context before a compaction, and at a
/// turn's end the harness's own total.
pub fn context_tokens(transcript: &Path) -> Option<u64> {
    last_record(transcript, model_call_tokens)
}

/// The harness's total at a turn's end: [`context_tokens`], or `None` when a compaction
/// boundary came after that model call, as that size is no longer the context.
pub fn turn_context(transcript: &Path) -> Option<u64> {
    tail(transcript)?
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|record| {
            if is_compact_boundary(&record) {
                return Some(None);
            }
            model_call_tokens(&record).map(Some)
        })
        .flatten()
}

fn model_call_tokens(record: &serde_json::Value) -> Option<u64> {
    if record["type"] != "assistant" || record["isSidechain"] == true {
        return None;
    }
    let message = &record["message"];
    if message["model"] == "<synthetic>" {
        return None;
    }
    let usage = &message["usage"];
    let tokens: u64 = [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .map(|field| usage[*field].as_u64().unwrap_or(0))
    .sum();
    (tokens > 0).then_some(tokens)
}

/// The `apiError` Claude Code tagged the turn's end with (#718): set only when the last
/// main-chain message, the user's or the assistant's, is Claude Code's own API error. A later
/// prompt, or an error message not written yet, gives `None`.
pub fn turn_api_error(transcript: &Path) -> Option<String> {
    let last_message = tail(transcript)?
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| {
            matches!(record["type"].as_str(), Some("user" | "assistant"))
                && record["isSidechain"] != true
        })?;
    if last_message["type"] != "assistant" || last_message["isApiErrorMessage"] != true {
        return None;
    }
    last_message["apiError"].as_str().map(str::to_owned)
}

fn is_compact_boundary(record: &serde_json::Value) -> bool {
    record["type"] == "system" && record["subtype"] == "compact_boundary"
}

/// The context after a compaction: `postTokens` of the latest `compact_boundary` record.
pub fn tokens_after(transcript: &Path) -> Option<u64> {
    last_record(transcript, |record| {
        if !is_compact_boundary(record) {
            return None;
        }
        record["compactMetadata"]["postTokens"].as_u64()
    })
}

/// What `pick` returns for the last record of the transcript's tail that it accepts.
fn last_record(transcript: &Path, pick: impl Fn(&serde_json::Value) -> Option<u64>) -> Option<u64> {
    tail(transcript)?
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|record| pick(&record))
}

/// The transcript's last [`TAIL_BYTES`], from the first whole line.
fn tail(transcript: &Path) -> Option<String> {
    let mut file = File::open(transcript).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if start == 0 {
        return Some(text);
    }
    let first_line_end = text.find('\n')?;
    Some(text[first_line_end + 1..].to_owned())
}
