//! Owner-only: how often the daemon reads each provider's plan windows, how long a reading
//! counts, and at which shares used it alerts (#58, report 18). Read at the daemon's start.
use crate::{ConfigError, Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

const REFRESH_MINUTES_MAX: u32 = 24 * 60;
/// As many as `cockpit_usage::QuotaPolicy` takes.
const ALERT_PERCENTS_MAX: usize = 5;

#[derive(Debug, Clone)]
pub struct UsageSettings {
    /// Minutes between two readings of one provider's windows while they succeed.
    pub quota_refresh_minutes: Sourced<u32>,
    /// How long a reading counts as current; never shorter than the refresh.
    pub quota_fresh_minutes: Sourced<u32>,
    /// The shares used, in percent and ascending, at which a window alerts once.
    pub quota_alert_percents: Sourced<Vec<u8>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UsageTable {
    quota_refresh_minutes: Option<Spanned<u32>>,
    quota_fresh_minutes: Option<Spanned<u32>>,
    quota_alert_percents: Option<Spanned<Vec<u8>>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<UsageSettings, ConfigError> {
    let mut result = UsageSettings {
        quota_refresh_minutes: Sourced::new(5, Source::ProductDefault),
        quota_fresh_minutes: Sourced::new(10, Source::ProductDefault),
        quota_alert_percents: Sourced::new(vec![80, 95], Source::ProductDefault),
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
    Ok(result)
}
