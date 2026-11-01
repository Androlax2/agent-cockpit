use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_provider_api::{SignIn, SignInStatus};

impl crate::OpenCode {
    /// Probe the harness's local credentials, giving up after `deadline`. No model request
    /// or login is started; raw output never escapes this adapter.
    pub fn sign_in_within(&self, deadline: Duration) -> SignIn {
        let unknown = |reason: String| {
            SignIn::unknown(
                reason,
                "run `opencode auth list` in a terminal and check its result",
            )
        };
        let output = match status_output(&self.program, &["auth", "list"], deadline) {
            Ok(output) => output,
            Err(reason) => return unknown(reason),
        };
        if !output.status.success() {
            return unknown("`opencode auth list` failed".to_owned());
        }
        let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
            return unknown("`opencode auth list` returned unreadable output".to_owned());
        };
        let Some(signed_in) = credential_counts(stdout) else {
            return unknown(
                "`opencode auth list` did not report recognized credential counts".to_owned(),
            );
        };
        SignIn {
            status: if signed_in {
                SignInStatus::SignedIn
            } else {
                SignInStatus::NotSignedIn { next: "run `opencode auth login` to connect a provider, or select a credential-free model in OpenCode".to_owned() }
            },
            provenance: cockpit_provider_api::Provenance::Discovered,
        }
    }
}

// v1.19.5's providers.list prints counts for saved credentials and then environment
// credentials. Do not infer authentication from paths or arbitrary provider-name lines.
fn credential_counts(output: &str) -> Option<bool> {
    let mut plain = String::new();
    let mut characters = output.chars();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            if characters.next()? != '[' {
                return None;
            }
            characters.find(|c| ('@'..='~').contains(c))?;
        } else {
            plain.push(character);
        }
    }
    let mut credentials = None;
    let mut environment = None;
    for line in plain.lines() {
        let Some(summary) = line.trim().strip_prefix('└') else {
            continue;
        };
        let (count, kind) = summary.trim().split_once(' ')?;
        let count = count.parse::<usize>().ok()?;
        let slot = match kind.trim() {
            "credentials" => &mut credentials,
            "environment variable" | "environment variables" => &mut environment,
            _ => return None,
        };
        if slot.replace(count).is_some() {
            return None;
        }
    }
    Some(credentials? > 0 || environment.unwrap_or(0) > 0)
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
