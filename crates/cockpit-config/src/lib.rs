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
use std::time::Duration;

use serde::Deserialize;
use toml::Spanned;

pub use layout::{AGENT_SOCKET, AGENT_SOCKET_DIR, DAEMON_SOCKET, Environment, Layout};
pub use show::render;
pub use start_files::{FileState, KNOWN_PROVIDERS, RecordedFile, StartFile, StartFileSnapshot};

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
    /// The project sessions' blocker reports go to (owner setting, #38).
    pub tasks_blocker_project: Sourced<Option<String>>,
    /// Open items one project's sessions may have filed in another project's list.
    pub tasks_cross_project_open_max: Sourced<u32>,
    /// How long an agent waits at its prompt before its session sleeps.
    pub idle_sleep_after: Sourced<Duration>,
    /// How long a session may sleep before it ends.
    pub asleep_expiry: Sourced<Duration>,
    /// Whether launched agents reach the owner's desktop session (#305).
    pub agents_desktop_session: Sourced<DesktopSession>,
    /// The checks the project declares, in its order: what Cockpit runs and a gate requires.
    pub checks: Vec<Sourced<Check>>,
}

/// Whether a launched agent gets the owner's display and session bus variables. The one
/// named opt-out (#305): the owner's `agents.desktop_session` selects it today, a sandbox
/// profile later (#61). The session bus can start units outside the agent's containment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopSession {
    /// The agent gets the variables the daemon has, as #303 decided.
    Shared,
    /// The agent gets none of them.
    Withheld,
}

impl DesktopSession {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Withheld => "withheld",
        }
    }
}

