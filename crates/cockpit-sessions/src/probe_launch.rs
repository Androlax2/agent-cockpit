//! `torlek doctor`'s probe launch (#833): the profile a session of a provider would get, run
//! for real through the native sandbox with a shell check in place of the agent, which reads
//! each per-session cache variable of the toolchain table (#859) inside it. Nothing of the
//! provider runs, no prompt is sent and no credential is staged; the dry run's folders are
//! removed after.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use cockpit_provider_api::toolchains::{COMMON_VARIABLES, Setting, TOOLCHAINS};

use super::{DRY_RUN_SESSION, Sessions, create_session_dir, docker_config_dir, remove_tree};

/// How long one probe may run: bubblewrap and the check take well under a second.
const PROBE_DEADLINE: Duration = Duration::from_secs(10);

/// The shell the check runs in, inside the sandbox's system folders.
const SHELL: &str = "/bin/sh";

/// What starts each line the check prints, so its findings are told from the launcher's.
const PREFIX: &str = "torlek probe: ";

/// The check's line when every variable holds.
const PASSED: &str = "torlek probe: passed";

/// What a probe launch found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeLaunch {
    /// The check ran in the sandbox and every variable held: what it checked.
    Passed(String),
    /// The launch or its check failed: what it found.
    Failed(String),
    /// No probe ran, for this reason: there was no time left, or the daemon was too busy.
    NotRun(String),
}

impl Sessions {
    /// Runs `prepared`, a sandboxed profile [`Sessions::sandbox_preflight`] built, with the
    /// check in place of its program, ending by `until`. One probe runs at a time, since
    /// every probe stages the same dry-run folders: the lock is held until its blocking step
    /// returns, not until its caller stops waiting.
    pub async fn probe_launch(
        &self,
        prepared: cockpit_sandbox::Prepared,
        until: Instant,
    ) -> ProbeLaunch {
        let lock = Arc::clone(&self.probe_launches);
        let Ok(held) = tokio::time::timeout_at(until.into(), lock.lock_owned()).await else {
            return ProbeLaunch::NotRun(
                "another probe launch held the dry run until this one's time was up".to_owned(),
            );
        };
        let Some(deadline) = until
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .map(|left| Instant::now() + left.min(PROBE_DEADLINE))
        else {
            return ProbeLaunch::NotRun(
                "the probe launches before it used the time `torlek doctor` gives them".to_owned(),
            );
        };
        let session_dir = self.session_dirs.join(DRY_RUN_SESSION);
        let temp_dir = self.dry_run_temp_dir();
        let sandbox = self.settings.sandbox.clone();
        let ran = self
            .blocking("run the probe launch", move || {
                let _held = held;
                let staged = prepared
                    .staged
                    .as_ref()
                    .map(|staged| (staged.mount_dir.clone(), staged.home_source.clone()));
                // The folders a launch creates before it starts the agent, as it creates them.
                let created = [&session_dir, &temp_dir, &docker_config_dir(&temp_dir)]
                    .into_iter()
                    .try_for_each(|folder| create_session_dir(folder))
                    .map_err(|error| format!("cannot create the probe launch's folders: {error}"));
                let found = created.and_then(|()| run(prepared, deadline));
                for folder in [&session_dir, &temp_dir] {
                    remove_folder(folder);
                }
                if let Some((mount_dir, home_source)) = staged {
                    remove_folder(&home_source);
                    // The network loop rewrites the decisions file in it: removed under that
                    // loop's lock, as a session's is (#1003).
                    let _writes = sandbox.hold_mount_writes();
                    remove_folder(&mount_dir);
                }
                Ok(found)
            })
            .await;
        match ran {
            Ok(Ok(checked)) => ProbeLaunch::Passed(checked),
            Ok(Err(found)) => ProbeLaunch::Failed(found),
            Err(error) => ProbeLaunch::NotRun(error.to_string()),
        }
    }
}

fn remove_folder(folder: &Path) {
    match remove_tree(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(folder = %folder.display(), %error, "the probe launch's folder is left");
        }
    }
}

/// What the check found when it passed; its own lines, or the launcher's, when not.
fn run(prepared: cockpit_sandbox::Prepared, deadline: Instant) -> Result<String, String> {
    let Some(mut staged) = prepared.staged else {
        return Err("the profile runs unsandboxed, so there is no sandbox to probe".to_owned());
    };
    let (script, checked) = check_script();
    staged.spec.program = PathBuf::from(SHELL);
    staged.spec.arguments = vec!["-c".to_owned(), script, "torlek-probe".to_owned()];
    // The shell reaches no provider, so no credential is written to disk for it.
    staged.credential_values.clear();
    staged.spec.interception = None;
    // A project's container engine is the launch's to reach, through a helper this probe
    // has no part in.
    staged.spec.engine = None;
    staged
        .write()
        .map_err(|error| format!("cannot stage the probe launch: {error}"))?;
    let (status, output) = run_until(
        &prepared.program,
        &prepared.arguments,
        &prepared.environment,
        &staged.spec.working_dir,
        deadline,
    )?;
    judge(status, &output, checked)
}

/// The probe's finding from the launcher's exit and what it printed: passed only on exit 0
/// with the check's passing line, wherever the launcher's own lines fall around it. A
/// failure names the check's lines, or the launcher's when the check printed none.
fn judge(status: Option<i32>, output: &str, checked: usize) -> Result<String, String> {
    let own: Vec<&str> = output
        .lines()
        .filter(|line| line.starts_with(PREFIX))
        .collect();
    if status == Some(0) && own.contains(&PASSED) {
        return Ok(format!(
            "ran in the sandbox: {checked} per-session variables of the toolchain table hold, \
             each cache folder writable"
        ));
    }
    let exit = status.map_or_else(|| "a signal".to_owned(), |code| format!("exit {code}"));
    let found: Vec<&str> = if own.is_empty() {
        output.lines().map(str::trim).collect()
    } else {
        own.iter()
            .filter(|line| **line != PASSED)
            .filter_map(|line| line.strip_prefix(PREFIX))
            .collect()
    };
    let found = found
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    Err(if found.is_empty() {
        format!("the probe launch ended with {exit} and printed nothing")
    } else {
        format!("the probe launch ended with {exit}: {found}")
    })
}

