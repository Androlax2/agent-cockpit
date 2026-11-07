//! The context sizes of a compaction (#782) and at a turn's end (#479), read from the rollout
//! file a hook payload names (`transcript_path`). The `PreCompact`, `PostCompact` and `Stop`
//! payloads carry no sizes; a size the rollout does not hold yet is `None`, never a guess.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use cockpit_provider_api::ContextUsage;

/// How much of the rollout's end is read; a whole rollout can be many megabytes.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// The context before a compaction: the input tokens of the last model call's
/// `token_count` event (`info.last_token_usage.input_tokens`, report 686's formula).
pub fn tokens_before(rollout: &Path) -> Option<u64> {
    records(rollout)?.iter().rev().find_map(context_tokens)
}

/// The harness's context at a turn's end: the last `token_count` event's input tokens, as
/// [`tokens_before`] counts them, with the window it names (`info.model_context_window`).
pub fn context_usage(rollout: &Path) -> Option<ContextUsage> {
    let records = records(rollout)?;
    let last = records
        .iter()
        .rev()
        .find(|record| context_tokens(record).is_some())?;
    Some(ContextUsage {
        tokens: context_tokens(last)?,
        window: last["payload"]["info"]["model_context_window"]
            .as_u64()
            .filter(|window| *window > 0),
    })
}

/// The context after a compaction: the first `token_count` event that follows the latest
/// `compacted` record. `None` until Codex has made a model call since.
pub fn tokens_after(rollout: &Path) -> Option<u64> {
    let records = records(rollout)?;
    let compacted = records
        .iter()
        .rposition(|record| record["type"] == "compacted")?;
    records[compacted + 1..].iter().find_map(context_tokens)
}

fn context_tokens(record: &serde_json::Value) -> Option<u64> {
    let payload = &record["payload"];
    if record["type"] != "event_msg" || payload["type"] != "token_count" {
        return None;
    }
    payload["info"]["last_token_usage"]["input_tokens"]
        .as_u64()
        .filter(|tokens| *tokens > 0)
}

/// The records of the rollout's last [`TAIL_BYTES`], from the first whole line, oldest first.
fn records(rollout: &Path) -> Option<Vec<serde_json::Value>> {
    let mut file = File::open(rollout).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let whole_lines = if start == 0 {
        &text[..]
    } else {
        &text[text.find('\n')? + 1..]
    };
    Some(
        whole_lines
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect(),
    )
}