/// A check a project declares (report 09): its own runner (Cargo, Make, ...) stays the source
/// of truth; Cockpit runs the command and records what it found. `cockpit-verification`
/// validates the fields when the check runs or a gate is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// What a gate requires it by, unique in the project.
    pub name: String,
    /// A shell command line.
    pub command: String,
    /// Relative to the checkout; `.` (the default) is its root.
    pub working_directory: String,
    /// Where it runs; `native` by default.
    pub environment: String,
    /// The files it covers, relative to the checkout: a change to one makes its result stale.
    pub artifacts: Vec<String>,
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
    tasks: Option<Spanned<TasksTable>>,
    sessions: Option<Spanned<SessionsTable>>,
    agents: Option<Spanned<AgentsTable>>,
    checks: Option<Spanned<Vec<Spanned<CheckEntry>>>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentsTable {
    desktop_session: Option<Spanned<DesktopSession>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckEntry {
    name: String,
    command: String,
    working_directory: Option<String>,
    environment: Option<String>,
    #[serde(default)]
    artifacts: Vec<String>,
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

/// Whole minutes and days: `u32` keeps a negative or huge value a parse error with its line.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionsTable {
    idle_sleep_after_minutes: Option<Spanned<u32>>,
    asleep_expiry_days: Option<Spanned<u32>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TasksTable {
    blocker_project: Option<Spanned<String>>,
    cross_project_open_max: Option<Spanned<i64>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartFileEntry {
    label: String,
    path: String,
    providers: Vec<String>,
}

/// The file a layer came from; `--set` overrides have none.
enum LayerOrigin {
    User(PathBuf),
    Project(PathBuf),
    CommandLine,
}

struct Layer {
    origin: LayerOrigin,
    text: String,
    settings: SettingsFile,
}

impl Layer {
    fn source_at(&self, span: std::ops::Range<usize>) -> Source {
        let line = self.text[..span.start].matches('\n').count() + 1;
        match &self.origin {
            LayerOrigin::User(path) => Source::UserFile {
                path: path.clone(),
                line,
            },
            LayerOrigin::Project(path) => Source::ProjectFile {
                path: path.clone(),
                line,
            },
            LayerOrigin::CommandLine => Source::CommandLine,
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

    let mut layers = Vec::new();
    let (user_file, user_layer) =
        read_layer(layout.user_config_file(), LayerOrigin::User, "user config")?;
    if let Some(layer) = &user_layer {
        reject_project_settings(layer)?;
    }
    layers.extend(user_layer);

    let mut project_file = None;
    if let Some(project_dir) = project_dir {
        if !project_dir.is_dir() {
            return Err(ConfigError::ProjectDir {
                path: project_dir.to_path_buf(),
                reason: "not a directory".to_string(),
            });
        }
        let (file, project_layer) = read_layer(
            project_dir.join(PROJECT_CONFIG_FILE),
            LayerOrigin::Project,
            "project config",
        )?;
        project_file = Some(file);
        if let Some(layer) = project_layer {
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
        tasks_blocker_project: tasks_blocker_project(&layers)?,
        tasks_cross_project_open_max: tasks_cross_project_open_max(&layers)?,
        idle_sleep_after: idle_sleep_after(&layers)?,
        asleep_expiry: asleep_expiry(&layers)?,
        agents_desktop_session: agents_desktop_session(&layers),
        checks: checks(&layers)?,
    })
}

/// Reads and parses a configuration file; an absent file is no layer, not an error.
fn read_layer(
    path: PathBuf,
    origin: fn(PathBuf) -> LayerOrigin,
    description: &str,
) -> Result<(ConfigFile, Option<Layer>), ConfigError> {
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let file = ConfigFile {
                path,
                is_present: false,
            };
            return Ok((file, None));
        }
        Err(error) => {
            return Err(ConfigError::Read {
                path,
                error: error.to_string(),
            });
        }
    };
    let settings = toml::from_str(&text).map_err(|error| ConfigError::Parse {
        origin: format!("{description} {}", path.display()),
        message: error.to_string(),
    })?;
    let file = ConfigFile {
        path: path.clone(),
        is_present: true,
    };
    Ok((
        file,
        Some(Layer {
            origin: origin(path),
            text,
            settings,
        }),
    ))
}

fn reject_owner_settings(layer: &Layer) -> Result<(), ConfigError> {
    // Exhaustive on purpose: a new setting must be classified here as owner or describing.
    let SettingsFile {
        projects,
        start_files,
        tasks,
        sessions,
        agents,
        workflow: _,
        checks: _,
    } = &layer.settings;
    if let Some(tasks) = tasks {
        return Err(ConfigError::OwnerOnly {
            key: "tasks",
            source: layer.source_at(tasks.span()),
        });
    }
    if let Some(projects) = projects {
        return Err(ConfigError::OwnerOnly {
            key: "projects",
            source: layer.source_at(projects.span()),
        });
    }
    if let Some(start_files) = start_files {
        return Err(ConfigError::OwnerOnly {
            key: "start_files",
            source: layer.source_at(start_files.span()),
        });
    }
    if let Some(sessions) = sessions {
        return Err(ConfigError::OwnerOnly {
            key: "sessions",
            source: layer.source_at(sessions.span()),
        });
    }
    if let Some(agents) = agents {
        return Err(ConfigError::OwnerOnly {
            key: "agents",
            source: layer.source_at(agents.span()),
        });
    }
    Ok(())
}

/// The owner's file declares no checks: they would apply to every project.
fn reject_project_settings(layer: &Layer) -> Result<(), ConfigError> {
    if let Some(checks) = &layer.settings.checks {
        return Err(ConfigError::Invalid {
            key: "checks".to_string(),
            source: layer.source_at(checks.span()),
            reason: "describes a project; declare it in the project's .cockpit/config.toml"
                .to_string(),
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
    let text = format!("{key} = {value}");
    let settings = toml::from_str(&text).map_err(|error| fail(&error.to_string()))?;
    Ok(Layer {
        origin: LayerOrigin::CommandLine,
        text,
        settings,
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

/// Product default for [`EffectiveConfig::tasks_cross_project_open_max`].
pub const CROSS_PROJECT_OPEN_MAX: u32 = 3;

fn tasks_blocker_project(layers: &[Layer]) -> Result<Sourced<Option<String>>, ConfigError> {
    let mut project = Sourced::new(None, Source::ProductDefault);
    for layer in layers {
        let Some(name) = layer
            .settings
            .tasks
            .as_ref()
            .and_then(|tasks| tasks.get_ref().blocker_project.as_ref())
        else {
            continue;
        };
        let source = layer.source_at(name.span());
        if name.get_ref().trim().is_empty() {
            return Err(ConfigError::Invalid {
                key: "tasks.blocker_project".to_string(),
                source,
                reason: "is empty".to_string(),
            });
        }
        project = Sourced::new(Some(name.get_ref().clone()), source);
    }
    Ok(project)
}

fn tasks_cross_project_open_max(layers: &[Layer]) -> Result<Sourced<u32>, ConfigError> {
    let mut cap = Sourced::new(CROSS_PROJECT_OPEN_MAX, Source::ProductDefault);
    for layer in layers {
        let Some(value) = layer
            .settings
            .tasks
            .as_ref()
            .and_then(|tasks| tasks.get_ref().cross_project_open_max.as_ref())
        else {
            continue;
        };
        let source = layer.source_at(value.span());
        let value = u32::try_from(*value.get_ref()).map_err(|_| ConfigError::Invalid {
            key: "tasks.cross_project_open_max".to_string(),
            source: source.clone(),
            reason: "must be a whole number from 0".to_string(),
        })?;
        cap = Sourced::new(value, source);
    }
    Ok(cap)
}

/// The current cockpit's `idle-sleep-after`: 60 minutes by default, 30 at least.
pub(crate) const IDLE_SLEEP_AFTER_MINUTES: Bound = Bound {
    key: "sessions.idle_sleep_after_minutes",
    default: 60,
    minimum: 30,
    unit: "minutes",
    seconds_per_unit: 60,
};

/// The current cockpit's fixed 7-day expiry; less than a day would end a session its owner
/// put to sleep for the night.
pub(crate) const ASLEEP_EXPIRY_DAYS: Bound = Bound {
    key: "sessions.asleep_expiry_days",
    default: 7,
    minimum: 1,
    unit: "days",
    seconds_per_unit: 86_400,
};

/// A whole number of `unit`s with a product default and a minimum.
pub(crate) struct Bound {
    key: &'static str,
    default: u32,
    minimum: u32,
    unit: &'static str,
    seconds_per_unit: u64,
}

impl Bound {
    fn duration(&self, count: u32) -> Duration {
        Duration::from_secs(u64::from(count) * self.seconds_per_unit)
    }

    /// The whole units in `duration`, as the owner wrote them.
    pub(crate) fn count(&self, duration: Duration) -> u64 {
        duration.as_secs() / self.seconds_per_unit
    }

    /// The last layer's value, checked against the minimum.
    fn resolve(
        &self,
        layers: &[Layer],
        value_in: fn(&SessionsTable) -> Option<&Spanned<u32>>,
    ) -> Result<Sourced<Duration>, ConfigError> {
        let mut resolved = Sourced::new(self.duration(self.default), Source::ProductDefault);
        for layer in layers {
            let Some(count) = layer
                .settings
                .sessions
                .as_ref()
                .and_then(|sessions| value_in(sessions.get_ref()))
            else {
                continue;
            };
            let source = layer.source_at(count.span());
            if *count.get_ref() < self.minimum {
                return Err(ConfigError::Invalid {
                    key: self.key.to_string(),
                    source,
                    reason: format!("must be at least {} {}", self.minimum, self.unit),
                });
            }
            resolved = Sourced::new(self.duration(*count.get_ref()), source);
        }
        Ok(resolved)
    }
}

fn idle_sleep_after(layers: &[Layer]) -> Result<Sourced<Duration>, ConfigError> {
    IDLE_SLEEP_AFTER_MINUTES.resolve(layers, |sessions| {
        sessions.idle_sleep_after_minutes.as_ref()
    })
}

fn asleep_expiry(layers: &[Layer]) -> Result<Sourced<Duration>, ConfigError> {
    ASLEEP_EXPIRY_DAYS.resolve(layers, |sessions| sessions.asleep_expiry_days.as_ref())
}

fn agents_desktop_session(layers: &[Layer]) -> Sourced<DesktopSession> {
    let mut resolved = Sourced::new(DesktopSession::Shared, Source::ProductDefault);
    for layer in layers {
        let Some(value) = layer
            .settings
            .agents
            .as_ref()
            .and_then(|agents| agents.get_ref().desktop_session.as_ref())
        else {
            continue;
        };
        resolved = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
    }
    resolved
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

/// The project's `[[checks]]`, in its order. Only the project file declares them, so there
/// is one list and no precedence.
fn checks(layers: &[Layer]) -> Result<Vec<Sourced<Check>>, ConfigError> {
    let mut checks: Vec<Sourced<Check>> = Vec::new();
    for layer in layers {
        let Some(entries) = &layer.settings.checks else {
            continue;
        };
        for entry in entries.get_ref() {
            let source = layer.source_at(entry.span());
            let check = entry.get_ref();
            let invalid = |reason: String| ConfigError::Invalid {
                key: "checks".to_string(),
                source: source.clone(),
                reason: format!("entry {reason}"),
            };
            if check.name.trim().is_empty() {
                return Err(invalid("has an empty name".to_string()));
            }
            if check.command.trim().is_empty() {
                return Err(invalid(format!("{:?} has an empty command", check.name)));
            }
            if checks.iter().any(|known| known.value.name == check.name) {
                return Err(invalid(format!("repeats the name {:?}", check.name)));
            }
            checks.push(Sourced::new(
                Check {
                    name: check.name.clone(),
                    command: check.command.clone(),
                    working_directory: check
                        .working_directory
                        .clone()
                        .unwrap_or_else(|| ".".to_string()),
                    environment: check
                        .environment
                        .clone()
                        .unwrap_or_else(|| "native".to_string()),
                    artifacts: check.artifacts.clone(),
                },
                source,
            ));
        }
    }
    Ok(checks)
}
