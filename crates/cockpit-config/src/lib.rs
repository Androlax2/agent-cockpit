//! Owns the effective configuration and where each value came from: XDG layout, owner
//! settings, precedence, provenance and start-file change detection (report 09).
//!
//! Precedence, lowest first: product defaults, the owner's `config.toml`, the project's
//! `.cockpit/config.toml` (only settings that describe the project), `--set` overrides.
//! A project file that sets an owner setting fails: repository configuration describes
//! requirements, owner settings grant.

mod layout;
mod show;
mod start_files;

use std::fmt;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use toml::Spanned;

pub use layout::{Environment, Layout};
pub use show::render;
pub use start_files::{FileState, KNOWN_PROVIDERS, StartFile, StartFileSnapshot};

/// Where the project's own configuration lives, relative to the project directory.
pub const PROJECT_CONFIG_FILE: &str = ".cockpit/config.toml";

/// The settings `--set key=value` accepts; values are TOML (`--set 'projects.trusted=["/p"]'`).
pub const OVERRIDABLE_KEYS: [&str; 2] = ["projects.trusted", "workflow.preset"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    ProductDefault,
    Environment(&'static str),
    /// The variable is set to a relative path, which the XDG spec says to ignore; the
    /// product default applies.
    IgnoredEnvironment(&'static str),
    /// The variable is set to a relative path and nothing replaces it.
    InvalidEnvironment(&'static str),
    EnvironmentUnset(&'static str),
    UserFile {
        path: PathBuf,
        line: usize,
    },
    ProjectFile {
        path: PathBuf,
        line: usize,
    },
    CommandLine,
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProductDefault => write!(formatter, "product default"),
            Self::Environment(variable) => write!(formatter, "environment ${variable}"),
            Self::IgnoredEnvironment(variable) => write!(
                formatter,
                "product default; ${variable} ignored: not an absolute path"
            ),
            Self::InvalidEnvironment(variable) => {
                write!(formatter, "${variable} ignored: not an absolute path")
            }
            Self::EnvironmentUnset(variable) => write!(formatter, "${variable} is not set"),
            Self::UserFile { path, line } => {
                write!(formatter, "user config {}:{line}", path.display())
            }
            Self::ProjectFile { path, line } => {
                write!(formatter, "project config {}:{line}", path.display())
            }
            Self::CommandLine => write!(formatter, "command line --set"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: Source,
}

impl<T> Sourced<T> {
    pub fn new(value: T, source: Source) -> Self {
        Self { value, source }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub is_present: bool,
}

#[derive(Clone, Debug)]
pub struct EffectiveConfig {
    pub layout: Layout,
    pub user_file: ConfigFile,
    pub project_file: Option<ConfigFile>,
    /// Projects whose own skills and guidance may load (owner decision, report 09).
    pub trusted_projects: Sourced<Vec<PathBuf>>,
    /// The workflow preset the project asks for; workflows validate the name.
    pub workflow_preset: Sourced<Option<String>>,
    pub start_files: Vec<Sourced<StartFile>>,
}

impl EffectiveConfig {
    pub fn declared_start_files(&self) -> Vec<StartFile> {
        self.start_files
            .iter()
            .map(|start_file| start_file.value.clone())
            .collect()
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Home(String),
    Read {
        path: PathBuf,
        error: String,
    },
    Parse {
        origin: String,
        message: String,
    },
    OwnerOnly {
        key: &'static str,
        source: Source,
    },
    Invalid {
        key: String,
        source: Source,
        reason: String,
    },
    Override {
        argument: String,
        reason: String,
    },
    ProjectDir {
        path: PathBuf,
        reason: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Home(reason) => write!(formatter, "{reason}"),
            Self::Read { path, error } => write!(formatter, "{}: {error}", path.display()),
            Self::Parse { origin, message } => write!(formatter, "{origin}: {message}"),
            Self::OwnerOnly { key, source } => write!(
                formatter,
                "{source}: `{key}` is an owner setting; project configuration describes the \
                 project and cannot set it"
            ),
            Self::Invalid {
                key,
                source,
                reason,
            } => write!(formatter, "{source}: `{key}` {reason}"),
            Self::Override { argument, reason } => {
                write!(formatter, "--set {argument}: {reason}")
            }
            Self::ProjectDir { path, reason } => {
                write!(formatter, "project {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// One configuration file's settings, before precedence. Unknown keys fail with their line.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    projects: Option<Spanned<ProjectsTable>>,
    workflow: Option<WorkflowTable>,
    start_files: Option<Spanned<Vec<Spanned<StartFileEntry>>>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectsTable {
    trusted: Option<Spanned<Vec<String>>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowTable {
    preset: Option<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartFileEntry {
    label: String,
    path: String,
    providers: Vec<String>,
}

#[derive(Clone, Copy)]
enum LayerKind {
    User,
    Project,
    CommandLine,
}

struct Layer {
    kind: LayerKind,
    path: PathBuf,
    text: String,
    settings: SettingsFile,
}

impl Layer {
    fn parse(kind: LayerKind, path: PathBuf, text: String) -> Result<Self, ConfigError> {
        let settings = toml::from_str(&text).map_err(|error| ConfigError::Parse {
            origin: match kind {
                LayerKind::User => format!("user config {}", path.display()),
                LayerKind::Project => format!("project config {}", path.display()),
                LayerKind::CommandLine => format!("--set {text}"),
            },
            message: error.to_string(),
        })?;
        Ok(Self {
            kind,
            path,
            text,
            settings,
        })
    }

    fn source_at(&self, span: std::ops::Range<usize>) -> Source {
        let line = self.text[..span.start].matches('\n').count() + 1;
        match self.kind {
            LayerKind::User => Source::UserFile {
                path: self.path.clone(),
                line,
            },
            LayerKind::Project => Source::ProjectFile {
                path: self.path.clone(),
                line,
            },
            LayerKind::CommandLine => Source::CommandLine,
        }
    }
}

/// Resolves the effective configuration. `project_dir` adds that project's own file;
/// `overrides` are `key=value` arguments, applied in order after every file.
pub fn load(
    environment: &Environment,
    project_dir: Option<&Path>,
    overrides: &[String],
) -> Result<EffectiveConfig, ConfigError> {
    let home = environment.home_dir()?;
    let layout = Layout::resolve(environment, &home);

    let user_path = layout.user_config_file();
    let user_text = read_optional(&user_path)?;
    let user_file = ConfigFile {
        path: user_path.clone(),
        is_present: user_text.is_some(),
    };
    let mut layers = Vec::new();
    if let Some(text) = user_text {
        layers.push(Layer::parse(LayerKind::User, user_path, text)?);
    }

    let mut project_file = None;
    if let Some(project_dir) = project_dir {
        if !project_dir.is_dir() {
            return Err(ConfigError::ProjectDir {
                path: project_dir.to_path_buf(),
                reason: "not a directory".to_string(),
            });
        }
        let project_path = project_dir.join(PROJECT_CONFIG_FILE);
        let project_text = read_optional(&project_path)?;
        project_file = Some(ConfigFile {
            path: project_path.clone(),
            is_present: project_text.is_some(),
        });
        if let Some(text) = project_text {
            let layer = Layer::parse(LayerKind::Project, project_path, text)?;
            reject_owner_settings(&layer)?;
            layers.push(layer);
        }
    }

    for argument in overrides {
        layers.push(override_layer(argument)?);
    }

    Ok(EffectiveConfig {
        layout,
        user_file,
        project_file,
        trusted_projects: trusted_projects(&layers, &home)?,
        workflow_preset: workflow_preset(&layers)?,
        start_files: start_files(&layers, &home)?,
    })
}

fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            error: error.to_string(),
        }),
    }
}

fn reject_owner_settings(layer: &Layer) -> Result<(), ConfigError> {
    if let Some(projects) = &layer.settings.projects {
        return Err(ConfigError::OwnerOnly {
            key: "projects",
            source: layer.source_at(projects.span()),
        });
    }
    if let Some(start_files) = &layer.settings.start_files {
        return Err(ConfigError::OwnerOnly {
            key: "start_files",
            source: layer.source_at(start_files.span()),
        });
    }
    Ok(())
}

fn override_layer(argument: &str) -> Result<Layer, ConfigError> {
    let fail = |reason: &str| ConfigError::Override {
        argument: argument.to_string(),
        reason: reason.to_string(),
    };
    let Some((key, value)) = argument.split_once('=') else {
        return Err(fail("expected key=value"));
    };
    let key = key.trim();
    if !OVERRIDABLE_KEYS.contains(&key) {
        return Err(fail(&format!(
            "unknown key `{key}`; --set accepts {}",
            OVERRIDABLE_KEYS.join(", ")
        )));
    }
    // One line keeps the value from smuggling in other keys or tables.
    if value.contains(['\n', '\r']) {
        return Err(fail("the value must be on one line"));
    }
    Layer::parse(
        LayerKind::CommandLine,
        PathBuf::new(),
        format!("{key} = {value}"),
    )
    .map_err(|error| match error {
        ConfigError::Parse { message, .. } => fail(&message),
        other => other,
    })
}

/// `~` and `~/…` expand to `$HOME`. The result must be absolute without `..`, so a path
/// reads as the directory it names, and is normalized (`.` and trailing `/` dropped) so
/// callers can compare paths by equality.
fn expand_path(raw: &str, home: &Path) -> Result<PathBuf, String> {
    let path = match raw.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) => match rest.strip_prefix('/') {
            Some(relative) => home.join(relative),
            None => PathBuf::from(raw),
        },
        None => PathBuf::from(raw),
    };
    if !path.is_absolute() {
        return Err(format!("{raw:?} is not an absolute path"));
    }
    if path.components().any(|part| part == Component::ParentDir) {
        return Err(format!("{raw:?} contains `..`"));
    }
    Ok(path.components().collect())
}

fn trusted_projects(layers: &[Layer], home: &Path) -> Result<Sourced<Vec<PathBuf>>, ConfigError> {
    let mut trusted = Sourced::new(Vec::new(), Source::ProductDefault);
    for layer in layers {
        let Some(raw_paths) = layer
            .settings
            .projects
            .as_ref()
            .and_then(|projects| projects.get_ref().trusted.as_ref())
        else {
            continue;
        };
        let source = layer.source_at(raw_paths.span());
        let paths = raw_paths
            .get_ref()
            .iter()
            .map(|raw| {
                expand_path(raw, home).map_err(|reason| ConfigError::Invalid {
                    key: "projects.trusted".to_string(),
                    source: source.clone(),
                    reason: format!("entry {reason}"),
                })
            })
            .collect::<Result<_, _>>()?;
        trusted = Sourced::new(paths, source);
    }
    Ok(trusted)
}

fn workflow_preset(layers: &[Layer]) -> Result<Sourced<Option<String>>, ConfigError> {
    let mut preset = Sourced::new(None, Source::ProductDefault);
    for layer in layers {
        let Some(name) = layer
            .settings
            .workflow
            .as_ref()
            .and_then(|workflow| workflow.preset.as_ref())
        else {
            continue;
        };
        let source = layer.source_at(name.span());
        if name.get_ref().trim().is_empty() {
            return Err(ConfigError::Invalid {
                key: "workflow.preset".to_string(),
                source,
                reason: "is empty".to_string(),
            });
        }
        preset = Sourced::new(Some(name.get_ref().clone()), source);
    }
    Ok(preset)
}

/// Product defaults, then each `[[start_files]]` entry: a new label adds a file, an existing
/// label replaces that file in place.
fn start_files(layers: &[Layer], home: &Path) -> Result<Vec<Sourced<StartFile>>, ConfigError> {
    let mut declared: Vec<Sourced<StartFile>> = start_files::DEFAULT_START_FILES
        .iter()
        .map(|(label, relative_path, providers)| {
            Sourced::new(
                StartFile {
                    label: label.to_string(),
                    path: home.join(relative_path),
                    providers: providers.iter().map(|name| name.to_string()).collect(),
                },
                Source::ProductDefault,
            )
        })
        .collect();
    for layer in layers {
        let Some(entries) = &layer.settings.start_files else {
            continue;
        };
        let mut labels_in_layer: Vec<&str> = Vec::new();
        for entry in entries.get_ref() {
            let source = layer.source_at(entry.span());
            let start_file = validate_start_file(entry.get_ref(), home, &source)?;
            if labels_in_layer.contains(&start_file.label.as_str()) {
                return Err(invalid_start_file(&source, "repeats a label in this file"));
            }
            labels_in_layer.push(&entry.get_ref().label);
            let replacement = Sourced::new(start_file, source);
            match declared
                .iter_mut()
                .find(|existing| existing.value.label == replacement.value.label)
            {
                Some(existing) => *existing = replacement,
                None => declared.push(replacement),
            }
        }
    }
    Ok(declared)
}

fn invalid_start_file(source: &Source, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        key: "start_files".to_string(),
        source: source.clone(),
        reason: format!("entry {reason}"),
    }
}

fn validate_start_file(
    entry: &StartFileEntry,
    home: &Path,
    source: &Source,
) -> Result<StartFile, ConfigError> {
    if entry.label.trim().is_empty() {
        return Err(invalid_start_file(source, "has an empty label"));
    }
    // Labels match defaults by equality: "CLAUDE.md " would silently add a second file.
    if entry.label.trim() != entry.label {
        return Err(invalid_start_file(
            source,
            &format!("label {:?} has surrounding whitespace", entry.label),
        ));
    }
    let path = expand_path(&entry.path, home)
        .map_err(|reason| invalid_start_file(source, &format!("path {reason}")))?;
    if entry.providers.is_empty() {
        return Err(invalid_start_file(source, "names no providers"));
    }
    if let Some(unknown) = entry
        .providers
        .iter()
        .find(|provider| !KNOWN_PROVIDERS.contains(&provider.as_str()))
    {
        return Err(invalid_start_file(
            source,
            &format!(
                "names unknown provider {unknown:?}; known: {}",
                KNOWN_PROVIDERS.join(", ")
            ),
        ));
    }
    Ok(StartFile {
        label: entry.label.clone(),
        path,
        providers: entry.providers.clone(),
    })
}
