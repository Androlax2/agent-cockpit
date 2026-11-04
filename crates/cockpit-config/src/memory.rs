//! Owner-only bounds on what the daemon and its tmux server hold in memory (#406). Each is
//! read at the daemon's start.
use crate::{ConfigError, Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

/// The fewest history lines the tmux server may keep: an exited agent's last lines are read
/// from them.
const TMUX_HISTORY_LINES_MIN: u32 = 200;
const TMUX_HISTORY_LINES_MAX: u32 = 1_000_000;
const FEED_BUFFER_KIB_MIN: u32 = 64;
const FEED_BUFFER_KIB_MAX: u32 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct MemorySettings {
    /// Lines of history each pane of the private tmux server keeps.
    pub tmux_history_lines: Sourced<u32>,
    /// Change batches waiting for one subscriber that reads slower than the daemon commits.
    pub feed_buffer_kib: Sourced<u32>,
    /// How long a running session may have no agent in its terminal before it is reported.
    pub agentless_session_minutes: Sourced<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemoryTable {
    tmux_history_lines: Option<Spanned<u32>>,
    feed_buffer_kib: Option<Spanned<u32>>,
    agentless_session_minutes: Option<Spanned<u32>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<MemorySettings, ConfigError> {
    let mut result = MemorySettings {
        tmux_history_lines: Sourced::new(10_000, Source::ProductDefault),
        feed_buffer_kib: Sourced::new(1024, Source::ProductDefault),
        agentless_session_minutes: Sourced::new(10, Source::ProductDefault),
    };
    for layer in layers {
        let Some(table) = &layer.settings.memory else {
            continue;
        };
        let table = table.get_ref();
        let sourced =
            |value: &Spanned<u32>| Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        if let Some(value) = &table.tmux_history_lines {
            result.tmux_history_lines = sourced(value);
        }
        if let Some(value) = &table.feed_buffer_kib {
            result.feed_buffer_kib = sourced(value);
        }
        if let Some(value) = &table.agentless_session_minutes {
            result.agentless_session_minutes = sourced(value);
        }
    }
    let invalid = |key: &str, source: &Source, reason: String| ConfigError::Invalid {
        key: format!("memory.{key}"),
        source: source.clone(),
        reason,
    };
    if !(TMUX_HISTORY_LINES_MIN..=TMUX_HISTORY_LINES_MAX).contains(&result.tmux_history_lines.value)
    {
        return Err(invalid(
            "tmux_history_lines",
            &result.tmux_history_lines.source,
            format!("must be {TMUX_HISTORY_LINES_MIN} to {TMUX_HISTORY_LINES_MAX} lines"),
        ));
    }
    if !(FEED_BUFFER_KIB_MIN..=FEED_BUFFER_KIB_MAX).contains(&result.feed_buffer_kib.value) {
        return Err(invalid(
            "feed_buffer_kib",
            &result.feed_buffer_kib.source,
            format!("must be {FEED_BUFFER_KIB_MIN} to {FEED_BUFFER_KIB_MAX} KiB"),
        ));
    }
    if result.agentless_session_minutes.value == 0 {
        return Err(invalid(
            "agentless_session_minutes",
            &result.agentless_session_minutes.source,
            "must be at least 1 minute".to_owned(),
        ));
    }
    Ok(result)
}
