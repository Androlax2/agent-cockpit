//! The manager tmux session (report 05, coverage map row 32): the TUI's column on the left,
//! the viewer pane beside it, where a session is shown by nested attach. The daemon owns it
//! because only the daemon's side runs tmux; the TUI asks for it over the socket.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cockpit_protocol::{Command, ErrorCode, ManagerPaneView, Outcome, ProtocolError, Reply};
use cockpit_sessions::{Session, SessionId, SessionState, Sessions, StopTarget};
use cockpit_tmux::{
    Keystroke, PaneGeometry, PaneId, PaneState, SessionLaunch, SessionName, Split, Viewing,
};

use crate::routes::{Route, WorkClass, refusal};

/// The manager's tmux session name. Session ids are `s` and 12 hex digits, so no agent's
/// session can take it.
const MANAGER: &str = "torlek";
/// The viewer's share of the manager's width.
const VIEWER_PERCENT: u8 = 70;
/// The column's rows above a session open in the phone layout: its name and read-only or
/// typing, the keys or the typed line, and what waits on the owner (#855).
const PHONE_BAR_ROWS: u16 = 3;

/// A handle on the private tmux server for the manager, or why there is none.
pub struct Manager {
    /// The daemon's one handle, shared with the sessions, or why tmux is unavailable.
    tmux: Result<Arc<cockpit_tmux::Server>, String>,
    /// Where the owner's configuration is read: when the column is built
    /// (`tui.attention_bell`) and at each fit (`tui.phone_below_columns`).
    environment: cockpit_config::Environment,
    /// `wl-paste`, which reads the owner's clipboard for CMD+V in the viewer (#848).
    clipboard_reader: PathBuf,
    /// Held while the manager is checked and built, so two `cockpit tui` at once build it
    /// once.
    opening: Mutex<()>,
    /// The session last shown in the viewer: the owner views it, so scheduling gives it no
    /// new work (#43).
    shown: Mutex<Option<SessionId>>,
    /// Held while the keys move, so a give-back between another focus's look at the active
    /// pane and its select cannot falsify `took_from_viewer`.
    focusing: Mutex<()>,
    /// The last `manager-fit` found the window narrower than the owner's phone width: the
    /// viewer attaches read-only (#855).
    phone: AtomicBool,
}

impl Manager {
    pub(crate) fn new(
        tmux: Result<Arc<cockpit_tmux::Server>, String>,
        environment: cockpit_config::Environment,
        clipboard_reader: PathBuf,
    ) -> Manager {
        Manager {
            tmux,
            environment,
            clipboard_reader,
            opening: Mutex::new(()),
            shown: Mutex::new(None),
            focusing: Mutex::new(()),
            phone: AtomicBool::new(false),
        }
    }

