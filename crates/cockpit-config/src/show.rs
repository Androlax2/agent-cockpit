//! `config show`: every effective value with its source, one `key = value  # source` per line.

use std::path::Path;

use toml::{Table, Value};

use crate::{ConfigFile, EffectiveConfig, Source};

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
    lines.join("\n") + "\n"
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
