//! Host Git under the safety rules: hooks off, settings that run programs reset, no Git
//! environment inherited, no prompts.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::Error;

/// Settings applied to every host Git call, after the repository's own configuration so they
/// win over it. Each one either runs a program or sends data somewhere: a checkout's
/// `.git/config` is written by whoever works in it, so it must not choose what the daemon
/// runs. The list started as the current `cockpit` script's `SAFE_GIT_CONFIG`.
pub const SAFE_CONFIG: &[&str] = &[
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    "core.sshCommand=ssh",
    "core.askPass=",
    "core.gitProxy=",
    "core.pager=cat",
    "diff.external=",
    "credential.helper=",
    "log.showSignature=false",
    "push.gpgSign=false",
    "gpg.program=gpg",
    "gpg.ssh.program=ssh-keygen",
    "gpg.x509.program=gpgsm",
    "protocol.ext.allow=never",
    "protocol.file.allow=never",
    "remote.origin.receivepack=git-receive-pack",
    "remote.origin.uploadpack=git-upload-pack",
];

/// Settings whose driver name the configuration chooses and whose value is a program to run:
/// content filters (run by `status` and `switch`), diff and merge drivers.
const PROGRAM_DRIVERS: &str =
    r"^(filter\..+\.(clean|smudge|process)|diff\..+\.(textconv|command)|merge\..+\.driver)$";

/// The host Git program. Every call goes through `Git::run` or `Git::output`, which apply
/// [`SAFE_CONFIG`] and reset every program-running driver; nothing in this crate builds a Git command another way.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
}

impl Git {
    /// `git` as found on the daemon's `PATH`.
    pub fn from_path() -> Git {
        Git {
            program: PathBuf::from("git"),
        }
    }

    /// Runs `arguments` in `dir` and returns its standard output; a non-zero exit is an error.
    pub(crate) fn run(&self, dir: &Path, arguments: &[&OsStr]) -> Result<String, Error> {
        self.run_with(dir, &[], arguments)
    }

    /// [`Git::run`] with `extra_config` applied after [`SAFE_CONFIG`], for the one call that
    /// needs a setting it resets.
    pub(crate) fn run_with(
        &self,
        dir: &Path,
        extra_config: &[&str],
        arguments: &[&OsStr],
    ) -> Result<String, Error> {
        let output = self.output_with(dir, extra_config, arguments)?;
        if !output.status.success() {
            return Err(git_failure(arguments, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Runs `arguments` in `dir` and returns its output whatever the exit status.
    pub(crate) fn output(&self, dir: &Path, arguments: &[&OsStr]) -> Result<Output, Error> {
        self.output_with(dir, &[], arguments)
    }

    fn output_with(
        &self,
        dir: &Path,
        extra_config: &[&str],
        arguments: &[&OsStr],
    ) -> Result<Output, Error> {
        let drivers_off = self.drivers_off(dir)?;
        let mut command = self.command(dir, extra_config);
        for setting in &drivers_off {
            command.arg("-c").arg(setting);
        }
        command.args(arguments);
        command.output().map_err(Error::io("run git in", dir))
    }

    /// An empty value for every program-running driver the configuration seen from `dir`
    /// names, as `key=` settings. Driver names are chosen by whoever writes the
    /// configuration, so they cannot be listed in [`SAFE_CONFIG`]; reading the configuration
    /// runs nothing. Approved runs (#34) apply them next to [`SAFE_CONFIG`].
    pub fn drivers_off(&self, dir: &Path) -> Result<Vec<String>, Error> {
        let arguments = [
            arg("config"),
            arg("--includes"),
            arg("--null"),
            arg("--name-only"),
            arg("--get-regexp"),
            arg(PROGRAM_DRIVERS),
        ];
        let mut command = self.command(dir, &[]);
        command.args(arguments);
        let output = command.output().map_err(Error::io("run git in", dir))?;
        // `git config --get-regexp` exits 1 when nothing matches.
        if output.status.code() == Some(1) {
            return Ok(Vec::new());
        }
        if !output.status.success() {
            return Err(git_failure(&arguments, &output));
        }
        let mut settings = Vec::new();
        for name in output.stdout.split(|byte| *byte == 0) {
            if name.is_empty() {
                continue;
            }
            let name = String::from_utf8_lossy(name);
            // `-c` splits at the first `=`, so such a name could not be overridden.
            if name.contains('=') {
                return Err(Error::Refused {
                    reason: format!(
                        "{}: the setting {name:?} cannot be reset safely",
                        dir.display()
                    ),
                });
            }
            settings.push(format!("{name}="));
        }
        Ok(settings)
    }

    fn command(&self, dir: &Path, extra_config: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        command.current_dir(dir);
        for setting in SAFE_CONFIG.iter().chain(extra_config) {
            command.arg("-c").arg(setting);
        }
        for (variable, _) in std::env::vars_os() {
            if variable.to_string_lossy().starts_with("GIT_") {
                command.env_remove(variable);
            }
        }
        command.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
        command
    }
}

pub(crate) fn git_failure(arguments: &[&OsStr], output: &Output) -> Error {
    Error::Git {
        arguments: arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// `text` as a Git argument.
pub(crate) fn arg(text: &(impl AsRef<OsStr> + ?Sized)) -> &OsStr {
    text.as_ref()
}

/// Whether `url` is a GitHub repository URL in one of the three forms the push accepts, and
/// nothing else: no user other than `git`, no port, no query, no credentials.
pub(crate) fn is_github_url(url: &str) -> bool {
    let repository = [
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
    ]
    .iter()
    .find_map(|prefix| url.strip_prefix(prefix));
    let Some(repository) = repository else {
        return false;
    };
    !repository.is_empty()
        && repository
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._/-".contains(character))
}
