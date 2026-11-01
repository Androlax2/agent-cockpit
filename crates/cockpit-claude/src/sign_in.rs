use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_provider_api::{SignIn, SignInStatus};

impl crate::ClaudeCode {
    /// Probe the harness's local credentials, giving up after `deadline`. No model request
    /// or login is started; raw output never escapes this adapter.
    pub fn sign_in_within(&self, deadline: Duration) -> SignIn {
        let unknown = |reason: String| {
            SignIn::unknown(
                reason,
                "run `claude auth status` in a terminal and check its result",
            )
        };
        let output = match status_output(&self.program, &["auth", "status"], deadline) {
            Ok(output) => output,
            Err(reason) => return unknown(reason),
        };
        let logged_in = serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .ok()
            .and_then(|value| value.get("loggedIn").and_then(serde_json::Value::as_bool));
        let signed_in = match (output.status.code(), logged_in) {
            (Some(0), Some(true)) => true,
            (Some(1), Some(false)) => false,
            _ => {
                return unknown(
                    "`claude auth status` did not report a recognized sign-in result".to_owned(),
                );
            }
        };
        SignIn {
            status: if signed_in {
                SignInStatus::SignedIn
            } else {
                SignInStatus::NotSignedIn {
                    next: "run `claude auth login` in a terminal, then run `cockpit doctor` again"
                        .to_owned(),
                }
            },
            provenance: self.provenance(),
        }
    }
}

/// Capture a status command without retaining more than 64 KiB per stream. Socket pairs
/// make reads nonblocking, so an inherited pipe cannot strand a reader thread after timeout.
fn status_output(
    program: &Path,
    arguments: &[&str],
    deadline: Duration,
) -> Result<std::process::Output, String> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let pair = || {
        let (read, write) = UnixStream::pair().map_err(|_| "cannot create output pipe")?;
        read.set_nonblocking(true)
            .map_err(|_| "cannot configure output pipe")?;
        Ok::<_, String>((read, Stdio::from(OwnedFd::from(write))))
    };
    let (mut stdout, out) = pair()?;
    let (mut stderr, err) = pair()?;
    let mut child = Command::new(program)
        .args(arguments)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|_| "cannot run the status command".to_owned())?;
    let pid = Pid::from_child(&child);
    let ends_at = Instant::now() + deadline;
    let result = (|| {
        let mut output = Vec::new();
        let mut error = Vec::new();
        loop {
            let out_done = read_status_pipe(&mut stdout, &mut output)?;
            let err_done = read_status_pipe(&mut stderr, &mut error)?;
            // Keep the leader waitable until group cleanup, so its pid cannot be reused
            // when a wrapper exits before its descendants close the output streams.
            let exited = rustix::process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map_err(|_| "cannot wait for the status command")?
            .is_some();
            if exited && out_done && err_done {
                return Ok((output, error));
            }
            if Instant::now() >= ends_at {
                return Err(format!(
                    "status command did not answer within {} ms",
                    deadline.as_millis()
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    // Status commands own no long-lived workers, including after a successful answer.
    let cleanup = rustix::process::kill_process_group(pid, Signal::KILL);
    if cleanup.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|_| "cannot reap the status command")?;
    match cleanup {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(_) => return Err("cannot stop the status command's process group".to_owned()),
    }
    let (stdout, stderr) = result?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn read_status_pipe(pipe: &mut impl Read, bytes: &mut Vec<u8>) -> Result<bool, String> {
    let mut buffer = [0; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if bytes.len() + count > 64 * 1024 {
                    return Err("status command output exceeded 64 KiB".to_owned());
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("cannot read the status command output".to_owned()),
        }
    }
}
