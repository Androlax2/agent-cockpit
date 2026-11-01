//! Running a program with a deadline: `opencode --version` at discovery and the bridge's
//! `--version` at the daemon's start. The same runner as `cockpit-claude`'s: adapters depend
//! on the provider contract only, so they cannot share it.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The trimmed standard output of `program argument`, or why it failed. Kills it at the
/// deadline, and stops waiting for its output there too: a wrapper may leave a background
/// process holding its pipes.
pub(crate) fn run_with_deadline(
    program: &Path,
    argument: &str,
    deadline: Duration,
) -> Result<String, String> {
    let mut child = Command::new(program)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run it: {error}"))?;
    let gave_up = format!(
        "`{argument}` did not answer within {} ms",
        deadline.as_millis()
    );
    let ends_at = Instant::now() + deadline;
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for it: {error}"))?
        {
            break status;
        }
        if Instant::now() > ends_at {
            let _ = child.kill();
            let _ = child.wait();
            return Err(gave_up);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let remaining = || ends_at.saturating_duration_since(Instant::now());
    let stdout = stdout
        .recv_timeout(remaining())
        .map_err(|_| gave_up.clone())?;
    let stderr = stderr
        .recv_timeout(remaining())
        .map_err(|_| gave_up.clone())?;
    if !status.success() {
        return Err(format!("`{argument}` failed ({status}): {}", stderr.trim()));
    }
    Ok(stdout.trim().to_owned())
}

/// The whole of `pipe` as text, read on a thread of its own so a full pipe never stalls
/// the child.
fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}
