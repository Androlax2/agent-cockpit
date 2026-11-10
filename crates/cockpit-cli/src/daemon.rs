//! What every verb served by the daemon shares: deadlines, the request, failures, and the
//! argument rules.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode, ProtocolError, Reply};

/// Control verbs answer within this; the daemon answers `busy` at once when it is saturated.
pub(crate) const CONTROL_DEADLINE: Duration = Duration::from_secs(30);
/// Cloning or pushing a large repository can take minutes.
pub(crate) const BULK_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// The arguments as UTF-8 words, or the one that is not.
pub(crate) fn utf8_words(arguments: &[OsString]) -> Result<Vec<&str>, String> {
    arguments
        .iter()
        .map(|word| {
            word.to_str()
                .ok_or_else(|| format!("{}: not UTF-8", word.to_string_lossy()))
        })
        .collect()
}

/// The daemon resolves nothing relative to its own directory: a relative `dir` is made
/// absolute against this command's working directory here.
pub(crate) fn absolute_dir(dir: &str) -> Result<String, String> {
    let path = std::path::absolute(dir).map_err(|error| format!("{dir}: {error}"))?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: not UTF-8", path.display()))
}

/// The daemon's reply to `command`, or exit 1 after saying why there is none.
pub(crate) fn request(command: Command, deadline: Duration) -> Result<Reply, ExitCode> {
    exchange(command, deadline)?.map_err(|error| failed(&error))
}

/// The daemon's answer to `command` on the caller's socket, the client's error included for a
/// caller that handles one itself; exit 1 only when no request could be made.
pub(crate) fn exchange(
    command: Command,
    deadline: Duration,
) -> Result<Result<Reply, ClientError>, ExitCode> {
    let socket = crate::agent::caller_socket()?;
    Ok(
        exchange_all(&socket, vec![command], deadline)?.map(|replies| {
            replies
                .into_iter()
                .next()
                .expect("exchange_all answers each command")
        }),
    )
}

/// The replies of the daemon at `socket` to `commands`, sent in order over one connection,
/// or the first client error; exit 1 only when no request could be made. `deadline` bounds
/// all of them together.
pub(crate) fn exchange_all(
    socket: &std::path::Path,
    commands: Vec<Command>,
    deadline: Duration,
) -> Result<Result<Vec<Reply>, ClientError>, ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| fail(&format!("cannot start the client runtime: {error}")))?;
    Ok(runtime.block_on(async {
        let connection = Connection::open(socket, CONTROL_DEADLINE).await?;
        let started = std::time::Instant::now();
        let mut replies = Vec::with_capacity(commands.len());
        for command in commands {
            let remaining = deadline
                .checked_sub(started.elapsed())
                .ok_or(ClientError::DeadlineExceeded)?;
            replies.push(connection.request(command, remaining).await?);
        }
        Ok(replies)
    }))
}

/// [`exchange_all`], then, on the same connection, the command `follow_up` names from those
/// replies, if any. Its own failure is its answer; it fails none of the others.
pub(crate) fn exchange_then(
    socket: &std::path::Path,
    commands: Vec<Command>,
    follow_up: impl FnOnce(&[Reply]) -> Option<Command>,
    deadline: Duration,
) -> Result<FollowedReplies, ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| fail(&format!("cannot start the client runtime: {error}")))?;
    Ok(runtime.block_on(async {
        let connection = Connection::open(socket, CONTROL_DEADLINE).await?;
        let started = std::time::Instant::now();
        let remaining = || {
            deadline
                .checked_sub(started.elapsed())
                .ok_or(ClientError::DeadlineExceeded)
        };
        let mut replies = Vec::with_capacity(commands.len());
        for command in commands {
            replies.push(connection.request(command, remaining()?).await?);
        }
        let followed = match follow_up(&replies) {
            Some(command) => match remaining() {
                Ok(remaining) => Some(connection.request(command, remaining).await),
                Err(error) => Some(Err(error)),
            },
            None => None,
        };
        Ok((replies, followed))
    }))
}

/// What [`exchange_then`] answers: the replies, and the follow-up's answer when one was sent;
/// or the first error of the replies.
pub(crate) type FollowedReplies =
    Result<(Vec<Reply>, Option<Result<Reply, ClientError>>), ClientError>;


/// sysexits' `EX_TEMPFAIL`: the daemon refused for now and the same request may succeed later.
/// Clear of `ask`'s 124 to 126; every other refusal exits 1, so a caller retries only on this.
pub(crate) const RETRY_LATER_STATUS: u8 = 75;

/// Exit [`RETRY_LATER_STATUS`] after saying why the request failed and for a refusal what
/// would lift it, or 1 when retrying the same request cannot help.
pub(crate) fn failed(error: &ClientError) -> ExitCode {
    match error {
        ClientError::Daemon(error) => report_refusal(error),
        error => fail(&error.to_string()),
    }
}

/// Prints the refusal and its unblock, and returns the status [`refusal_status`] names.
pub(crate) fn report_refusal(error: &ProtocolError) -> ExitCode {
    eprintln!("torlek: {}", refusal_text(error));
    if let Some(unblock) = &error.unblock {
        crate::agent::print_unblock(unblock);
    }
    refusal_status(&error.code)
}

/// `refused [busy]: MESSAGE`, saying when the daemon changed nothing. An `internal` failure or a
/// code this version does not know may have taken effect, so neither claims otherwise.
fn refusal_text(error: &ProtocolError) -> String {
    let code = error.code.as_str();
    match error.code {
        ErrorCode::Internal => format!("failed [{code}]: {}", error.message),
        ErrorCode::Unknown(_) => format!("refused [{code}]: {}", error.message),
        ErrorCode::Busy => format!(
            "refused [{code}]: {} (nothing changed; retry later)",
            error.message
        ),
        _ => format!("refused [{code}]: {} (nothing changed)", error.message),
    }
}

/// An unknown code is final: nothing says retrying would help.
fn refusal_status(code: &ErrorCode) -> ExitCode {
    match code {
        ErrorCode::Busy => ExitCode::from(RETRY_LATER_STATUS),
        _ => ExitCode::FAILURE,
    }
}

pub(crate) fn unexpected(reply: &Reply) -> ExitCode {
    fail(&format!("the daemon answered with an unexpected {reply:?}"))
}

pub(crate) fn fail(problem: &str) -> ExitCode {
    eprintln!("torlek: {problem}");
    ExitCode::FAILURE
}
