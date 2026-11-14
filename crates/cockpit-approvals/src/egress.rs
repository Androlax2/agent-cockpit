//! A confined run whose card names hosts (#1387) reaches them through a proxy of its own,
//! `torlek-sandbox run-egress`, on the host's network. The run's service keeps a network
//! namespace of its own with nothing but loopback: its first program, `torlek-sandbox
//! hand-over`, listens on the proxy address there and hands the listener to the proxy, which
//! lets out only the card's hosts. Once the run is over the proxy names each destination it
//! refused, which the record keeps among the run's limit hits.
//!
//! Each run's hand-off socket is in a folder of its own that only its service is shown: a run
//! that reached another's could hand that proxy its own listener and so the other card's hosts.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Child;

use crate::runs::Approved;

/// How long the proxy has to say it is ready, and to name what it refused once the run is over.
const PROXY_DEADLINE: Duration = Duration::from_secs(10);

/// What `run-egress` prints once the run can hand its listener over, and before each
/// destination it refused.
const READY: &str = "ready";
const REFUSED: &str = "refused ";

/// Where the proxy listens in the run's network namespace, as `cockpit_sandbox::PROXY_ADDRESS`.
const PROXY_URL: &str = "http://127.0.0.1:3128";

/// The variables that send clients through the proxy, and those that keep the run's own
/// loopback direct: the sandbox's (`cockpit_sandbox`'s `native.rs`).
const PROXY_VARIABLES: [&str; 6] = [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
];
const NO_PROXY_VARIABLES: [&str; 2] = ["NO_PROXY", "no_proxy"];
const NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// The proxy confined runs reach their card's hosts through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressProxy {
    /// `torlek-sandbox`.
    pub launcher: PathBuf,
    /// Where each run's folder is made: a folder confined runs are shown empty, so no run sees
    /// another's.
    pub folders: PathBuf,
}

/// One run's proxy, serving until [`RunEgress::end`].
pub(crate) struct RunEgress {
    child: Child,
    /// Read off the runtime's threads: blocking, as its descriptor is.
    stdout: Option<BufReader<File>>,
    folder: PathBuf,
    launcher: PathBuf,
}

impl EgressProxy {
    /// Starts `run`'s proxy, serving the hosts its card names, once it is ready for the run's
    /// listener.
    pub(crate) async fn start(&self, run: &Approved) -> Result<RunEgress, String> {
        let folder = self.folders.join(format!("run-{}", run.id.0));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&folder)
            .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
        let mut child = tokio::process::Command::new(&self.launcher)
            .arg("run-egress")
            .arg(&folder)
            .arg(run.session.as_str())
            .arg(run.id.0.to_string())
            .args(&run.reach.hosts)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", self.launcher.display()))?;
        let stdout = child
            .stdout
            .take()
            .expect("its stdout is piped")
            .into_owned_fd()
            .map_err(|error| format!("cannot read the run's proxy: {error}"))?;
        let mut egress = RunEgress {
            child,
            stdout: None,
            folder,
            launcher: self.launcher.clone(),
        };
        let reading = tokio::task::spawn_blocking(move || {
            let mut stdout = BufReader::new(File::from(stdout));
            let mut line = String::new();
            let read = stdout.read_line(&mut line).map(|_| line);
            (stdout, read)
        });
        match tokio::time::timeout(PROXY_DEADLINE, reading).await {
            Ok(Ok((stdout, Ok(line)))) if line.trim_end() == READY => {
                egress.stdout = Some(stdout);
                Ok(egress)
            }
            Ok(Ok((_, Ok(_)))) => Err(format!(
                "the run's proxy did not start: {}",
                egress.failure().await
            )),
            Ok(Ok((_, Err(error)))) => Err(format!("cannot read the run's proxy: {error}")),
            Ok(Err(error)) => Err(format!("cannot read the run's proxy: {error}")),
            // Dropping `egress` kills the proxy, which ends the read.
            Err(_) => Err(format!(
                "the run's proxy was not ready within {}s",
                PROXY_DEADLINE.as_secs()
            )),
        }
    }
}

impl RunEgress {
    /// The folder the run's service is shown, read-only: the hand-off socket's.
    pub(crate) fn handoff_folder(&self) -> PathBuf {
        self.folder.join("proxy-handoff")
    }

    /// The program that hands the listener over then runs `leader` with `arguments` in its
    /// place, as the leader and its arguments.
    pub(crate) fn handing_over(
        &self,
        leader: &Path,
        arguments: &[OsString],
    ) -> (PathBuf, Vec<OsString>) {
        let mut handing = vec![
            OsString::from("hand-over"),
            self.handoff_folder().join("handoff.sock").into_os_string(),
            leader.as_os_str().to_owned(),
        ];
        handing.extend(arguments.iter().cloned());
        (self.launcher.clone(), handing)
    }

    /// Ends the proxy, the run being over: each destination it refused.
    pub(crate) async fn end(mut self) -> Result<Vec<String>, String> {
        drop(self.child.stdin.take());
        let mut stdout = self
            .stdout
            .take()
            .expect("a started proxy's stdout is kept");
        let reading = tokio::task::spawn_blocking(move || {
            let mut rest = String::new();
            stdout.read_to_string(&mut rest).map(|_| rest)
        });
        let read = tokio::time::timeout(PROXY_DEADLINE, reading).await;
        let _ = self.child.kill().await;
        let _ = std::fs::remove_dir_all(&self.folder);
        match read {
            Ok(Ok(Ok(rest))) => Ok(rest
                .lines()
                .filter_map(|line| line.strip_prefix(REFUSED))
                .map(str::to_owned)
                .collect()),
            Ok(Ok(Err(error))) => Err(format!("cannot read the run's proxy: {error}")),
            Ok(Err(error)) => Err(format!("cannot read the run's proxy: {error}")),
            Err(_) => Err(format!(
                "the run's proxy named nothing within {}s",
                PROXY_DEADLINE.as_secs()
            )),
        }
    }

    /// How the proxy ended when it said something else than ready.
    async fn failure(&mut self) -> String {
        match tokio::time::timeout(PROXY_DEADLINE, self.child.wait()).await {
            Ok(Ok(status)) => format!("it exited with {status}"),
            Ok(Err(error)) => error.to_string(),
            Err(_) => "it answered something else".to_owned(),
        }
    }
}

impl Drop for RunEgress {
    fn drop(&mut self) {
        // The child is killed on drop; its folder goes with it.
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The variables that send the run's clients through its proxy.
pub(crate) fn proxy_variables() -> Vec<(OsString, OsString)> {
    PROXY_VARIABLES
        .iter()
        .map(|name| (OsString::from(name), OsString::from(PROXY_URL)))
        .chain(
            NO_PROXY_VARIABLES
                .iter()
                .map(|name| (OsString::from(name), OsString::from(NO_PROXY))),
        )
        .collect()
}

/// Whether `name` is one of the variables [`proxy_variables`] sets.
pub(crate) fn is_proxy_variable(name: &std::ffi::OsStr) -> bool {
    PROXY_VARIABLES
        .iter()
        .chain(&NO_PROXY_VARIABLES)
        .any(|variable| name == *variable)
}