    /// The session the viewer shows, until another is shown.
    pub(crate) fn viewed(&self) -> Option<SessionId> {
        self.shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether a copy in a session goes on to the owner's terminal (#848), or why tmux cannot
    /// say.
    pub(crate) fn passes_copies_out(&self) -> Result<bool, String> {
        let tmux = self.tmux.as_deref().map_err(Clone::clone)?;
        tmux.passes_copies_out().map_err(|error| error.to_string())
    }

    /// The program CMD+V in the viewer reads the clipboard with.
    pub(crate) fn clipboard_reader(&self) -> &Path {
        &self.clipboard_reader
    }

    fn tmux(&self) -> Result<&cockpit_tmux::Server, ProtocolError> {
        self.tmux.as_deref().map_err(|reason| {
            ProtocolError::new(
                ErrorCode::Refused,
                format!(
                    "the manager needs tmux, which was unavailable when the daemon started: \
                     {reason}; install tmux, then restart the daemon"
                ),
            )
        })
    }
}

/// `manager-open`, `manager-focus`, `manager-fit`, `session-show`, `session-type` and
/// `session-paste-image`; `routes.rs` sends only those here. Each runs tmux, so they take the bulk lane.
pub(crate) fn route(command: Command, sessions: Arc<Sessions>, manager: Arc<Manager>) -> Route {
    let job = match command {
        Command::ManagerOpen {
            dir,
            column,
            viewer,
            image_paste,
        } => job(tmux_blocking(move || {
            open(&manager, dir, &column, &viewer, &image_paste)
        })),
        Command::ManagerFocus { pane } => job(tmux_blocking(move || focus(&manager, pane))),
        Command::ManagerFit { session_open } => job(tmux_blocking(move || {
            fit(
                &manager,
                phone_below_columns(&manager.environment)?,
                session_open,
            )
        })),
        Command::SessionShow { session } => {
            job(async move { show(&sessions, manager, &session).await })
        }
        Command::SessionType { session, text } => {
            job(async move { type_line(&sessions, &manager, session, &text).await })
        }
        Command::SessionPasteImage => job(async move { paste_image(&sessions, manager).await }),
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
    image_paste: &[String],
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
        ManagerState::Whole(_) => false,
        ManagerState::Absent => true,
        ManagerState::Broken => {
            tracing::warn!("the manager session lost its column or viewer: building it again");
            tmux.close_session(&name).map_err(tmux_error)?;
            true
        }
    };
    if created {
        let column = with_attention_bell(column, &manager.environment)?;
        // A new viewer shows no session yet, beside the column: the desktop layout.
        *manager
            .shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        manager.phone.store(false, Ordering::SeqCst);
        let column_pane = tmux
            .new_session(&SessionLaunch {
                name: name.clone(),
                working_dir: dir,
                command: words(&column),
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
    if !image_paste.is_empty() {
        // Each open: the script names the client that opened the manager last. An optional
        // key: without it CMD+V reaches the session as before, so the manager still opens.
        let panes = whole_manager(tmux)?;
        if let Err(error) = tmux.bind_image_paste(&panes.viewer, &words(image_paste)) {
            tracing::warn!(%error, "CMD+V in the viewer is not bound: it reaches the session as a key");
        }
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

struct ManagerPanes {
    column: PaneId,
    viewer: PaneId,
}

enum ManagerState {
    Absent,
    /// Its column runs and its viewer pane is there.
    Whole(ManagerPanes),
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
        [column, viewer, ..] if column.state == PaneState::Running => {
            ManagerState::Whole(ManagerPanes {
                column: column.id.clone(),
                viewer: viewer.id.clone(),
            })
        }
        _ => ManagerState::Broken,
    })
}

/// Shows `session` in the viewer, waking it first when it sleeps (row 18: wake on open).
/// The viewer is checked first: without one, nobody would see the session it woke. A
/// session without a terminal is only described, manager or not.
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
    if is_without_terminal(shown.state) {
        return describe(sessions, &shown).await;
    }
    {
        let manager = Arc::clone(&manager);
        tmux_blocking(move || whole_manager(manager.tmux()?).map(drop)).await?;
    }
    if shown.state == SessionState::Asleep {
        shown = sessions
            .wake(&id, cockpit_sessions::WakeCause::Open)
            .await
            .map_err(|error| crate::sessions::protocol_error(&error))?;
    }
    let target = SessionName::new(id.as_str()).map_err(tmux_error)?;
    let showing = Arc::clone(&manager);
    let showing_id = id.clone();
    let attaching = target.clone();
    let attached = tmux_blocking(move || {
        let tmux = showing.tmux()?;
        let panes = whole_manager(tmux)?;
        // Under the fits' lock: a fit crossing the width between reading the layout and the
        // attach would leave the viewer in the other one's input mode.
        let _focusing = showing
            .focusing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let is_phone = showing.phone.load(Ordering::SeqCst);
        if is_phone {
            // Stacked before the attach, so the session takes the size the owner sees (#338).
            let column = tmux.pane_geometry(&panes.column).map_err(tmux_error)?;
            arrange(tmux, &panes, column, true, true).map_err(tmux_error)?;
        }
        let attached = tmux.show_in_viewer(&panes.viewer, &attaching, viewing(is_phone));
        if attached.is_ok() {
            *showing
                .shown
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(showing_id);
        }
        Ok(attached)
    })
    .await?;
    if let Err(error) = attached {
        // The session may have ended since it was read: its agent exited, or the owner
        // ended it. Then its terminal is gone, and the session is described instead.
        let current = sessions
            .get(&id)
            .await
            .map_err(|error| crate::sessions::protocol_error(&error))?;
        if is_without_terminal(current.state) {
            return describe(sessions, &current).await;
        }
        let probing = Arc::clone(&manager);
        let terminal_exists = tmux_blocking(move || {
            Ok(probing
                .tmux()?
                .sessions()
                .map_err(tmux_error)?
                .contains(&target))
        })
        .await?;
        if !terminal_exists {
            return Err(ProtocolError::new(
                ErrorCode::Conflict,
                format!(
                    "session {id}'s terminal is gone: its tmux session no longer exists; \
                     end it with `torlek session end {id}`, then resume it with `torlek session resume {id}`"
                ),
            ));
        }
        return Err(tmux_error(error));
    }
    crate::sessions::session_reply(sessions, &shown)
        .await
        .map_err(|crate::approvals::Failed(error)| error)
}

/// Whether a session in `state` has, or is about to have, no terminal to attach to (#630):
/// ended, failed, or being stopped other than for a restart.
fn is_without_terminal(state: SessionState) -> bool {
    matches!(
        state,
        SessionState::Ended
            | SessionState::Failed
            | SessionState::Stopping {
                to: StopTarget::Ended | StopTarget::Asleep,
            }
    )
}

/// The reply for a session without a terminal: its view, with its state and reason. The
/// viewer keeps what it showed.
async fn describe(sessions: &Sessions, session: &Session) -> Result<Reply, ProtocolError> {
    crate::sessions::session_reply(sessions, session)
        .await
        .map_err(|crate::approvals::Failed(error)| error)
}

/// Gives the keys to `pane` of the manager: the column, so the next key the owner presses
/// reaches what the TUI shows there rather than the agent in the viewer (#317), or the viewer
/// the column took them from (#345). Answers whether the viewer had them just before.
fn focus(manager: &Manager, pane: ManagerPaneView) -> Result<Reply, ProtocolError> {
    let tmux = manager.tmux()?;
    let panes = whole_manager(tmux)?;
    let _focusing = manager
        .focusing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let took_from_viewer = tmux.active_pane(&manager_name()).map_err(tmux_error)? == panes.viewer;
    let target = match pane {
        ManagerPaneView::Column => &panes.column,
        ManagerPaneView::Viewer => &panes.viewer,
    };
    tmux.select_pane(target).map_err(tmux_error)?;
    Ok(Reply::ManagerFocused { took_from_viewer })
}

/// The owner's `tui.phone_below_columns`, read for each fit: a broken owner configuration
/// refuses the fit rather than guessing a width the owner did not choose.
fn phone_below_columns(environment: &cockpit_config::Environment) -> Result<u16, ProtocolError> {
    cockpit_config::load(environment, None, &[])
        .map(|config| config.tui_phone_below_columns.value)
        .map_err(|error| {
            ProtocolError::new(
                ErrorCode::Refused,
                format!("the owner configuration cannot be read: {error}"),
            )
        })
}

/// Arranges the manager for its window's width (#855): narrower than `phone_below` it is the
/// phone layout, otherwise the desktop's. The viewer's input follows, off on the phone; when
/// it changes, the viewed session is shown again at the viewer's new size. The phone flag is
/// stored only once all this is done.
fn fit(manager: &Manager, phone_below: u16, session_open: bool) -> Result<Reply, ProtocolError> {
    let tmux = manager.tmux()?;
    let panes = whole_manager(tmux)?;
    let _focusing = manager
        .focusing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let column = tmux.pane_geometry(&panes.column).map_err(tmux_error)?;
    let is_phone = column.window_width < phone_below;
    arrange(tmux, &panes, column, is_phone, session_open).map_err(tmux_error)?;
    let viewer = tmux.pane_geometry(&panes.viewer).map_err(tmux_error)?;
    if viewer.is_input_off != is_phone {
        show_again(manager, tmux, &panes.viewer, viewing(is_phone)).map_err(tmux_error)?;
    }
    manager.phone.store(is_phone, Ordering::SeqCst);
    Ok(Reply::ManagerFitted { phone: is_phone })
}

/// The layout for `is_phone` and `session_open`, from what tmux has, `column` read before:
/// the column alone and zoomed for the phone's list, a bar `PHONE_BAR_ROWS` high above the
/// viewer for an open session, the keys in the column either way; side by side on the
/// desktop. Read from tmux rather than remembered, so a fit after a failed one or a daemon
/// restart still undoes a phone layout, and changed only where it differs, so the resize it
/// causes in the column brings a fit that changes nothing.
fn arrange(
    tmux: &cockpit_tmux::Server,
    panes: &ManagerPanes,
    column: PaneGeometry,
    is_phone: bool,
    session_open: bool,
) -> Result<(), cockpit_tmux::Error> {
    let is_column_full_width = column.pane_width == column.window_width;
    match (is_phone, session_open) {
        (false, _) if column.is_window_zoomed || is_column_full_width => {
            tmux.place_side_by_side(&panes.column, 100 - VIEWER_PERCENT)?;
        }
        (false, _) => {}
        (true, false) if column.is_window_zoomed && column.is_active => {}
        (true, false) => {
            if column.is_window_zoomed {
                tmux.toggle_zoom(&panes.column)?;
            }
            tmux.toggle_zoom(&panes.column)?;
        }
        (true, true)
            if !column.is_window_zoomed
                && is_column_full_width
                && column.pane_height == PHONE_BAR_ROWS => {}
        (true, true) => tmux.stack_panes(&panes.column, PHONE_BAR_ROWS)?,
    }
    if is_phone && !column.is_active {
        tmux.select_pane(&panes.column)?;
    }
    Ok(())
}

/// Shows the viewed session again with `viewing`, or turns the viewer's input to it when no
/// session is viewed or the one viewed has lost its terminal.
fn show_again(
    manager: &Manager,
    tmux: &cockpit_tmux::Server,
    viewer: &PaneId,
    viewing: Viewing,
) -> Result<(), cockpit_tmux::Error> {
    if let Some(shown) = manager.viewed() {
        let shown_again = SessionName::new(shown.as_str())
            .and_then(|target| tmux.show_in_viewer(viewer, &target, viewing));
        match shown_again {
            Ok(()) => return Ok(()),
            // The session may have ended since it was shown: its terminal is gone.
            Err(error) => {
                tracing::warn!(%error, session = %shown, "cannot show the viewed session again");
            }
        }
    }
    tmux.set_viewing(viewer, viewing)
}

fn viewing(is_phone: bool) -> Viewing {
    if is_phone {
        Viewing::ReadOnly
    } else {
        Viewing::ReadWrite
    }
}

/// Types `text` and Enter into `session`'s terminal, only while the viewer shows it: the
/// phone layout's typing reaches what the owner sees and nothing else (#855).
async fn type_line(
    sessions: &Sessions,
    manager: &Manager,
    session: String,
    text: &str,
) -> Result<Reply, ProtocolError> {
    let id = SessionId::parse(&session).map_err(|error| crate::sessions::protocol_error(&error))?;
    if manager.viewed().as_ref() != Some(&id) {
        return Err(ProtocolError::new(
            ErrorCode::Conflict,
            format!("{session} is not the session on screen: open it, then type"),
        ));
    }
    sessions
        .type_line(&id, text)
        .await
        .map_err(|error| crate::sessions::protocol_error(&error))?;
    Ok(Reply::SessionAction {
        session,
        held: Vec::new(),
    })
}

/// CMD+V in the viewer (#848): the owner's clipboard PNG saved in the temporary folder of the
/// session on screen and its path pasted there; without a PNG, the key goes on to the viewer,
/// so the session gets it as it would have. Read only for a running session the viewer shows.
async fn paste_image(sessions: &Sessions, manager: Arc<Manager>) -> Result<Reply, ProtocolError> {
    let Some(id) = manager.viewed() else {
        return Err(ProtocolError::new(
            ErrorCode::Conflict,
            "no session is on screen: open one in the viewer, then press CMD+V",
        ));
    };
    if manager.phone.load(Ordering::SeqCst) {
        return Err(ProtocolError::new(
            ErrorCode::Conflict,
            "the viewer is read-only in the phone layout: widen the window to paste an image",
        ));
    }
    // Checked before the clipboard is read, so no image is read for a session that cannot
    // take it.
    let shown = sessions
        .get(&id)
        .await
        .map_err(|error| crate::sessions::protocol_error(&error))?;
    if shown.state != SessionState::Running {
        return Err(ProtocolError::new(
            ErrorCode::Conflict,
            format!("{id} is not running: open it again, then press CMD+V"),
        ));
    }
    let reader = manager.clipboard_reader.clone();
    let desktop = sessions.desktop_environment();
    let image = tmux_blocking(move || crate::clipboard::read_png(&reader, &desktop)).await?;
    let Some(image) = image else {
        tmux_blocking(move || {
            let tmux = manager.tmux()?;
            let panes = whole_manager(tmux)?;
            tmux.send_keys(&panes.viewer, &[Keystroke::Control('v')])
                .map_err(tmux_error)
        })
        .await?;
        return Ok(Reply::ClipboardImage {
            session: id.to_string(),
            path: None,
        });
    };
    let folder = sessions.session_temp_dir(&id);
    let saved = tmux_blocking(move || crate::clipboard::save(&folder, &image)).await?;
    let path = saved.to_str().ok_or_else(|| {
        ProtocolError::new(
            ErrorCode::Internal,
            format!("{}: not UTF-8, so it cannot be pasted", saved.display()),
        )
    })?;
    sessions
        .paste_text(&id, path)
        .await
        .map_err(|error| crate::sessions::protocol_error(&error))?;
    Ok(Reply::ClipboardImage {
        session: id.to_string(),
        path: Some(path.to_owned()),
    })
}

/// The manager's column and viewer panes, or why it has none: only reopening the manager
/// mends a broken one.
fn whole_manager(tmux: &cockpit_tmux::Server) -> Result<ManagerPanes, ProtocolError> {
    match manager_state(tmux, &manager_name())? {
        ManagerState::Whole(panes) => Ok(panes),
        ManagerState::Absent => Err(ProtocolError::new(
            ErrorCode::NotFound,
            "no manager session is open: run `torlek tui`",
        )),
        ManagerState::Broken => Err(ProtocolError::new(
            ErrorCode::Conflict,
            "the manager session lost its column or its viewer: run `torlek tui` again to rebuild it",
        )),
    }
}

fn manager_name() -> SessionName {
    SessionName::new(MANAGER).expect("the manager's name is a valid tmux session name")
}

/// `column` and the owner's `tui.attention_bell` as the TUI's `--bell on|off` (#682). Read
/// when the column is built, so a change counts from the next time the manager is built: the
/// column keeps the flag it started with. An invalid file refuses the build and names the fix,
/// as `config reload` refuses it.
fn with_attention_bell(
    column: &[String],
    environment: &cockpit_config::Environment,
) -> Result<Vec<String>, ProtocolError> {
    let config = cockpit_config::load(environment, None, &[]).map_err(|error| {
        ProtocolError::new(
            ErrorCode::Refused,
            format!(
                "the owner configuration is invalid, so the manager is not built: {error}; \
                 fix the file, then run `torlek tui` again"
            ),
        )
    })?;
    let bell = if config.tui_attention_bell.value {
        "on"
    } else {
        "off"
    };
    Ok(column
        .iter()
        .cloned()
        .chain(["--bell".to_owned(), bell.to_owned()])
        .collect())
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
