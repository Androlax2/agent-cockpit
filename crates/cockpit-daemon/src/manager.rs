//! The manager tmux session (report 05, coverage map row 32): the TUI's column on the left,
//! the viewer pane beside it, where a session is shown by nested attach. The daemon owns it
//! because only the daemon's side runs tmux; the TUI asks for it over the socket.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cockpit_protocol::{Command, ErrorCode, Outcome, ProtocolError, Reply};
use cockpit_sessions::{SessionId, SessionState, Sessions};
use cockpit_tmux::{PaneId, PaneState, SessionLaunch, SessionName, Split};

use crate::routes::{Route, WorkClass, refusal};

/// The manager's tmux session name. Session ids are `s` and 12 hex digits, so no agent's
/// session can take it.
const MANAGER: &str = "cockpit";
/// The viewer's share of the manager's width.
const VIEWER_PERCENT: u8 = 70;

/// A handle on the private tmux server for the manager, or why there is none.
pub struct Manager {
    tmux: Result<cockpit_tmux::Server, String>,
    /// Held while the manager is checked and built, so two `cockpit tui` at once build it
    /// once.
    opening: Mutex<()>,
}

impl Manager {
    pub(crate) fn open(spec: cockpit_tmux::ServerSpec) -> Manager {
        Manager {
            tmux: cockpit_tmux::Server::open(spec).map_err(|error| error.to_string()),
            opening: Mutex::new(()),
        }
    }

    fn tmux(&self) -> Result<&cockpit_tmux::Server, ProtocolError> {
        self.tmux.as_ref().map_err(|reason| {
            ProtocolError::new(ErrorCode::Refused, format!("tmux is unavailable: {reason}"))
        })
    }
}

/// `manager-open` and `session-show`; `routes.rs` sends only those here. Both run tmux, so
/// they take the bulk lane.
pub(crate) fn route(command: Command, sessions: Arc<Sessions>, manager: Arc<Manager>) -> Route {
    let job = match command {
        Command::ManagerOpen {
            dir,
            column,
            viewer,
        } => job(tmux_blocking(move || open(&manager, dir, &column, &viewer))),
        Command::SessionShow { session } => {
            job(async move { show(&sessions, manager, &session).await })
        }
        other => {
            tracing::error!(
                ?other,
                "a non-manager command was routed to the manager handler"
            );
            return Route::Answer(refusal(
                ErrorCode::Internal,
                "the daemon routed this command to the wrong handler",
            ));
        }
    };
    Route::Run {
        class: WorkClass::Bulk,
        job,
    }
}

fn job(work: impl Future<Output = Result<Reply, ProtocolError>> + Send + 'static) -> crate::Job {
    Box::pin(async move {
        match work.await {
            Ok(reply) => Outcome::Ok(reply),
            Err(error) => Outcome::Error(error),
        }
    })
}

async fn tmux_blocking<Value: Send + 'static>(
    work: impl FnOnce() -> Result<Value, ProtocolError> + Send + 'static,
) -> Result<Value, ProtocolError> {
    tokio::task::spawn_blocking(work).await.map_err(|panic| {
        tracing::error!(%panic, "a manager command panicked");
        ProtocolError::new(ErrorCode::Internal, "the manager command failed")
    })?
}

/// Opens the manager session unless a whole one is open already; either way answers how to
/// attach. A manager missing its viewer or whose column ended is closed and built again.
fn open(
    manager: &Manager,
    dir: String,
    column: &[String],
    viewer: &[String],
) -> Result<Reply, ProtocolError> {
    let tmux = manager.tmux()?;
    let name = manager_name();
    if column.is_empty() || viewer.is_empty() {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "the column and the viewer each need a command",
        ));
    }
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            format!("{} is not an absolute directory", dir.display()),
        ));
    }
    let _opening = manager
        .opening
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let created = match manager_state(tmux, &name)? {
        ManagerState::Whole { .. } => false,
        ManagerState::Absent => true,
        ManagerState::Broken => {
            tracing::warn!("the manager session lost its column or viewer: building it again");
            tmux.close_session(&name).map_err(tmux_error)?;
            true
        }
    };
    if created {
        let column_pane = tmux
            .new_session(&SessionLaunch {
                name: name.clone(),
                working_dir: dir,
                command: words(column),
                environment: Vec::new(),
                columns: 200,
                rows: 50,
            })
            .map_err(tmux_error)?;
        // Should this fail, the manager lacks its viewer: the next open rebuilds it.
        tmux.split_pane(
            &column_pane,
            Split::SideBySide {
                percent: VIEWER_PERCENT,
            },
            &words(viewer),
        )
        .map_err(tmux_error)?;
        tracing::info!("opened the manager session");
    }
    Ok(Reply::Manager {
        attach: tmux
            .attach_command(&name)
            .into_iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect(),
        created,
    })
}

