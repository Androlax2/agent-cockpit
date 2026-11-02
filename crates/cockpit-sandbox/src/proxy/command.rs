//! Bounded service/HTTP execution. Nonblocking sockets avoid detached pipe-reader threads.
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 64 * 1024;
pub(super) type LineFilter<'a> = &'a dyn Fn(&[u8]) -> bool;

pub(super) fn run(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, String> {
    run_filtered(command, input, timeout, None)
}

/// Stream namespace tables without retaining unrelated rows. Both retained output and
/// individual lines remain bounded; the deadline also applies while discarding output.
pub(super) fn run_filtered(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
    keep_line: Option<LineFilter<'_>>,
) -> Result<Output, String> {
    if input.is_some_and(|input| input.len() > 256) {
        return Err("command input exceeded its limit".into());
    }
    let pair = || UnixStream::pair().map_err(|error| error.to_string());
    let (stdout, stdout_child) = pair()?;
    let (stderr, stderr_child) = pair()?;
    stdout
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    stderr
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let mut child = command
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(OwnedFd::from(stdout_child)))
        .stderr(Stdio::from(OwnedFd::from(stderr_child)))
        .spawn()
        .map_err(|error| format!("cannot start command: {error}"))?;
    // Command retains supplied descriptors for reuse; release its copies so EOF is observable.
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    let deadline = Instant::now() + timeout;
    let result = (|| {
        if let Some(input) = input {
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(input)
                .map_err(|error| format!("cannot write command input: {error}"))?;
        }
        let mut streams = [stdout, stderr];
        let mut bytes = [Vec::new(), Vec::new()];
        let mut pending_line = Vec::new();
        let mut closed = [false; 2];
        loop {
            for index in 0..2 {
                if closed[index] {
                    continue;
                }
                let mut buffer = [0; 8192];
                loop {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "command did not finish within {} ms",
                            timeout.as_millis()
                        ));
                    }
                    match streams[index].read(&mut buffer) {
                        Ok(0) => {
                            if index == 0 && !pending_line.is_empty() {
                                if keep_line.is_some_and(|keep| keep(&pending_line)) {
                                    append(&mut bytes[index], &pending_line)?;
                                }
                                pending_line.clear();
                            }
                            closed[index] = true;
                            break;
                        }
                        Ok(count) => {
                            if index == 0
                                && let Some(keep) = keep_line
                            {
                                for part in buffer[..count].split_inclusive(|byte| *byte == b'\n') {
                                    if pending_line.len() + part.len() > 4096 {
                                        return Err("command table line exceeded 4 KiB".into());
                                    }
                                    pending_line.extend_from_slice(part);
                                    if part.last() == Some(&b'\n') {
                                        if keep(&pending_line) {
                                            append(&mut bytes[index], &pending_line)?;
                                        }
                                        pending_line.clear();
                                    }
                                }
                            } else {
                                append(&mut bytes[index], &buffer[..count])?;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error.to_string()),
                    }
                }
            }
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())?
                && closed.into_iter().all(|closed| closed)
            {
                let [stdout, stderr] = bytes;
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "command did not finish within {} ms",
                    timeout.as_millis()
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        // Only the new process group we created, including a descendant holding output open.
        kill_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn append(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), String> {
    if output.len() + bytes.len() > OUTPUT_LIMIT {
        return Err("command output exceeded 64 KiB".into());
    }
    output.extend_from_slice(bytes);
    Ok(())
}

fn kill_group(pid: u32) {
    let Ok(mut kill) = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        if matches!(kill.try_wait(), Ok(Some(_))) {
            return;
        }
        if Instant::now() >= deadline {
            let _ = kill.kill();
            let _ = kill.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
