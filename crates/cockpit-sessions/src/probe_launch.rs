//! `torlek doctor`'s probe launch (#833): the profile a session of a provider would get, run
//! for real through the native sandbox with a shell check in place of the agent, which reads
//! each per-session cache variable of the toolchain table (#859) inside it. Nothing of the
//! provider runs and no prompt is sent; the dry run's folders are removed after.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_provider_api::toolchains::{COMMON_VARIABLES, Setting, TOOLCHAINS};

use super::{DRY_RUN_SESSION, Error, Sessions, create_session_dir, docker_config_dir};

/// How long the probe may run: bubblewrap and the check take well under a second, and
/// `torlek doctor` waits a minute for every provider's.
const PROBE_DEADLINE: Duration = Duration::from_secs(10);

/// The shell the check runs in, inside the sandbox's system folders.
const SHELL: &str = "/bin/sh";

/// What the check prints last when every variable holds.
const PASSED: &str = "torlek probe: passed";

impl Sessions {
    /// Runs `prepared`, a sandboxed profile [`Sessions::sandbox_preflight`] built, with the
    /// check in place of its program: what it found, or why it failed. One probe runs at a
    /// time, since every probe stages the same dry-run folders. Only a blocking step that
    /// does not end is an error.
    pub async fn probe_launch(
        &self,
        prepared: cockpit_sandbox::Prepared,
    ) -> Result<Result<String, String>, Error> {
        let _one_at_a_time = self.probe_launches.lock().await;
        let session_dir = self.session_dirs.join(DRY_RUN_SESSION);
        let temp_dir = self.dry_run_temp_dir();
        self.blocking("run the probe launch", move || {
            let staged_folders = prepared
                .staged
                .as_ref()
                .map(staged_folders)
                .unwrap_or_default();
            // The folders a launch creates before it starts the agent, as it creates them.
            let created = [&session_dir, &temp_dir, &docker_config_dir(&temp_dir)]
                .into_iter()
                .try_for_each(|folder| create_session_dir(folder))
                .map_err(|error| format!("cannot create the probe launch's folders: {error}"));
            let result = created.and_then(|()| run(prepared));
            for folder in staged_folders.iter().chain([&session_dir, &temp_dir]) {
                if let Err(error) = remove_folder(folder) {
                    tracing::warn!(folder = %folder.display(), %error, "the probe launch's folder is left");
                }
            }
            Ok(result)
        })
        .await
    }
}

fn staged_folders(staged: &cockpit_sandbox::Staged) -> Vec<PathBuf> {
    [
        staged.mount_dir.clone(),
        staged.home_source.clone(),
        staged.config_dir.clone(),
    ]
    .into_iter()
    .chain(staged.folder_sources.iter().cloned())
    .collect()
}

fn remove_folder(folder: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

/// The probe's finding when the check passed; its own lines, or the launcher's, when not.
fn run(prepared: cockpit_sandbox::Prepared) -> Result<String, String> {
    let Some(mut staged) = prepared.staged else {
        return Err("the profile runs unsandboxed, so there is no sandbox to probe".to_owned());
    };
    let (script, checked) = check_script();
    staged.spec.program = PathBuf::from(SHELL);
    staged.spec.arguments = vec!["-c".to_owned(), script, "torlek-probe".to_owned()];
    // A project's container engine is the launch's to reach, through a helper this probe
    // has no part in.
    staged.spec.engine = None;
    staged
        .write()
        .map_err(|error| format!("cannot stage the probe launch: {error}"))?;
    let (status, output) = run_until_deadline(
        &prepared.program,
        &prepared.arguments,
        &prepared.environment,
        &staged.spec.working_dir,
    )?;
    let printed = output.trim_end();
    if status == Some(0) && printed.ends_with(PASSED) {
        return Ok(format!(
            "ran in the sandbox: {checked} per-session variables of the toolchain table hold, \
             each cache folder writable"
        ));
    }
    let printed = printed.strip_suffix(PASSED).unwrap_or(printed).trim_end();
    let exit = status.map_or_else(|| "a signal".to_owned(), |code| format!("exit {code}"));
    Err(if printed.is_empty() {
        format!("the probe launch ended with {exit} and printed nothing")
    } else {
        format!("the probe launch ended with {exit}: {}", one_line(printed))
    })
}

fn one_line(printed: &str) -> String {
    printed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Starts the launcher with exactly `environment`, and its exit code (none for a signal) with
/// what it printed on both streams; one that outlives [`PROBE_DEADLINE`] is killed.
fn run_until_deadline(
    program: &Path,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    working_dir: &Path,
) -> Result<(Option<i32>, String), String> {
    let (mut reader, writer) =
        std::io::pipe().map_err(|error| format!("cannot open the probe's output: {error}"))?;
    let mut child = Command::new(program)
        .args(arguments)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(
            writer
                .try_clone()
                .map_err(|error| format!("cannot open the probe's output: {error}"))?,
        )
        .stderr(writer)
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
    let deadline = Instant::now() + PROBE_DEADLINE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the probe launch did not end within {} seconds",
                    PROBE_DEADLINE.as_secs()
                ));
            }
            Err(error) => return Err(format!("cannot wait for the probe launch: {error}")),
        }
    };
    // The command holds the pipe's writing ends; they close with it, so the read ends.
    drop(child);
    let mut output = String::new();
    reader
        .read_to_string(&mut output)
        .map_err(|error| format!("cannot read the probe's output: {error}"))?;
    Ok((status.code(), output))
}

