//! Profiles (#196, #134): one switch that keeps two Cockpits on one machine apart. Profile
//! `<name>` places every file, socket, lock, tmux server and unit under `cockpit-<name>`;
//! the default profile is `cockpit` itself.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::{ConfigError, Source, Sourced};

/// The variable that selects a profile; `--profile` beats it.
pub const PROFILE_VARIABLE: &str = "COCKPIT_PROFILE";

/// What the cutover ships, and what a release build runs without a choice.
pub const DEFAULT_PROFILE: &str = "cockpit";

/// What a build outside a release bundle runs without a choice, so `cargo run` from a clone
/// never reaches the installed Cockpit.
pub const DEVELOPMENT_PROFILE: &str = "dev";

/// Where a release bundle puts its manifest under the prefix holding `bin/cockpit`; the same
/// for the unpacked archive and an install under `/usr` or `/opt/cockpit` (`tools/release.py`).
pub const MANIFEST_IN_PREFIX: &str = "share/cockpit/manifest.json";

/// Cockpit's programs are `cockpit-<suffix>`; a profile of that name would make its
/// `cockpit-<name>` command replace the program.
const PROGRAM_SUFFIXES: &[&str] = &[
    "daemon",
    "tui",
    "claude-hook",
    "codex-hook",
    "codex-session",
    "opencode-bridge",
];

/// Long enough for `next` or a branch tag, short enough for a unix socket path inside it.
const NAME_MAX: usize = 24;

/// A validated profile name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Profile(String);

impl Profile {
    /// The default profile, `cockpit`.
    pub fn default_profile() -> Self {
        Self(DEFAULT_PROFILE.to_owned())
    }

    /// Lowercase ASCII letters, digits and inner hyphens, starting with a letter; never the
    /// suffix of one of Cockpit's programs.
    pub fn parse(name: &str) -> Result<Self, String> {
        let is_shaped = name.len() <= NAME_MAX
            && name.starts_with(|first: char| first.is_ascii_lowercase())
            && !name.ends_with('-')
            && !name.contains("--")
            && name.chars().all(|letter| {
                letter.is_ascii_lowercase() || letter.is_ascii_digit() || letter == '-'
            });
        if !is_shaped {
            return Err(format!(
                "profile {name:?} is not a name: use at most {NAME_MAX} lowercase letters, \
                 digits and inner hyphens, starting with a letter"
            ));
        }
        if PROGRAM_SUFFIXES.contains(&name) {
            return Err(format!(
                "profile {name:?} would be named like Cockpit's program cockpit-{name}"
            ));
        }
        Ok(Self(name.to_owned()))
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_PROFILE
    }

    /// `cockpit` or `cockpit-<name>`: the directory name under each XDG base, the tmux
    /// server's socket name, the command name and the unit's base name.
    pub fn qualified_name(&self) -> String {
        if self.is_default() {
            DEFAULT_PROFILE.to_owned()
        } else {
            format!("{DEFAULT_PROFILE}-{}", self.0)
        }
    }

    /// The systemd user unit the daemon of this profile runs as.
    pub fn systemd_unit(&self) -> String {
        format!("{}.service", self.qualified_name())
    }

    /// The launchd agent's label on macOS, with the `-<name>` suffix OpenLogi's dev bundle
    /// uses for its identifiers.
    pub fn launchd_label(&self) -> String {
        format!("io.github.androlax2.{}", self.qualified_name())
    }

    /// The profile a command named `cockpit-<name>` selects; `None` for `cockpit` and any
    /// other name.
    pub fn of_command_name(command: &OsStr) -> Option<Self> {
        let name = Path::new(command).file_name()?.to_str()?;
        let suffix = name.strip_prefix(DEFAULT_PROFILE)?.strip_prefix('-')?;
        Self::parse(suffix).ok()
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::default_profile()
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether a program is part of a release bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    Release,
    /// A debug build, or one with no release manifest beside it (`cargo build --release`).
    Development,
}

impl Build {
    /// `program` is this process's own path. Symlinks are resolved first, so
    /// `/usr/local/bin/cockpit -> /opt/cockpit/bin/cockpit` finds `/opt/cockpit/share/`.
    pub fn of_program(program: &Path) -> Self {
        if cfg!(debug_assertions) {
            return Self::Development;
        }
        match release_manifest(program) {
            Ok(manifest) if manifest.is_file() => Self::Release,
            _ => Self::Development,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Development => "development",
        }
    }
}

/// Where the release manifest of the bundle holding `program` would be.
pub fn release_manifest(program: &Path) -> std::io::Result<PathBuf> {
    let binary = program.canonicalize()?;
    let prefix = binary
        .parent()
        .and_then(|binary_dir| binary_dir.parent())
        .ok_or_else(|| std::io::Error::other(format!("{} has no prefix", binary.display())))?;
    Ok(prefix.join(MANIFEST_IN_PREFIX))
}

/// What may select a profile for one process, strongest first.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProfileChoice<'a> {
    /// `--profile NAME`.
    pub flag: Option<&'a str>,
    /// The name the program was started as (`argv[0]`): `cockpit-next` selects `next`.
    pub command_name: Option<&'a OsStr>,
    /// `$COCKPIT_PROFILE`.
    pub variable: Option<&'a OsStr>,
}

/// The profile `choice` selects, then the build's own: `cockpit` for a release, `dev`
/// otherwise. The command name beats the variable: every launched agent inherits
/// `$COCKPIT_PROFILE`, and `cockpit-next` typed in one must still reach `next`. A development build reaches the default profile only through `--profile
/// cockpit`: `$COCKPIT_PROFILE=cockpit`, which every agent of the installed Cockpit inherits,
/// is ignored there, so a test build run by such an agent stays on `dev`.
pub fn resolve_profile(
    choice: ProfileChoice<'_>,
    build: Build,
) -> Result<Sourced<Profile>, ConfigError> {
    if let Some(flag) = choice.flag {
        let source = Source::Flag("--profile");
        let profile = Profile::parse(flag).map_err(|reason| invalid(source.clone(), reason))?;
        return Ok(Sourced::new(profile, source));
    }
    let build_default = || match build {
        Build::Release => Profile::default_profile(),
        Build::Development => Profile(DEVELOPMENT_PROFILE.to_owned()),
    };
    if let Some(profile) = choice.command_name.and_then(Profile::of_command_name) {
        return Ok(Sourced::new(profile, Source::CommandName));
    }
    if let Some(variable) = choice.variable.filter(|value| !value.is_empty()) {
        let source = Source::Environment(PROFILE_VARIABLE);
        let name = variable
            .to_str()
            .ok_or_else(|| invalid(source.clone(), format!("{variable:?} is not UTF-8")))?;
        let profile = Profile::parse(name).map_err(|reason| invalid(source.clone(), reason))?;
        if profile.is_default() && build == Build::Development {
            return Ok(Sourced::new(
                build_default(),
                Source::DevelopmentBuild {
                    ignored: Some(PROFILE_VARIABLE),
                },
            ));
        }
        return Ok(Sourced::new(profile, source));
    }
    Ok(match build {
        Build::Release => Sourced::new(build_default(), Source::ProductDefault),
        Build::Development => {
            Sourced::new(build_default(), Source::DevelopmentBuild { ignored: None })
        }
    })
}

fn invalid(source: Source, reason: String) -> ConfigError {
    ConfigError::Invalid {
        key: "profile".to_owned(),
        source,
        reason,
    }
}