/// Starts the launcher with exactly `environment`, and its exit code (none for a signal) with
/// what it printed on both streams, read as it prints so a full pipe never stalls it. One
/// that outlives `deadline` is killed; its bubblewrap dies with it (`--die-with-parent`).
fn run_until(
    program: &Path,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    working_dir: &Path,
    deadline: Instant,
) -> Result<(Option<i32>, String), String> {
    let cannot_open = |error| format!("cannot open the probe's output: {error}");
    let (mut reader, writer) = std::io::pipe().map_err(cannot_open)?;
    let mut child = Command::new(program)
        .args(arguments)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(cannot_open)?)
        .stderr(writer)
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
    let (read_tx, read_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let read = reader.read_to_end(&mut output).map(|_| output);
        // The receiver is gone once the probe gave up on it: nothing is left to tell.
        let _ = read_tx.send(read);
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                stop(&mut child);
                return Err(format!(
                    "the probe launch did not end within {} seconds",
                    PROBE_DEADLINE.as_secs()
                ));
            }
            Err(error) => {
                stop(&mut child);
                return Err(format!("cannot wait for the probe launch: {error}"));
            }
        }
    };
    // The command held the pipe's writing ends; they close with it, so the read ends.
    drop(child);
    let left = deadline.saturating_duration_since(Instant::now());
    let output = match read_rx.recv_timeout(left.max(Duration::from_millis(500))) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("cannot read the probe's output: {error}")),
        Err(_) => {
            return Err("the probe launch ended, but its output stayed open".to_owned());
        }
    };
    Ok((status.code(), String::from_utf8_lossy(&output).into_owned()))
}

fn stop(child: &mut std::process::Child) {
    if let Err(error) = child.kill() {
        tracing::warn!(pid = child.id(), %error, "the probe launch was not killed");
    }
    if let Err(error) = child.wait() {
        tracing::warn!(pid = child.id(), %error, "the probe launch was not reaped");
    }
}

/// The check, and how many variables it reads: each cache folder variable names a folder the
/// sandbox lets it write (in a subshell: a failed redirection of `:` ends the shell itself),
/// each fixed value (the compiler wrappers turned off) holds it, and each configuration file
/// variable names a file it can read. Every name and value is Torlek's own table's, never
/// the project's. Each line it prints starts with [`PREFIX`].
fn check_script() -> (String, usize) {
    let mut script = format!(
        "failed=0\n\
         say() {{ echo \"{PREFIX}$*\"; }}\n\
         folder() {{ if [ -z \"$2\" ]; then say \"$1 is not set\"; failed=1; \
         elif ! mkdir -p \"$2\" 2>/dev/null || ! (: > \"$2/.torlek-probe\") 2>/dev/null; then \
         say \"$1=$2 is not writable\"; failed=1; else rm -f \"$2/.torlek-probe\"; fi; }}\n\
         value() {{ if [ \"$2\" != \"$3\" ]; then say \"$1 is '$2', not '$3'\"; failed=1; fi; }}\n\
         file() {{ if [ ! -r \"$2\" ]; then say \"$1=$2 is not a readable file\"; failed=1; fi; }}\n"
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
    fn a_passing_check_passes_whatever_the_launcher_prints_around_it() {
        let output = format!("sandbox: enforced by torlek-sandbox\n{PASSED}\nproxy: closed\n");

        let found = judge(Some(0), &output, 15);

        assert!(found.is_ok_and(|checked| checked.contains("15 per-session")));
    }

    #[test]
    fn a_failed_check_names_its_own_lines_without_the_launchers() {
        let output = "sandbox: enforced by torlek-sandbox\ntorlek probe: CARGO_HOME is not set\n";

        let found = judge(Some(1), output, 15);

        assert_eq!(
            found,
            Err("the probe launch ended with exit 1: CARGO_HOME is not set".to_owned())
        );
    }

    #[test]
    fn a_launcher_that_failed_before_the_check_is_named() {
        let output = "torlek-sandbox: cannot bind /run/dry-run: No such file\n";

        let found = judge(Some(125), output, 15);

        assert_eq!(
            found,
            Err(
                "the probe launch ended with exit 125: torlek-sandbox: cannot bind /run/dry-run: \
                 No such file"
                    .to_owned()
            )
        );
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
        assert!(
            printed.contains("torlek probe: CARGO_HOME is not set\n"),
            "{printed}"
        );
        assert!(
            printed.contains("torlek probe: RUSTC_WRAPPER is 'sccache', not ''\n"),
            "{printed}"
        );
        assert!(
            printed.contains("torlek probe: PIP_CONFIG_FILE="),
            "{printed}"
        );
        assert!(!printed.contains(PASSED), "{printed}");
    }

    #[test]
    fn the_check_names_a_cache_folder_it_cannot_write() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let root = tempfile::tempdir().unwrap();
        if std::fs::metadata(root.path()).unwrap().uid() == 0 {
            // Root writes a folder whatever its mode, so a mode cannot deny it the write.
            eprintln!("skipped: running as root");
            return;
        }
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
            printed.contains(&format!(
                "torlek probe: GOCACHE={} is not writable",
                locked.display()
            )),
            "{printed}"
        );
    }
}
