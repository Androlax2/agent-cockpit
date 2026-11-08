//! `config show`: every effective value with its source, one `key = value  # source` per line.

use std::path::Path;
use std::time::Duration;

use toml::{Table, Value};

use crate::{
    ASLEEP_EXPIRY_DAYS, AgentSandbox, ConfigFile, DesktopSession, EffectiveConfig,
    IDLE_SLEEP_AFTER_MINUTES, Source,
};

const UNSET: &str = "<unset>";

pub fn render(config: &EffectiveConfig) -> String {
    let layout = &config.layout;
    let mut lines: Vec<String> = [
        ("paths.config_dir", &layout.config_dir),
        ("paths.state_dir", &layout.state_dir),
        ("paths.cache_dir", &layout.cache_dir),
    ]
    .into_iter()
    .map(|(key, dir)| line(key, &path_toml(&dir.value).to_string(), &dir.source))
    .collect();
    lines.insert(
        0,
        line(
            "profile",
            &Value::from(layout.profile.value.name()).to_string(),
            &layout.profile.source,
        ),
    );
    lines.push(line(
        "paths.runtime_dir",
        &layout
            .runtime_dir
            .value
            .as_deref()
            .map_or(UNSET.to_string(), |dir| path_toml(dir).to_string()),
        &layout.runtime_dir.source,
    ));
    lines.push(file_line("files.user_config", &config.user_file));
    if let Some(project_file) = &config.project_file {
        lines.push(file_line("files.project_config", project_file));
    }
    lines.push(line(
        "projects.trusted",
        &Value::Array(
            config
                .trusted_projects
                .value
                .iter()
                .map(|path| path_toml(path))
                .collect(),
        )
        .to_string(),
        &config.trusted_projects.source,
    ));
    let storage = &config.storage;
    lines.push(line(
        "storage.target_prune_automatic",
        &storage.automatic.value.to_string(),
        &storage.automatic.source,
    ));
    lines.push(line(
        "storage.target_prune_projects",
        &Value::Array(
            storage
                .projects
                .value
                .iter()
                .cloned()
                .map(Value::from)
                .collect(),
        )
        .to_string(),
        &storage.projects.source,
    ));
    lines.push(line(
        "storage.target_prune_confirm_above_mib",
        &storage.confirm_above_mib.value.to_string(),
        &storage.confirm_above_mib.source,
    ));
    lines.push(line(
        "storage.target_prune_interval_minutes",
        &storage.interval_minutes.value.to_string(),
        &storage.interval_minutes.source,
    ));
    lines.push(line(
        "storage.target_prune_base_ref",
        &storage
            .base_ref
            .value
            .as_ref()
            .map_or(UNSET.to_owned(), |v| Value::from(v.as_str()).to_string()),
        &storage.base_ref.source,
    ));
    lines.push(line(
        "storage.output_retention_days",
        &storage.output_retention_days.value.to_string(),
        &storage.output_retention_days.source,
    ));
    lines.push(line(
        "storage.output_budget_mib",
        &storage.output_budget_mib.value.to_string(),
        &storage.output_budget_mib.source,
    ));
    lines.push(line(
        "storage.output_budget_grace_hours",
        &storage.output_budget_grace_hours.value.to_string(),
        &storage.output_budget_grace_hours.source,
    ));
    lines.push(line(
        "storage.output_prune_automatic",
        &storage.output_prune_automatic.value.to_string(),
        &storage.output_prune_automatic.source,
    ));
    lines.push(line(
        "storage.output_prune_projects",
        &Value::Array(
            storage
                .output_prune_projects
                .value
                .iter()
                .cloned()
                .map(Value::from)
                .collect(),
        )
        .to_string(),
        &storage.output_prune_projects.source,
    ));
    lines.push(line(
        "storage.output_prune_interval_minutes",
        &storage.output_prune_interval_minutes.value.to_string(),
        &storage.output_prune_interval_minutes.source,
    ));
    lines.push(line(
        "storage.sweep_on_session_end",
        &storage.sweep_on_session_end.value.to_string(),
        &storage.sweep_on_session_end.source,
    ));
    let memory = &config.memory;
    for (key, bound) in [
        ("memory.tmux_history_lines", &memory.tmux_history_lines),
        ("memory.feed_buffer_kib", &memory.feed_buffer_kib),
        (
            "memory.agentless_session_minutes",
            &memory.agentless_session_minutes,
        ),
    ] {
        lines.push(line(key, &bound.value.to_string(), &bound.source));
    }
    let usage = &config.usage;
    for (key, minutes) in [
        ("usage.quota_refresh_minutes", &usage.quota_refresh_minutes),
        ("usage.quota_fresh_minutes", &usage.quota_fresh_minutes),
    ] {
        lines.push(line(key, &minutes.value.to_string(), &minutes.source));
    }
    lines.push(line(
        "usage.quota_alert_percents",
        &format!("{:?}", usage.quota_alert_percents.value),
        &usage.quota_alert_percents.source,
    ));
    for provider in crate::KNOWN_PROVIDERS {
        let accounts = config.accounts.sourced(provider);
        lines.push(line(
            &format!("accounts.{provider}"),
            &Value::Array(accounts.value.into_iter().map(Value::from).collect()).to_string(),
            &accounts.source,
        ));
    }
    lines.push(line(
        "storage.low_disk_floor_mib",
        &storage.low_disk_floor_mib.value.to_string(),
        &storage.low_disk_floor_mib.source,
    ));
    lines.push(line(
        "storage.database_retention_days",
        &storage.database_retention_days.value.to_string(),
        &storage.database_retention_days.source,
    ));
    lines.push(line(
        "storage.database_prune_automatic",
        &storage.database_prune_automatic.value.to_string(),
        &storage.database_prune_automatic.source,
    ));
    lines.push(line(
        "storage.database_maintenance_interval_minutes",
        &storage
            .database_maintenance_interval_minutes
            .value
            .to_string(),
        &storage.database_maintenance_interval_minutes.source,
    ));
    lines.push(line(
        "storage.database_backup_interval_hours",
        &storage.database_backup_interval_hours.value.to_string(),
        &storage.database_backup_interval_hours.source,
    ));
    lines.push(line(
        "storage.database_backups_kept",
        &storage.database_backups_kept.value.to_string(),
        &storage.database_backups_kept.source,
    ));
    lines.push(line(
        "storage.feed_history_changes",
        &storage.feed_history_changes.value.to_string(),
        &storage.feed_history_changes.source,
    ));
    lines.push(line(
        "workflow.preset",
        &config
            .workflow_preset
            .value
            .as_ref()
            .map_or(UNSET.to_string(), |name| {
                Value::from(name.as_str()).to_string()
            }),
        &config.workflow_preset.source,
    ));
    lines.push(line(
        "tasks.blocker_project",
        &config
            .tasks_blocker_project
            .value
            .as_ref()
            .map_or(UNSET.to_string(), |name| {
                Value::from(name.as_str()).to_string()
            }),
        &config.tasks_blocker_project.source,
    ));
    lines.push(line(
        "tasks.cross_project_open_max",
        &config.tasks_cross_project_open_max.value.to_string(),
        &config.tasks_cross_project_open_max.source,
    ));
    let session_settings = SessionSettings {
        idle_sleep_after: config.idle_sleep_after.value,
        asleep_expiry: config.asleep_expiry.value,
        desktop_session: config.agents_desktop_session.value,
        sandbox: config.agents_sandbox.value,
    };
    let sources = [
        &config.idle_sleep_after.source,
        &config.asleep_expiry.source,
        &config.agents_desktop_session.source,
        &config.agents_sandbox.source,
    ];
    for ((key, value), source) in session_settings.rendered().into_iter().zip(sources) {
        lines.push(line(key, &value, source));
    }
    lines.push(line(
        "tui.attention_bell",
        &config.tui_attention_bell.value.to_string(),
        &config.tui_attention_bell.source,
    ));
    lines.push(line(
        "tui.phone_below_columns",
        &config.tui_phone_below_columns.value.to_string(),
        &config.tui_phone_below_columns.source,
    ));
    lines.push(line(
        "notify.autostart",
        &config.notify.autostart.value.to_string(),
        &config.notify.autostart.source,
    ));
    for (outlet, events) in [
        ("desktop", &config.notify.desktop),
        ("sound", &config.notify.sound),
    ] {
        for (event, setting) in events.by_name() {
            lines.push(line(
                &format!("notify.{outlet}.{event}"),
                &setting.value.to_string(),
                &setting.source,
            ));
        }
    }
    for (event, file) in &config.notify.sound_files {
        lines.push(line(
            &format!("notify.sound.files.{event}"),
            &Value::from(file.value.display().to_string()).to_string(),
            &file.source,
        ));
    }
    let skill_pack = &config.skill_pack;
    lines.push(line(
        "skill_pack.enabled",
        &skill_pack.enabled.value.to_string(),
        &skill_pack.enabled.source,
    ));
    for (name, is_on) in &skill_pack.skills.value {
        lines.push(line(
            &format!("skill_pack.skills.{name}"),
            &is_on.to_string(),
            &skill_pack.skills.source,
        ));
    }
    lines.push(line(
        "skill_pack.grants_confirmed_at",
        &skill_pack
            .grants_confirmed_at
            .value
            .as_ref()
            .map_or(UNSET.to_string(), |commit| {
                Value::from(commit.as_str()).to_string()
            }),
        &skill_pack.grants_confirmed_at.source,
    ));
    for (name, program) in &config.notify.holds {
        lines.push(line(
            &format!("notify.hold.{name}"),
            &Value::from(program.value.display().to_string()).to_string(),
            &program.source,
        ));
    }
    // Before the start files and checks, which list last as they grow.
    lines.push(line(
        "execution.environment",
        &config
            .execution_environment
            .value
            .map_or(UNSET.to_string(), |environment| {
                Value::from(environment.as_str()).to_string()
            }),
        &config.execution_environment.source,
    ));
    let needs = config.needs.value;
    for (key, amount) in [
        ("needs.cpu_millis", needs.map(|needs| needs.cpu_millis)),
        ("needs.memory_mib", needs.map(|needs| needs.memory_mib)),
    ] {
        lines.push(line(
            key,
            &amount.map_or(UNSET.to_string(), |amount| amount.to_string()),
            &config.needs.source,
        ));
    }
    lines.push(line(
        "results.artifacts",
        &Value::from(config.result_artifacts.value.clone()).to_string(),
        &config.result_artifacts.source,
    ));
    for start_file in &config.start_files {
        let mut fields = Table::new();
        fields.insert("path".to_string(), path_toml(&start_file.value.path));
        fields.insert(
            "providers".to_string(),
            Value::from(start_file.value.providers.clone()),
        );
        lines.push(line(
            &format!(
                "start_files.{}",
                Value::from(start_file.value.label.as_str())
            ),
            &Value::Table(fields).to_string(),
            &start_file.source,
        ));
    }
    for check in &config.checks {
        let mut fields = Table::new();
        fields.insert(
            "command".to_string(),
            Value::from(check.value.command.as_str()),
        );
        fields.insert(
            "working_directory".to_string(),
            Value::from(check.value.working_directory.as_str()),
        );
        fields.insert(
            "environment".to_string(),
            Value::from(check.value.environment.as_str()),
        );
        fields.insert(
            "artifacts".to_string(),
            Value::from(check.value.artifacts.clone()),
        );
        lines.push(line(
            &format!("checks.{}", Value::from(check.value.name.as_str())),
            &Value::Table(fields).to_string(),
            &check.source,
        ));
    }
    lines.join("\n") + "\n"
}

