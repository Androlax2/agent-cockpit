//! The owner's desktop clipboard, read for CMD+V in the manager's viewer (#848): only its PNG,
//! only when the owner pressed the key, with wl-clipboard's `wl-paste` on the display the owner
//! imported. Agents never read it. Why: `docs/decisions/2026-11-05-848-image-paste-key.md`.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cockpit_protocol::{DESKTOP_SESSION_VARIABLES, ErrorCode, ProtocolError};

const PNG_TYPE: &str = "image/png";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// A larger image is refused rather than cut: a 4K screenshot is about 10 MiB.
const IMAGE_LIMIT_MIB: usize = 32;
/// The clipboard's owner, another program, answers each read; one that never does fails the
/// key instead of holding a daemon thread.
const READ_LIMIT: Duration = Duration::from_secs(5);
/// What `wl-paste` says, failing, when the clipboard is empty.
const NOTHING_COPIED: &str = "Nothing is copied";
/// What of `wl-paste`'s complaints is kept for the refusal.
const STDERR_LIMIT: usize = 4096;
/// What of the clipboard's list of types is read: a few lines in practice.
const TYPES_LIMIT: usize = 64 * 1024;

/// The clipboard's PNG, none when it holds no image, or why it could not be read. `desktop` is
/// the owner's last import; the desktop variables it lacks are unset for `reader`.
pub(crate) fn read_png(
    reader: &Path,
    desktop: &[(OsString, OsString)],
) -> Result<Option<Vec<u8>>, ProtocolError> {
    let has_wayland = desktop
        .iter()
        .any(|(name, value)| name == "WAYLAND_DISPLAY" && !value.is_empty());
    if !has_wayland {
        return Err(refused(
            "the daemon knows no Wayland display to read the clipboard on: run \
             `torlek desktop-session import` in a terminal of your desktop, then press CMD+V again"
                .to_owned(),
        ));
    }
    let types = run(reader, desktop, &["--list-types"], TYPES_LIMIT)?;
    if !types.status.success() {
        if types.stderr.contains(NOTHING_COPIED) {
            return Ok(None);
        }
        return Err(failed(reader, &types));
    }
    let has_png = String::from_utf8_lossy(&types.stdout)
        .lines()
        .any(|offered| offered == PNG_TYPE);
    if !has_png {
        return Ok(None);
    }
    let image_limit = IMAGE_LIMIT_MIB * 1024 * 1024;
    let image = run(
        reader,
        desktop,
        &["--no-newline", "--type", PNG_TYPE],
        image_limit,
    )?;
    // Before the status: cut short, `wl-paste` fails writing the rest.
    if image.stdout.len() > image_limit {
        return Err(refused(format!(
            "the clipboard's image is over {IMAGE_LIMIT_MIB} MiB: save it to a file and give the agent its path"
        )));
    }
    if !image.status.success() {
        return Err(failed(reader, &image));
    }
    if !image.stdout.starts_with(PNG_SIGNATURE) {
        return Err(refused(format!(
            "the clipboard's {PNG_TYPE} is not a PNG: its first bytes are not the PNG signature"
        )));
    }
    Ok(Some(image.stdout))
}

/// Saves `image` as a new `clipboard-<milliseconds>.png` in `folder`, readable and writable by
/// the owner only, and answers its path.
pub(crate) fn save(folder: &Path, image: &[u8]) -> Result<PathBuf, ProtocolError> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = folder.join(format!("clipboard-{milliseconds}.png"));
    let cannot = |error: std::io::Error| {
        ProtocolError::new(
            ErrorCode::Internal,
            format!(
                "cannot save the clipboard's image as {}: {error}",
                path.display()
            ),
        )
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(cannot)?;
    file.write_all(image).map_err(cannot)?;
    Ok(path)
}

struct Answer {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: String,
}

/// Runs `reader` with `arguments` for at most [`READ_LIMIT`], keeping up to one byte more than
/// `stdout_limit` of its output, so a caller sees it was over.
fn run(
    reader: &Path,
    desktop: &[(OsString, OsString)],
    arguments: &[&str],
    stdout_limit: usize,
) -> Result<Answer, ProtocolError> {
    let mut command = Command::new(reader);
    for name in DESKTOP_SESSION_VARIABLES {
        command.env_remove(name);
    }
    command
        .envs(desktop.iter().map(|(name, value)| (name, value)))
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        refused(format!(
            "cannot run {} to read the clipboard: {error}; install wl-clipboard",
            reader.display()
        ))
    })?;
    // Read while it runs: a pipe it fills would otherwise stall it until the deadline.
    let stdout = read_bounded(child.stdout.take(), stdout_limit + 1);
    let stderr = read_bounded(child.stderr.take(), STDERR_LIMIT);
    let deadline = Instant::now() + READ_LIMIT;
    let no_answer = || {
        refused(format!(
            "{} gave no answer within {} s: the program holding the clipboard did not hand it over",
            reader.display(),
            READ_LIMIT.as_secs()
        ))
    };
    let status = loop {
        let waited = child.try_wait().map_err(|error| {
            ProtocolError::new(
                ErrorCode::Internal,
                format!("cannot wait for {}: {error}", reader.display()),
            )
        })?;
        if let Some(status) = waited {
            break status;
        }
        if Instant::now() >= deadline {
            // Kill fails only when it exited meanwhile, which `wait` then reaps.
            let _ = child.kill();
            let _ = child.wait();
            return Err(no_answer());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let collect = |output: &Receiver<Vec<u8>>| {
        output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| no_answer())
    };
    Ok(Answer {
        status,
        stdout: collect(&stdout)?,
        stderr: String::from_utf8_lossy(&collect(&stderr)?).into_owned(),
    })
}

/// Reads up to `limit` bytes of `pipe` on a thread of its own, then drops the rest unread.
fn read_bounded(pipe: Option<impl Read + Send + 'static>, limit: usize) -> Receiver<Vec<u8>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut read = Vec::new();
        if let Some(pipe) = pipe {
            let limit = u64::try_from(limit).unwrap_or(u64::MAX);
            if let Err(error) = pipe.take(limit).read_to_end(&mut read) {
                tracing::warn!(%error, "cannot read the clipboard reader's output");
            }
        }
        let _ = sender.send(read);
    });
    receiver
}

fn failed(reader: &Path, answer: &Answer) -> ProtocolError {
    refused(format!(
        "{} could not read the clipboard ({}): {}",
        reader.display(),
        answer.status,
        answer.stderr.trim()
    ))
}

fn refused(message: String) -> ProtocolError {
    ProtocolError::new(ErrorCode::Refused, message)
}
