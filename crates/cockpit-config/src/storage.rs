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
    /// Check output older than this many days is retired (#406).
    pub output_retention_days: Sourced<u32>,
    /// Per project: past this, the oldest check output beyond the grace period is retired.
    pub output_budget_mib: Sourced<u64>,
    pub output_budget_grace_hours: Sourced<u32>,
    pub output_prune_automatic: Sourced<bool>,
    pub output_prune_projects: Sourced<Vec<String>>,
    pub output_prune_interval_minutes: Sourced<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StorageTable {
    target_prune_automatic: Option<Spanned<bool>>,
    target_prune_projects: Option<Spanned<Vec<String>>>,
    target_prune_confirm_above_mib: Option<Spanned<u64>>,
    target_prune_interval_minutes: Option<Spanned<u32>>,
    target_prune_base_ref: Option<Spanned<String>>,
    output_retention_days: Option<Spanned<u32>>,
    output_budget_mib: Option<Spanned<u64>>,
    output_budget_grace_hours: Option<Spanned<u32>>,
    output_prune_automatic: Option<Spanned<bool>>,
    output_prune_projects: Option<Spanned<Vec<String>>>,
    output_prune_interval_minutes: Option<Spanned<u32>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<StorageSettings, ConfigError> {
    let mut result = StorageSettings {
        automatic: Sourced::new(false, Source::ProductDefault),
        projects: Sourced::new(Vec::new(), Source::ProductDefault),
        confirm_above_mib: Sourced::new(1024, Source::ProductDefault),
        interval_minutes: Sourced::new(60, Source::ProductDefault),
        base_ref: Sourced::new(None, Source::ProductDefault),
        output_retention_days: Sourced::new(14, Source::ProductDefault),
        output_budget_mib: Sourced::new(1024, Source::ProductDefault),
        output_budget_grace_hours: Sourced::new(24, Source::ProductDefault),
        output_prune_automatic: Sourced::new(false, Source::ProductDefault),
        output_prune_projects: Sourced::new(Vec::new(), Source::ProductDefault),
        output_prune_interval_minutes: Sourced::new(60, Source::ProductDefault),
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
        if let Some(value) = &table.output_retention_days {
            result.output_retention_days =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_budget_mib {
            result.output_budget_mib =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_budget_grace_hours {
            result.output_budget_grace_hours =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_prune_automatic {
            result.output_prune_automatic =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_prune_projects {
            result.output_prune_projects =
                Sourced::new(value.get_ref().clone(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.output_prune_interval_minutes {
            result.output_prune_interval_minutes =
                Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
    }
    let invalid = |key: &str, source: &Source, reason: &str| ConfigError::Invalid {
        key: format!("storage.{key}"),
        source: source.clone(),
        reason: reason.to_owned(),
    };
    if !is_stored_mib(result.confirm_above_mib.value) {
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
    for (key, projects) in [
        ("target_prune_projects", &result.projects),
        ("output_prune_projects", &result.output_prune_projects),
    ] {
        if !is_project_allowlist(&projects.value) {
            return Err(invalid(
                key,
                &projects.source,
                "must list at most 128 plain registered project names",
            ));
        }
    }
    if result.output_retention_days.value == 0 {
        return Err(invalid(
            "output_retention_days",
            &result.output_retention_days.source,
            "must be at least 1 day",
        ));
    }
    if !is_stored_mib(result.output_budget_mib.value) {
        return Err(invalid(
            "output_budget_mib",
            &result.output_budget_mib.source,
            "must be positive and fit the stored byte range",
        ));
    }
    if result.output_prune_interval_minutes.value == 0 {
        return Err(invalid(
            "output_prune_interval_minutes",
            &result.output_prune_interval_minutes.source,
            "must be at least 1 minute",
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

fn is_stored_mib(mib: u64) -> bool {
    mib != 0
        && mib
            .checked_mul(1024 * 1024)
            .is_some_and(|bytes| i64::try_from(bytes).is_ok())
}

fn is_project_allowlist(projects: &[String]) -> bool {
    projects.len() <= 128
        && projects.iter().all(|name| {
            !name.is_empty()
                && name.len() <= 64
                && !name.starts_with(['.', '-'])
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