/// The owner settings a running daemon holds from its start until `cockpit config reload`
/// (#354), the others it reads at each use or only at its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSettings {
    pub idle_sleep_after: Duration,
    pub asleep_expiry: Duration,
    pub desktop_session: DesktopSession,
    pub sandbox: AgentSandbox,
}

impl SessionSettings {
    /// Each setting's key and value as the owner's file writes them, in `config show`'s order.
    pub fn rendered(&self) -> [(&'static str, String); 4] {
        [
            (
                IDLE_SLEEP_AFTER_MINUTES.key,
                IDLE_SLEEP_AFTER_MINUTES
                    .count(self.idle_sleep_after)
                    .to_string(),
            ),
            (
                ASLEEP_EXPIRY_DAYS.key,
                ASLEEP_EXPIRY_DAYS.count(self.asleep_expiry).to_string(),
            ),
            (
                "agents.desktop_session",
                Value::from(self.desktop_session.as_str()).to_string(),
            ),
            (
                "agents.sandbox",
                Value::from(self.sandbox.as_str()).to_string(),
            ),
        ]
    }
}

fn line(key: &str, value: &str, source: &Source) -> String {
    format!("{key} = {value}  # {source}")
}

fn path_toml(path: &Path) -> Value {
    Value::from(path.display().to_string())
}

fn file_line(key: &str, file: &ConfigFile) -> String {
    let presence = if file.is_present { "read" } else { "absent" };
    format!("{key} = {}  # {presence}", path_toml(&file.path))
}
