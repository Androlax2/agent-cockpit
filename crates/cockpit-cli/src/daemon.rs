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
    let socket = crate::agent::socket_dir()?.join(cockpit_config::DAEMON_SOCKET);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| fail(&format!("cannot start the client runtime: {error}")))?;
    let answer = runtime.block_on(async {
        let connection = Connection::open(&socket, CONTROL_DEADLINE).await?;
        connection.request(command, deadline).await
    });
    answer.map_err(|error| match error {
        ClientError::Daemon(error) => fail(&format!("{}: {}", error.code.as_str(), error.message)),
        error => fail(&error.to_string()),
    })
}

pub(crate) fn unexpected(reply: &Reply) -> ExitCode {
    fail(&format!("the daemon answered with an unexpected {reply:?}"))
}

pub(crate) fn fail(problem: &str) -> ExitCode {
    eprintln!("cockpit: {problem}");
    ExitCode::FAILURE
}
