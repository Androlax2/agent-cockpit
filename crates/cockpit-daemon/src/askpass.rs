//! sudo's askpass in an approved run (#1374): the socket the helper asks on, and the helper.
//! `cockpit-approvals` decides whether the asking process is the trusted sudo's and holds the
//! asks; this file carries the prompt in and the owner's password out.
//!
//! The helper is this program run as `torlek-daemon --sudo-askpass <socket> <prompt>`:
//!
//! 1. It writes the prompt and a NUL byte.
//! 2. The daemon checks its executable and output, which only an open process shows, and
//!    replies `seal\n`, or `no\n` and why.
//! 3. The helper closes itself to tracing and to `/proc` (non-dumpable), refuses if a tracer
//!    is attached already, and replies `sealed\n`.
//! 4. The daemon asks the owner and replies `ok\n` and the password, which the helper prints
//!    for sudo, or `no\n` and why, which it prints on its standard error, the run's terminal.
//!
//! Nothing else reads or keeps the password.

use std::ffi::OsString;
use std::io::{BufRead, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use cockpit_approvals::{Runner, SudoAskpass};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

use crate::StartError;

/// The argument that makes `torlek-daemon` the askpass helper sudo runs (#1374).
pub const SUDO_ASKPASS_FLAG: &str = "--sudo-askpass";

/// The longest prompt read: sudo's own is one short line.
const PROMPT_MAX_BYTES: usize = 4096;
/// How long a helper may take to send its prompt once connected.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(5);
/// Helpers served at once; each holds a connection while the owner reads.
const CONNECTIONS: usize = 32;
const ANSWERED: &[u8] = b"ok\n";
const REFUSED: &[u8] = b"no\n";
const SEAL: &[u8] = b"seal\n";
const SEALED: &[u8] = b"sealed\n";

/// How runs' `sudo` asks, with the launchers in `launchers` and the helper this program asking
/// on `socket`. Linux only: the check reads `/proc`.
pub fn sudo_askpass(program: &Path, socket: &Path, launchers: PathBuf) -> Option<SudoAskpass> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let askpass = SudoAskpass::new(
        launchers,
        vec![
            program.as_os_str().to_owned(),
            SUDO_ASKPASS_FLAG.into(),
            socket.as_os_str().to_owned(),
        ],
    );
    if let Err(why) = askpass.trusted_sudo_holds() {
        tracing::warn!(%why, "sudo in approved runs asks through its terminal: Torlek's askpass is off");
        return None;
    }
    Some(askpass)
}

/// Binds the askpass socket, owner-only, in the private runtime directory.
pub(crate) fn bind(path: &Path) -> Result<UnixListener, StartError> {
    crate::server::bind(path)
}

/// Answers helpers until the listener fails for good.
pub(crate) async fn answer_forever(listener: Arc<UnixListener>, runner: Runner, owner_uid: u32) {
    let places = Arc::new(Semaphore::new(CONNECTIONS));
    let mut answering = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = answering.join_next(), if !answering.is_empty() => continue,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::warn!(%error, "accepting an askpass connection failed; retrying");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(place) = Arc::clone(&places).try_acquire_owned() else {
            tracing::warn!("too many askpass connections; closing a new one");
            continue;
        };
        let runner = runner.clone();
        answering.spawn(async move {
            answer(stream, &runner, owner_uid).await;
            drop(place);
        });
    }
}

/// One helper: the owner's password once the owner typed it, or why not. A helper that goes
/// away meanwhile drops the ask, so the owner is asked nothing more for it.
async fn answer(mut stream: UnixStream, runner: &Runner, owner_uid: u32) {
    let helper = match stream.peer_cred() {
        Ok(credentials) if credentials.uid() == owner_uid => credentials.pid(),
        Ok(credentials) => {
            tracing::warn!(
                uid = credentials.uid(),
                "refused an askpass connection from another user"
            );
            return;
        }
        Err(error) => {
            tracing::warn!(%error, "refused an askpass connection whose peer cannot be read");
            return;
        }
    };
    let Some(helper) = helper.and_then(|pid| u32::try_from(pid).ok()) else {
        tracing::warn!("refused an askpass connection: the kernel named no process");
        return;
    };
    let prompt = match tokio::time::timeout(PROMPT_TIMEOUT, read_prompt(&mut stream)).await {
        Ok(Ok(prompt)) => prompt,
        Ok(Err(error)) => {
            tracing::info!(helper, %error, "an askpass helper sent no prompt");
            return;
        }
        Err(_) => {
            tracing::info!(helper, "an askpass helper sent no prompt in time");
            return;
        }
    };
    let checked = match runner.sudo_helper(helper) {
        Ok(checked) => checked,
        Err(why) => {
            tracing::info!(helper, %why, "refused sudo's askpass");
            let _ = stream.write_all(&[REFUSED, why.as_bytes()].concat()).await;
            return;
        }
    };
    if let Err(error) = sealed(&mut stream).await {
        tracing::info!(helper, %error, "an askpass helper did not close itself to tracing");
        return;
    }
    let (read_half, mut write_half) = stream.split();
    let asked = tokio::select! {
        asked = runner.ask_sudo(checked, &prompt) => asked,
        () = closed(read_half) => {
            tracing::info!(helper, "an askpass helper went before the owner answered");
            return;
        }
    };
    let reply = match asked {
        Ok(answered) => {
            tracing::info!(
                request = answered.request.0,
                ask = answered.ask,
                "sudo's askpass got the owner's password"
            );
            [ANSWERED, answered.password.as_bytes()].concat()
        }
        Err(refused) => {
            tracing::info!(helper, %refused, "refused sudo's askpass");
            [REFUSED, refused.as_bytes()].concat()
        }
    };
    if let Err(error) = write_half.write_all(&reply).await {
        tracing::info!(helper, %error, "an askpass helper went before its answer");
    }
}