/// The check, and how many variables it reads: each cache folder variable names a folder the
/// sandbox lets it write (in a subshell: a failed redirection of `:` ends the shell itself),
/// each fixed value (the compiler wrappers turned off) holds it, and
/// each configuration file variable names a file it can read. Every name and value is
/// Torlek's own table's, never the project's.
fn check_script() -> (String, usize) {
    let mut script = String::from(
        "failed=0\n\
         folder() { if [ -z \"$2\" ]; then echo \"$1 is not set\"; failed=1; \
         elif ! mkdir -p \"$2\" 2>/dev/null || ! (: > \"$2/.torlek-probe\") 2>/dev/null; then \
         echo \"$1=$2 is not writable\"; failed=1; else rm -f \"$2/.torlek-probe\"; fi; }\n\
         value() { if [ \"$2\" != \"$3\" ]; then echo \"$1 is '$2', not '$3'\"; failed=1; fi; }\n\
         file() { if [ ! -r \"$2\" ]; then echo \"$1=$2 is not a readable file\"; failed=1; fi; }\n",
    );
    let mut checked = Vec::new();
    for (name, setting) in COMMON_VARIABLES
        .iter()
        .chain(TOOLCHAINS.iter().flat_map(|toolchain| toolchain.variables))
    {
        if checked.contains(name) {
            continue;
        }
        let line = match setting {
            Setting::CacheFolder(_) => format!("folder {name} \"${{{name}-}}\"\n"),
            Setting::Value(value) => {
                format!("value {name} \"${{{name}-unset}}\" '{value}'\n")
            }
            Setting::ConfigFile(_) => format!("file {name} \"${{{name}-}}\"\n"),
            // The owner's own flags stay beside it, so there is no one value to compare.
            Setting::Flag(_) => continue,
        };
        script.push_str(&line);
        checked.push(name);
    }
    script.push_str(&format!(
        "folder HOME \"${{HOME-}}\"\n[ \"$failed\" = 0 ] && echo '{PASSED}'\nexit \"$failed\"\n"
    ));
    (script, checked.len() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_script(environment: &[(&str, &str)]) -> std::process::Output {
        let (script, _) = check_script();
        Command::new(SHELL)
            .arg("-c")
            .arg(script)
            .env_clear()
            .envs(environment.iter().copied())
            .output()
            .unwrap()
    }

    /// Every variable the table sets, as a launch sets it, under `root`.
    fn as_launched(root: &Path) -> Vec<(String, String)> {
        let mut environment = vec![("HOME".to_owned(), root.join("home").display().to_string())];
        for (name, setting) in COMMON_VARIABLES
            .iter()
            .chain(TOOLCHAINS.iter().flat_map(|toolchain| toolchain.variables))
        {
            let value = match setting {
                Setting::CacheFolder(folder) => {
                    root.join("caches").join(folder).display().to_string()
                }
                Setting::Value(value) => (*value).to_owned(),
                Setting::ConfigFile(file) => {
                    let path = root.join(file);
                    std::fs::write(&path, "").unwrap();
                    path.display().to_string()
                }
                Setting::Flag(flag) => (*flag).to_owned(),
            };
            environment.push(((*name).to_owned(), value));
        }
        environment
    }

    fn borrowed(environment: &[(String, String)]) -> Vec<(&str, &str)> {
        environment
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect()
    }

    #[test]
    fn the_check_passes_on_the_variables_a_launch_sets() {
        let root = tempfile::tempdir().unwrap();
        let environment = as_launched(root.path());

        let output = run_script(&borrowed(&environment));

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("{PASSED}\n")
        );
    }

    #[test]
    fn the_check_names_each_variable_that_does_not_hold() {
        let root = tempfile::tempdir().unwrap();
        let mut environment = as_launched(root.path());
        environment.retain(|(name, _)| name != "CARGO_HOME");
        for (name, value) in &mut environment {
            if name == "RUSTC_WRAPPER" {
                *value = "sccache".to_owned();
            }
            if name == "PIP_CONFIG_FILE" {
                *value = root.path().join("missing").display().to_string();
            }
        }

        let output = run_script(&borrowed(&environment));

        assert_eq!(output.status.code(), Some(1));
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(printed.contains("CARGO_HOME is not set\n"), "{printed}");
        assert!(
            printed.contains("RUSTC_WRAPPER is 'sccache', not ''\n"),
            "{printed}"
        );
        assert!(printed.contains("PIP_CONFIG_FILE="), "{printed}");
        assert!(!printed.contains(PASSED), "{printed}");
    }

    #[test]
    fn the_check_names_a_cache_folder_it_cannot_write() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let mut environment = as_launched(root.path());
        let locked = root.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        for (name, value) in &mut environment {
            if name == "GOCACHE" {
                *value = locked.display().to_string();
            }
        }

        let output = run_script(&borrowed(&environment));

        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(
            printed.contains(&format!("GOCACHE={} is not writable", locked.display())),
            "{printed}"
        );
    }
}