enum ManagerState {
    Absent,
    /// Its column runs and its viewer pane is there.
    Whole {
        viewer: PaneId,
    },
    Broken,
}

fn manager_state(
    tmux: &cockpit_tmux::Server,
    name: &SessionName,
) -> Result<ManagerState, ProtocolError> {
    if !tmux.sessions().map_err(tmux_error)?.contains(name) {
        return Ok(ManagerState::Absent);
    }
    let panes = tmux.panes(name).map_err(tmux_error)?;
    Ok(match panes.as_slice() {
        [column, viewer, ..] if column.state == PaneState::Running => ManagerState::Whole {
            viewer: viewer.id.clone(),
        },
        _ => ManagerState::Broken,
    })
}

/// Shows `session` in the viewer, waking it first when it sleeps (row 18: wake on open).
/// The viewer is checked first: without one, nobody would see the session it woke.
async fn show(
    sessions: &Sessions,
    manager: Arc<Manager>,
    session: &str,
) -> Result<Reply, ProtocolError> {
    let id = SessionId::parse(session).map_err(|error| crate::sessions::protocol_error(&error))?;
    let mut shown = sessions
        .get(&id)
        .await
        .map_err(|error| crate::sessions::protocol_error(&error))?;
    let viewer = {
        let manager = Arc::clone(&manager);
        tmux_blocking(move || viewer_pane(manager.tmux()?)).await?
    };
    if shown.state == SessionState::Asleep {
        shown = sessions
            .wake(&id, cockpit_sessions::WakeCause::Open)
            .await
            .map_err(|error| crate::sessions::protocol_error(&error))?;
    }
    let target = SessionName::new(id.as_str()).map_err(tmux_error)?;
    tmux_blocking(move || {
        manager
            .tmux()?
            .show_in_viewer(&viewer, &target)
            .map_err(tmux_error)
    })
    .await?;
    crate::sessions::session_reply(sessions, &shown)
        .await
        .map_err(|error| crate::sessions::protocol_error(&error))
}

/// The manager's viewer pane, or why it cannot show a session: only reopening the manager
/// mends a broken one.
fn viewer_pane(tmux: &cockpit_tmux::Server) -> Result<PaneId, ProtocolError> {
    match manager_state(tmux, &manager_name())? {
        ManagerState::Whole { viewer } => Ok(viewer),
        ManagerState::Absent => Err(ProtocolError::new(
            ErrorCode::NotFound,
            "no manager session is open: run `cockpit tui`",
        )),
        ManagerState::Broken => Err(ProtocolError::new(
            ErrorCode::Conflict,
            "the manager session lost its column or its viewer: run `cockpit tui` again to rebuild it",
        )),
    }
}

fn manager_name() -> SessionName {
    SessionName::new(MANAGER).expect("the manager's name is a valid tmux session name")
}

fn words(words: &[String]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

fn tmux_error(error: cockpit_tmux::Error) -> ProtocolError {
    match error {
        cockpit_tmux::Error::InvalidArgument { .. } => {
            ProtocolError::new(ErrorCode::InvalidArgument, error.to_string())
        }
        error => {
            tracing::error!(%error, "a manager tmux command failed");
            ProtocolError::new(ErrorCode::Internal, error.to_string())
        }
    }
}