/// The prompt, up to the NUL that ends it.
async fn read_prompt(stream: &mut UnixStream) -> std::io::Result<String> {
    let mut prompt = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == 0 {
            return Ok(String::from_utf8_lossy(&prompt).into_owned());
        }
        if prompt.len() == PROMPT_MAX_BYTES {
            return Err(std::io::Error::other("the prompt is too long"));
        }
        prompt.push(byte);
    }
}

/// Tells the helper to close itself to tracing, then waits until it says it did. A helper that
/// finds itself traced says so on the run's terminal and goes instead.
async fn sealed(stream: &mut UnixStream) -> std::io::Result<()> {
    stream.write_all(SEAL).await?;
    let mut said = [0u8; SEALED.len()];
    tokio::time::timeout(PROMPT_TIMEOUT, stream.read_exact(&mut said))
        .await
        .map_err(|_| std::io::Error::other("it did not answer in time"))??;
    if said != SEALED {
        return Err(std::io::Error::other("it answered something else"));
    }
    Ok(())
}

/// Resolves once the helper closed its side or sent more than its prompt.
async fn closed(mut read_half: tokio::net::unix::ReadHalf<'_>) {
    let mut byte = [0u8; 1];
    let _ = read_half.read(&mut byte).await;
}

/// The helper's work: sudo's prompt to the daemon on `socket`, its answer on standard output.
/// `arguments` are `<socket> [prompt]`, as sudo appends its prompt.
pub fn run_sudo_askpass(arguments: &[OsString]) -> ExitCode {
    let refused = |why: &str| {
        eprintln!("torlek: Torlek did not ask the owner for sudo: {why}");
        ExitCode::FAILURE
    };
    let (socket, prompt) = match arguments {
        [socket] => (socket, OsString::new()),
        [socket, prompt] => (socket, prompt.clone()),
        _ => return refused("the askpass helper takes a socket and sudo's prompt"),
    };
    let mut stream = match std::os::unix::net::UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error) => return refused(&format!("the Torlek daemon cannot be reached ({error})")),
    };
    let mut asked = prompt.as_bytes().to_vec();
    asked.retain(|byte| *byte != 0);
    asked.push(0);
    if let Err(error) = stream.write_all(&asked) {
        return refused(&format!("the Torlek daemon cannot be reached ({error})"));
    }
    // Buffered, so the reason after a refusal's first line is kept.
    let mut replies = std::io::BufReader::new(&stream);
    let mut first = Vec::new();
    if let Err(error) = (&mut replies)
        .take(PROMPT_MAX_BYTES as u64)
        .read_until(b'\n', &mut first)
    {
        return refused(&format!("the Torlek daemon's answer was lost ({error})"));
    }
    if first != SEAL {
        let mut why = Vec::new();
        let _ = replies.read_to_end(&mut why);
        return match first.strip_prefix(REFUSED) {
            Some(start) => refused(&String::from_utf8_lossy(&[start, &why].concat())),
            None => refused("the Torlek daemon ended before the owner was asked"),
        };
    }
    if let Err(why) = cockpit_processes::seal_against_tracing() {
        return refused(&format!(
            "Torlek's askpass helper is not alone: {why}, so nothing was asked"
        ));
    }
    if let Err(error) = (&stream).write_all(SEALED) {
        return refused(&format!("the Torlek daemon cannot be reached ({error})"));
    }
    let mut reply = Vec::new();
    if let Err(error) = replies.read_to_end(&mut reply) {
        return refused(&format!("the Torlek daemon's answer was lost ({error})"));
    }
    if let Some(password) = reply.strip_prefix(ANSWERED) {
        let mut stdout = std::io::stdout().lock();
        return match stdout
            .write_all(password)
            .and_then(|()| stdout.write_all(b"\n"))
            .and_then(|()| stdout.flush())
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("torlek: sudo stopped reading the owner's answer ({error})");
                ExitCode::FAILURE
            }
        };
    }
    match reply.strip_prefix(REFUSED) {
        Some(why) => refused(&String::from_utf8_lossy(why)),
        None => refused("the Torlek daemon ended before the owner answered"),
    }
}
