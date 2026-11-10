//! What every verb served by the daemon shares: deadlines, the request, failures, and the
//! argument rules.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, Reply};

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

/// Exit 1 after saying why the request failed, and for a refusal what would lift it.
pub(crate) fn failed(error: &ClientError) -> ExitCode {
    match error {
        ClientError::Daemon(error) => {
            let code = fail(&format!("{}: {}", error.code.as_str(), error.message));
            if let Some(unblock) = &error.unblock {
                crate::agent::print_unblock(unblock);
            }
            code
        }
        error => fail(&error.to_string()),
    }
}

pub(crate) fn unexpected(reply: &Reply) -> ExitCode {
    fail(&format!("the daemon answered with an unexpected {reply:?}"))
}

pub(crate) fn fail(problem: &str) -> ExitCode {
    eprintln!("torlek: {problem}");
    ExitCode::FAILURE
}
