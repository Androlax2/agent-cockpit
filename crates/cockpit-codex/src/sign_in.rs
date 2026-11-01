use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_provider_api::{SignIn, SignInStatus};

/// Version measured by the isolated sign-in conformance test, independently of delivery.
pub const SIGN_IN_CONFORMANCE_VERSION: &str = "0.166.0";

impl crate::Codex {
    /// Probe the harness's local credentials, giving up after `deadline`. No model request
    /// or login is started; raw output never escapes this adapter.
    pub fn sign_in_within(&self, deadline: Duration) -> SignIn {
        let unknown = |reason: String| {
            SignIn::unknown(
                reason,
                "run `codex login status` in a terminal and check its result",
            )
        };
        let output = match status_output(&self.program, &["login", "status"], deadline) {
            Ok(output) => output,
            Err(reason) => return unknown(reason),
        };
        let Ok(stderr) = std::str::from_utf8(&output.stderr) else {
            return unknown("`codex login status` returned unreadable output".to_owned());
        };
        // Codex also exits 1 for broken configuration or credential stores. Only the
        // explicit negative answer is evidence of being signed out.
        let answer = stderr.lines().last().unwrap_or_default().trim();
        let signed_in = match output.status.code() {
            Some(0)
                if stderr.lines().any(|line| {
                    line.starts_with("Logged in using ")
                        || line.starts_with("API key configured - ")
                }) =>
            {
                true
            }
            Some(1) if answer == "Not logged in" => false,
            _ => {
                return unknown(
                    "`codex login status` did not report a recognized sign-in result".to_owned(),
                );
            }
        };
        SignIn {
            status: if signed_in {
                SignInStatus::SignedIn
            } else {
                SignInStatus::NotSignedIn {
                    next: "run `codex login` in a terminal, then run `cockpit doctor` again"
                        .to_owned(),
                }
            },
            provenance: match self.version.as_deref() {
                Some(SIGN_IN_CONFORMANCE_VERSION) => cockpit_provider_api::Provenance::Tested {
                    version: SIGN_IN_CONFORMANCE_VERSION.to_owned(),
                },
                _ => cockpit_provider_api::Provenance::Discovered,
            },
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
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|_| "cannot run the status command".to_owned())?;
    let ends_at = Instant::now() + deadline;
    let result = (|| {
        let mut output = Vec::new();
        let mut error = Vec::new();
        loop {
            let out_done = read_status_pipe(&mut stdout, &mut output)?;
            let err_done = read_status_pipe(&mut stderr, &mut error)?;
            let status = child
                .try_wait()
                .map_err(|_| "cannot wait for the status command")?;
            if let Some(status) = status.filter(|_| out_done && err_done) {
                return Ok(std::process::Output {
                    status,
                    stdout: output,
                    stderr: error,
                });
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
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
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
