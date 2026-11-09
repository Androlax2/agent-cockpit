//! Owner-only: how often the daemon reads each provider's plan windows, how long a reading
//! counts, at which shares used it alerts (#58, report 18), and how queue starts follow the
//! windows (#412). Read at the daemon's start.
use std::collections::BTreeMap;

use crate::{ConfigError, Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

const REFRESH_MINUTES_MAX: u32 = 24 * 60;
/// As many as `cockpit_usage::QuotaPolicy` takes.
const ALERT_PERCENTS_MAX: usize = 5;
/// The queue keeps at least a tenth of a window for itself.
const PACING_RESERVE_PERCENT_MAX: u8 = 90;
const PACING_BURST_PERCENT_MAX: u8 = 100;

/// How queue starts follow the plan windows of each session's account (#412).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pacing {
    /// Spend each window evenly until its reset, keeping the reserve.
    Even,
    /// Start queued work whatever the windows show.
    Off,
}

impl Pacing {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Even => "even",
            Self::Off => "off",
        }
    }
}

/// The model and effort a project's economy sessions start with (#412), in the provider's own
/// words: an item marked economy goes only to a session of the project started with them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Economy {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageSettings {
    /// Minutes between two readings of one provider's windows while they succeed.
    pub quota_refresh_minutes: Sourced<u32>,
    /// How long a reading counts as current; never shorter than the refresh.
    pub quota_fresh_minutes: Sourced<u32>,
    /// The shares used, in percent and ascending, at which a window alerts once.
    pub quota_alert_percents: Sourced<Vec<u8>>,
    pub pacing: Sourced<Pacing>,
    /// The share of each window the queue never starts work in: the owner's own sessions'.
    pub pacing_reserve_percent: Sourced<u8>,
    /// How far ahead of the even line normal-priority work may start, so work starts when a
    /// window opens.
    pub pacing_burst_percent: Sourced<u8>,
    /// The most of each plan window a project's queued work may use, in percent, by project
    /// name; a project not listed has no cap of its own (#412).
    pub project_shares: BTreeMap<String, Sourced<u8>>,
    /// Each project's economy model and effort, by project name; a project not listed runs
    /// no economy, and its items marked economy wait (#412).
    pub economy: BTreeMap<String, Sourced<Economy>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UsageTable {
    quota_refresh_minutes: Option<Spanned<u32>>,
    quota_fresh_minutes: Option<Spanned<u32>>,
    quota_alert_percents: Option<Spanned<Vec<u8>>>,
    pacing: Option<Spanned<Pacing>>,
    pacing_reserve_percent: Option<Spanned<u8>>,
    pacing_burst_percent: Option<Spanned<u8>>,
    project_shares: Option<BTreeMap<String, Spanned<u8>>>,
    economy: Option<BTreeMap<String, Spanned<Economy>>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<UsageSettings, ConfigError> {
    let mut result = UsageSettings {
        quota_refresh_minutes: Sourced::new(5, Source::ProductDefault),
        quota_fresh_minutes: Sourced::new(10, Source::ProductDefault),
        quota_alert_percents: Sourced::new(vec![80, 95], Source::ProductDefault),
        // Owner decision 2026-11-09 (#412).
        pacing: Sourced::new(Pacing::Even, Source::ProductDefault),
        pacing_reserve_percent: Sourced::new(10, Source::ProductDefault),
        pacing_burst_percent: Sourced::new(15, Source::ProductDefault),
        project_shares: BTreeMap::new(),
        economy: BTreeMap::new(),
    };
    for layer in layers {
        let Some(table) = &layer.settings.usage else {
            continue;
        };
        let table = table.get_ref();
        let sourced =
            |value: &Spanned<u32>| Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        if let Some(value) = &table.quota_refresh_minutes {
            result.quota_refresh_minutes = sourced(value);
        }
        if let Some(value) = &table.quota_fresh_minutes {
            result.quota_fresh_minutes = sourced(value);
        }
        if let Some(value) = &table.quota_alert_percents {
            result.quota_alert_percents =
                Sourced::new(value.get_ref().clone(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.pacing {
            result.pacing = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        let percent =
            |value: &Spanned<u8>| Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        if let Some(value) = &table.pacing_reserve_percent {
            result.pacing_reserve_percent = percent(value);
        }
        if let Some(value) = &table.pacing_burst_percent {
            result.pacing_burst_percent = percent(value);
        }
        for (project, share) in table.project_shares.iter().flatten() {
            result
                .project_shares
                .insert(project.clone(), percent(share));
        }
        for (project, economy) in table.economy.iter().flatten() {
            result.economy.insert(
                project.clone(),
                Sourced::new(economy.get_ref().clone(), layer.source_at(economy.span())),
            );
        }
    }
    let invalid = |key: &str, source: &Source, reason: String| ConfigError::Invalid {
        key: format!("usage.{key}"),
        source: source.clone(),
        reason,
    };
    let refresh = &result.quota_refresh_minutes;
    if !(1..=REFRESH_MINUTES_MAX).contains(&refresh.value) {
        return Err(invalid(
            "quota_refresh_minutes",
            &refresh.source,
            format!("must be 1 to {REFRESH_MINUTES_MAX} minutes"),
        ));
    }
    let fresh = &result.quota_fresh_minutes;
    if fresh.value < refresh.value {
        return Err(invalid(
            "quota_fresh_minutes",
            &fresh.source,
            format!(
                "must be at least `quota_refresh_minutes` ({}): a reading would go stale \
                 before the next",
                refresh.value
            ),
        ));
    }
    let percents = &result.quota_alert_percents;
    let is_ascending = percents.value.windows(2).all(|pair| pair[0] < pair[1]);
    if percents.value.is_empty()
        || percents.value.len() > ALERT_PERCENTS_MAX
        || !is_ascending
        || percents
            .value
            .iter()
            .any(|percent| !(1..=100).contains(percent))
    {
        return Err(invalid(
            "quota_alert_percents",
            &percents.source,
            format!("must be 1 to {ALERT_PERCENTS_MAX} percents from 1 to 100, ascending"),
        ));
    }
    for (key, percent, max) in [
        (
            "pacing_reserve_percent",
            &result.pacing_reserve_percent,
            PACING_RESERVE_PERCENT_MAX,
        ),
        (
            "pacing_burst_percent",
            &result.pacing_burst_percent,
            PACING_BURST_PERCENT_MAX,
        ),
    ] {
        if percent.value > max {
            return Err(invalid(
                key,
                &percent.source,
                format!("must be 0 to {max} percent"),
            ));
        }
    }
    for (project, share) in &result.project_shares {
        if !is_project_name(project) {
            return Err(invalid(
                &format!("project_shares.{project}"),
                &share.source,
                NOT_A_PROJECT_NAME.to_owned(),
            ));
        }
        if !(1..=100).contains(&share.value) {
            return Err(invalid(
                &format!("project_shares.{project}"),
                &share.source,
                "must be 1 to 100 percent".to_owned(),
            ));
        }
    }
    for (project, economy) in &result.economy {
        let key = format!("economy.{project}");
        if !is_project_name(project) {
            return Err(invalid(
                &key,
                &economy.source,
                NOT_A_PROJECT_NAME.to_owned(),
            ));
        }
        let Economy { model, effort } = &economy.value;
        if model.is_none() && effort.is_none() {
            return Err(invalid(
                &key,
                &economy.source,
                "names a `model`, an `effort` or both".to_owned(),
            ));
        }
        for (field, word) in [("model", model), ("effort", effort)] {
            if word.as_deref().is_some_and(|word| !is_provider_word(word)) {
                return Err(invalid(
                    &format!("{key}.{field}"),
                    &economy.source,
                    "must be 1 to 64 characters, no spaces or control characters".to_owned(),
                ));
            }
        }
    }
    Ok(result)
}

const NOT_A_PROJECT_NAME: &str = "is not a project name: 1 to 64 ASCII letters, digits, `.`, `_` \
     and `-`, not starting with `.` or `-`";

/// A model or effort as a session's start takes it: the economy matches it word for word.
fn is_provider_word(word: &str) -> bool {
    (1..=64).contains(&word.chars().count())
        && !word
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

/// `cockpit-projects`' rule for a project's name, so a setting no project could carry is
/// refused instead of applying to nothing.
fn is_project_name(name: &str) -> bool {
    let is_plain =
        |character: char| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-');
    (1..=64).contains(&name.len()) && !name.starts_with(['.', '-']) && name.chars().all(is_plain)
}
