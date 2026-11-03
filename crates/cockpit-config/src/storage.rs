//! Owner-only storage policy; loading a value does not activate a cleanup operation.
use crate::{ConfigError, Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

#[derive(Debug, Clone)]
pub struct StorageSettings {
    pub automatic: Sourced<bool>,
    pub projects: Sourced<Vec<String>>,
    pub confirm_above_mib: Sourced<u64>,
    pub interval_minutes: Sourced<u32>,
    pub base_ref: Sourced<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StorageTable {
    target_prune_automatic: Option<Spanned<bool>>,
    target_prune_projects: Option<Spanned<Vec<String>>>,
    target_prune_confirm_above_mib: Option<Spanned<u64>>,
    target_prune_interval_minutes: Option<Spanned<u32>>,
    target_prune_base_ref: Option<Spanned<String>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<StorageSettings, ConfigError> {
    let mut result = StorageSettings {
        automatic: Sourced::new(false, Source::ProductDefault),
        projects: Sourced::new(Vec::new(), Source::ProductDefault),
        confirm_above_mib: Sourced::new(1024, Source::ProductDefault),
        interval_minutes: Sourced::new(60, Source::ProductDefault),
        base_ref: Sourced::new(None, Source::ProductDefault),
    };
    for layer in layers {
        let Some(table) = &layer.settings.storage else {
            continue;
        };
        let table = table.get_ref();
        if let Some(value) = &table.target_prune_automatic {
            result.automatic = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_projects {
            result.projects = Sourced::new(value.get_ref().clone(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_confirm_above_mib {
            result.confirm_above_mib =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_interval_minutes {
            result.interval_minutes = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.target_prune_base_ref {
            result.base_ref =
                Sourced::new(Some(value.get_ref().clone()), layer.source_at(value.span()));
        }
    }
    let invalid = |key: &str, source: &Source, reason: &str| ConfigError::Invalid {
        key: format!("storage.{key}"),
        source: source.clone(),
        reason: reason.to_owned(),
    };
    if result.confirm_above_mib.value == 0
        || !result
            .confirm_above_mib
            .value
            .checked_mul(1024 * 1024)
            .is_some_and(|bytes| i64::try_from(bytes).is_ok())
    {
        return Err(invalid(
            "target_prune_confirm_above_mib",
            &result.confirm_above_mib.source,
            "must be positive and fit the stored byte range",
        ));
    }
    if result.interval_minutes.value == 0 {
        return Err(invalid(
            "target_prune_interval_minutes",
            &result.interval_minutes.source,
            "must be at least 1 minute",
        ));
    }
    if result.projects.value.len() > 128
        || result.projects.value.iter().any(|name| {
            name.is_empty()
                || name.len() > 64
                || name.starts_with(['.', '-'])
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(invalid(
            "target_prune_projects",
            &result.projects.source,
            "must list at most 128 plain registered project names",
        ));
    }
    if let Some(base) = &result.base_ref.value
        && (!base.starts_with("refs/")
            || base.len() > 1024
            || base.chars().any(|c| c.is_control() || c.is_whitespace()))
    {
        return Err(invalid(
            "target_prune_base_ref",
            &result.base_ref.source,
            "must be an explicit full LOCAL ref, such as refs/heads/main",
        ));
    }
    Ok(result)
}
