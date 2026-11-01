//! Which Docker engine this user's `docker` reaches, and whether it is rootless (report 11):
//! a `docker` program is not an engine. Moves to `cockpit-containers` with #390.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long `docker info` may take before the engine counts as unreachable.
pub const DOCKER_DEADLINE: Duration = Duration::from_secs(10);

/// The engine `docker info` answered for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerEngine {
    /// The engine's own ID.
    pub id: String,
    /// The host name the engine reports.
    pub name: String,
    pub server_version: String,
    /// The engine runs as this user, not as root (its security options name `rootless`).
    pub rootless: bool,
    /// Docker Desktop: the engine runs in a Linux VM of its own, not on this host.
    pub in_desktop_vm: bool,
}

/// Asks `program` for its engine's identity, within `deadline`. The reason when it does not
/// answer as an engine.
pub fn probe_docker(program: &Path, deadline: Duration) -> Result<DockerEngine, String> {
    let shown = program.display();
    let (succeeded, stdout, stderr) = run(program, &["info", "--format", "{{json .}}"], deadline)?;
    if !succeeded {
        let reason = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("it exited unsuccessfully");
        return Err(format!(
            "{shown} info: no engine answered: {}",
            reason.trim()
        ));
    }
    parse_info(&stdout).map_err(|reason| format!("{shown} info: {reason}"))
}

/// `docker info --format '{{json .}}'`.
fn parse_info(output: &str) -> Result<DockerEngine, String> {
    let info: serde_json::Value = serde_json::from_str(output.trim())
        .map_err(|error| format!("not the JSON it prints: {error}"))?;
    if let Some(error) = info
        .get("ServerErrors")
        .and_then(serde_json::Value::as_array)
        .and_then(|errors| errors.first())
        .and_then(serde_json::Value::as_str)
    {
        return Err(format!("no engine answered: {error}"));
    }
    let text = |field: &str| {
        info.get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("its answer names no {field}"))
    };
    let rootless = info
        .get("SecurityOptions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|options| {
            options
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(|option| option.split(',').any(|part| part == "name=rootless"))
        });
    Ok(DockerEngine {
        id: text("ID")?,
        name: text("Name")?,
        server_version: text("ServerVersion")?,
        rootless,
        in_desktop_vm: info
            .get("OperatingSystem")
            .and_then(serde_json::Value::as_str)
            == Some("Docker Desktop"),
    })
}

/// Runs `program` with `arguments`, reading its output on threads of its own, so a full pipe
/// or a child holding the pipes open cannot stall the caller past `deadline`.
fn run(
    program: &Path,
    arguments: &[&str],
    deadline: Duration,
) -> Result<(bool, String, String), String> {
    let shown = program.display();
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{shown} does not run: {error}"))?;
    let read = |stream: Option<Box<dyn Read + Send>>| {
        let (sender, text) = std::sync::mpsc::channel();
        if let Some(mut stream) = stream {
            std::thread::spawn(move || {
                let mut collected = String::new();
                let read = stream
                    .read_to_string(&mut collected)
                    .map(|_| collected)
                    .map_err(|error| error.to_string());
                let _ = sender.send(read);
            });
        }
        text
    };
    let stdout = read(
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn Read + Send>),
    );
    let stderr = read(
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn Read + Send>),
    );
    let until = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{shown} {} did not answer within {}s",
                    arguments[0],
                    deadline.as_secs()
                ));
            }
            Err(error) => return Err(format!("{shown} could not be waited for: {error}")),
        }
    };
    // A helper `docker` started may hold the pipes after it exits; its output is then cut.
    let collect = |text: std::sync::mpsc::Receiver<Result<String, String>>| match text
        .recv_timeout(Duration::from_secs(1))
    {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(error)) => Err(format!("{shown}'s output could not be read: {error}")),
        Err(_) => Err(format!(
            "{shown}'s output stayed open after it exited, so its answer is incomplete"
        )),
    };
    Ok((status.success(), collect(stdout)?, collect(stderr)?))
}

#[cfg(test)]
mod tests {
    use super::parse_info;

    #[test]
    fn a_rootless_engine_is_recognized_among_its_security_options() {
        let engine = parse_info(
            r#"{"ID":"abc","Name":"host","ServerVersion":"28.1.1",
                "SecurityOptions":["name=seccomp,profile=builtin","name=rootless","name=cgroupns"]}"#,
        )
        .unwrap();

        assert!(engine.rootless);
        assert_eq!(engine.id, "abc");
    }

    #[test]
    fn an_engine_without_the_rootless_option_is_rootful() {
        let engine = parse_info(
            r#"{"ID":"abc","Name":"host","ServerVersion":"28.1.1",
                "SecurityOptions":["name=seccomp,profile=builtin"]}"#,
        )
        .unwrap();

        assert!(!engine.rootless);
        assert!(!engine.in_desktop_vm);
    }

    #[test]
    fn docker_desktop_is_recognized_by_its_operating_system() {
        let engine = parse_info(
            r#"{"ID":"abc","Name":"docker-desktop","ServerVersion":"28.1.1",
                "OperatingSystem":"Docker Desktop","SecurityOptions":["name=seccomp,profile=builtin"]}"#,
        )
        .unwrap();

        assert!(engine.in_desktop_vm);
    }

    #[test]
    fn a_client_whose_engine_is_down_names_the_server_error() {
        let error = parse_info(
            r#"{"ID":"","ServerErrors":["Cannot connect to the Docker daemon at unix:///run/user/1/docker.sock"]}"#,
        )
        .unwrap_err();

        assert!(
            error.contains("Cannot connect to the Docker daemon"),
            "{error}"
        );
    }
}
