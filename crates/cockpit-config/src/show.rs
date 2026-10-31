//! `config show`: every effective value with its source, one `key = value  # source` per line.

use std::path::Path;

use toml::Value;

use crate::{ConfigFile, EffectiveConfig, Source};

const UNSET: &str = "<unset>";

pub fn render(config: &EffectiveConfig) -> String {
    let layout = &config.layout;
    let mut lines = vec![
        line(
            "paths.config_dir",
            &path_value(&layout.config_dir.value),
            &layout.config_dir.source,
        ),
        line(
            "paths.state_dir",
            &path_value(&layout.state_dir.value),
            &layout.state_dir.source,
        ),
        line(
            "paths.cache_dir",
            &path_value(&layout.cache_dir.value),
            &layout.cache_dir.source,
        ),
        line(
            "paths.runtime_dir",
            &layout
                .runtime_dir
                .value
                .as_deref()
                .map_or(UNSET.to_string(), path_value),
            &layout.runtime_dir.source,
        ),
        file_line("files.user_config", &config.user_file),
    ];
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
                .map(|path| Value::String(path.display().to_string()))
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
                Value::String(name.clone()).to_string()
            }),
        &config.workflow_preset.source,
    ));
    for start_file in &config.start_files {
        let providers = Value::Array(
            start_file
                .value
                .providers
                .iter()
                .map(|provider| Value::String(provider.clone()))
                .collect(),
        );
        lines.push(line(
            &format!(
                "start_files.{}",
                Value::String(start_file.value.label.clone())
            ),
            &format!(
                "{{ path = {}, providers = {providers} }}",
                path_value(&start_file.value.path)
            ),
            &start_file.source,
        ));
    }
    lines.join("\n") + "\n"
}

fn line(key: &str, value: &str, source: &Source) -> String {
    format!("{key} = {value}  # {source}")
}

fn path_value(path: &Path) -> String {
    Value::String(path.display().to_string()).to_string()
}

fn file_line(key: &str, file: &ConfigFile) -> String {
    let presence = if file.is_present { "read" } else { "absent" };
    format!("{key} = {}  # {presence}", path_value(&file.path))
}
