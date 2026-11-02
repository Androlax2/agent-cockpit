//! Bound helper commands even when a wrapper leaves descendants holding its output open.

use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group};

pub fn output(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    let pair = || {
        let (read, write) = UnixStream::pair().map_err(|_| "cannot create output pipe")?;
        read.set_nonblocking(true)
            .map_err(|_| "cannot configure output pipe")?;
        Ok::<_, String>((read, Stdio::from(OwnedFd::from(write))))
    };
    let (mut stdout, out) = pair()?;
    let (mut stderr, err) = pair()?;
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|_| "cannot run probe helper")?;
    // Command retains its Stdio handles; close our writers so EOF is observable.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let pid = Pid::from_child(&child);
    let ends_at = Instant::now() + timeout;
    let result = (|| {
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        loop {
            let out_done = read(&mut stdout, &mut stdout_bytes)?;
            let err_done = read(&mut stderr, &mut stderr_bytes)?;
            let exited = rustix::process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map_err(|_| "cannot wait for probe helper")?
            .is_some();
            if exited && out_done && err_done {
                return Ok((stdout_bytes, stderr_bytes));
            }
            if Instant::now() >= ends_at {
                return Err("probe helper timed out".to_owned());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    // Retain the waitable leader until its group is stopped, preventing PID reuse.
    let cleanup = kill_process_group(pid, Signal::KILL);
    if cleanup.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|_| "cannot reap probe helper")?;
    match cleanup {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(_) => return Err("cannot stop probe helper process group".to_owned()),
    }
    let (stdout, stderr) = result?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read(pipe: &mut impl Read, bytes: &mut Vec<u8>) -> Result<bool, String> {
    let mut buffer = [0; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if bytes.len() + count > 64 * 1024 {
                    return Err("probe helper output exceeded 64 KiB".to_owned());
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("cannot read probe helper output".to_owned()),
        }
    }
}

#[test]
fn a_wrapper_cannot_strand_the_probe_with_inherited_output() {
    let started = Instant::now();
    let error = output(
        Command::new("/bin/sh").args(["-c", "sleep 30 & exit 0"]),
        Duration::from_millis(150),
    )
    .unwrap_err();
    assert!(error.contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn excessive_helper_output_is_bounded() {
    let error = output(
        Command::new("/usr/bin/head").args(["-c", "70000", "/dev/zero"]),
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(error.contains("exceeded 64 KiB"));
}
