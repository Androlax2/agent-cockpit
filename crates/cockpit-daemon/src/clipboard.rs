//! The owner's desktop clipboard, read for CMD+V in the manager's viewer (#848): only its PNG,
//! only when the owner pressed the key, with wl-clipboard's `wl-paste` on the display the owner
//! imported. Agents never read it. Why: `docs/decisions/2026-11-05-848-image-paste-key.md`.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cockpit_protocol::{DESKTOP_SESSION_VARIABLES, ErrorCode, ProtocolError};

const PNG_TYPE: &str = "image/png";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// A larger image is refused rather than cut: a 4K screenshot is about 10 MiB.
const IMAGE_LIMIT_MIB: usize = 32;
/// What the viewer shows for CMD+V when no display was imported. `torlek doctor` words its
/// advice around the same verb.
const NO_DISPLAY_IMPORTED: &str = "no Wayland display imported: run `torlek desktop-session import` in a terminal of your desktop (`torlek daemon install` runs it at each login)";
/// The clipboard's owner, another program, answers each read; one that never does fails the
/// key instead of holding a daemon thread.
const READ_LIMIT: Duration = Duration::from_secs(5);
/// What `wl-paste` says, failing, when the clipboard is empty.
const NOTHING_COPIED: &str = "Nothing is copied";
/// What of `wl-paste`'s complaints is kept for the refusal.
const STDERR_LIMIT: usize = 4096;
/// Names tried for one millisecond's image before the save gives up.
const SAVE_ATTEMPTS: u32 = 100;
/// What of the clipboard's list of types is read: a few lines in practice.
const TYPES_LIMIT: usize = 64 * 1024;

/// The clipboard's PNG; none when it holds no image, or when the daemon has no `reader`, so
/// CMD+V goes on to the session and `torlek doctor` says why; or why it could not be read.
/// With no Wayland display imported it fails (#1236): a silent C-v is what the owner got on
/// every default install, where the daemon starts before the compositor and nothing imported
/// the display. `desktop` is the owner's last import; the desktop variables it lacks are
/// unset for `reader`.
pub(crate) fn read_png(
    reader: &Path,
    desktop: &[(OsString, OsString)],
) -> Result<Option<Vec<u8>>, ProtocolError> {
    if wayland_display(desktop).is_none() {
        return Err(refused(NO_DISPLAY_IMPORTED.to_owned()));
    }
    if find_program(reader).is_none() {
        tracing::info!(reader = %reader.display(), "CMD+V goes on to the session: no clipboard reader");
        return Ok(None);
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

/// The imported `WAYLAND_DISPLAY`, when there is one.
pub(crate) fn wayland_display(desktop: &[(OsString, OsString)]) -> Option<&OsString> {
    desktop
        .iter()
        .find(|(name, value)| name == "WAYLAND_DISPLAY" && !value.is_empty())
        .map(|(_, value)| value)
}

/// `program` itself when it names a path, else the first executable file of that name on the
/// daemon's `PATH`.
pub(crate) fn find_program(program: &Path) -> Option<PathBuf> {
    let is_executable = |candidate: &Path| {
        candidate
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    if program.components().count() > 1 {
        return is_executable(program).then(|| program.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

/// Saves `image` as a new `clipboard-<milliseconds>[-<n>].png` in `folder`, readable and
/// writable by the owner only, and answers its path. Two pastes in one millisecond (a held
/// key) get two files.
pub(crate) fn save(folder: &Path, image: &[u8]) -> Result<PathBuf, ProtocolError> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let cannot = |path: &Path, error: std::io::Error| {
        ProtocolError::new(
            ErrorCode::Internal,
            format!(
                "cannot save the clipboard's image as {}: {error}",
                path.display()
            ),
        )
    };
    for attempt in 0..SAVE_ATTEMPTS {
        let name = match attempt {
            0 => format!("clipboard-{milliseconds}.png"),
            attempt => format!("clipboard-{milliseconds}-{attempt}.png"),
        };
        let path = folder.join(name);
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path);
        let mut file = match opened {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            opened => opened.map_err(|error| cannot(&path, error))?,
        };
        file.write_all(image)
            .map_err(|error| cannot(&path, error))?;
        return Ok(path);
    }
    Err(ProtocolError::new(
        ErrorCode::Internal,
        format!(
            "cannot save the clipboard's image in {}: {SAVE_ATTEMPTS} files of this millisecond exist",
            folder.display()
        ),
    ))
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
