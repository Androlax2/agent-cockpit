//! On-screen display: a volume/brightness island and the Claude banners.
//!
//! Bottom centre: a 320x36 island for volume, mute and brightness changes. Top centre: a stack of
//! banners, one per Claude Code session. Both are GTK4 + layer shell windows, namespace "osd".
//!
//! Reads one command per line from $XDG_RUNTIME_DIR/osd.fifo, written by osd-show and by the
//! Claude Code hook ~/.claude/hooks/session-state.sh:
//!
//!     volume <0-100>          yellow line, icon by level
//!     mute                    mute icon, empty line
//!     brightness <0-100>      blue line, sun icon
//!     session <json>          a Claude session's state report (the registry)
//!     sessions-clear          forget every session (cockpit reset)
//!     focus-next              show the most urgent Claude session in the manager
//!     session-seen <id>       Theo looked at that session (cockpit view): its banner goes
//!     session-pushed <dir>    cockpit pushed the branch of the sessions started there
//!     run-request <json>      Claude asks to run a command outside the sandbox (ask-to-run; report socket)
//!     run-phase <id> <phase>  claude-run-approve's asking / running / done
//!     guide-request <json>    steps for Theo to do by hand (ask-to-guide; report socket)
//!     todo-update <json>      a claude-todo change, queued for the column (report socket)
//!     notice-clear <id>       drop the banner a notice raised
//!     notice <json>           a sticky banner of its own ({"id", "title", "message"}); a click
//!                             dismisses it (the column's budget alerts)
//!
//! Clicking a session's banner shows that session in the manager and drops the banner. The island
//! hides 1.2 s after the last command. Icons are the bar's SVGs.
//!
//!     osd classify < pane-text          the state a pane's last lines show, for cockpit
//!     osd liveness <session> <pane>     whether the session runs: by id, by pane, or none
//!     osd owns-pane <session> <pane> <server-pid>   exit 0 only for verified live ownership

use desktop::registry::{self, Banner, Banners, NOTICE_ID, Notice, RealSystem, Registry};
use gtk4::prelude::*;
use gtk4::{Align, Application, Box as GtkBox, CssProvider, GestureClick, Image, Label, Orientation, Window, gdk, glib, pango};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

const HIDE_AFTER: Duration = Duration::from_millis(1200);
const WIDTH: i32 = 320;
const HEIGHT: i32 = 36;
const ISLAND_PADDING: i32 = 14;
const ICON_SIZE: i32 = 18;
const ROW_SPACING: i32 = 12;
const TRACK_WIDTH: i32 = WIDTH - 2 * ISLAND_PADDING - ICON_SIZE - ROW_SPACING - 2; // 2 = the hairline border
const BANNER_HIDE_AFTER: Duration = Duration::from_secs(6);
const BANNER_MAX_CHARS: i32 = 64;
const BANNER_SPACING: i32 = 8;
const BANNER_ICON_SIZE: i32 = 22;
const REPORT_READ_DEADLINE: Duration = Duration::from_secs(2);
const REPORT_MAX_BYTES: usize = 1 << 20;
const REPORT_READERS_MAX: usize = 16;
/// A FIFO line longer than this is no command: dropped, so a writer cannot grow the buffer forever.
const FIFO_LINE_MAX: usize = 65_536;

fn icon_dir() -> PathBuf {
    cockpit::paths::home().join(".config/waybar/icons")
}

fn layer_window(app: &Application, edge: Edge, margin: i32) -> Window {
    let window = Window::builder().application(app).resizable(false).decorated(false).build();
    // No client-side decoration: its shadow margins would inflate the layer surface.
    window.add_css_class("osd-window"); // not "osd": GTK styles that class itself
    window.init_layer_shell();
    window.set_namespace(Some("osd"));
    window.set_layer(Layer::Overlay);
    window.set_anchor(edge, true);
    window.set_margin(edge, margin);
    window.set_keyboard_mode(KeyboardMode::None);
    window
}

fn volume_icon(level: i32) -> &'static str {
    match level {
        0 => "volume-mute.svg",
        1..=33 => "volume-low.svg",
        34..=66 => "volume-mid.svg",
        _ => "volume-high.svg",
    }
}

fn banner_row(title: &str, message: &str, sticky: bool) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, ROW_SPACING);
    row.add_css_class("island");
    row.add_css_class("banner");
    if sticky {
        row.add_css_class("attention");
    }
    let icon = Image::from_file(icon_dir().join(if sticky { "bell-dot.svg" } else { "bell.svg" }));
    icon.set_pixel_size(BANNER_ICON_SIZE);
    icon.set_valign(Align::Center);
    let label = |text: &str, class: &str| {
        let label = Label::builder().label(text).xalign(0.0).max_width_chars(BANNER_MAX_CHARS).single_line_mode(true).build();
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.add_css_class(class);
        label
    };
    let text = GtkBox::new(Orientation::Vertical, 0);
    text.set_valign(Align::Center);
    text.append(&label(title, "title"));
    text.append(&label(message, "message"));
    row.append(&icon);
    row.append(&text);
    row
}

/// The banner stack's window and rows, shared with the click handlers.
struct Stack {
    window: Window,
    column: GtkBox,
    rows: HashMap<String, GtkBox>,
    timers: HashMap<String, glib::SourceId>,
}

#[derive(Clone)]
struct GtkBanners(Rc<RefCell<Stack>>);

impl GtkBanners {
    fn new(app: &Application) -> Self {
        let window = layer_window(app, Edge::Top, 15);
        let column = GtkBox::new(Orientation::Vertical, BANNER_SPACING);
        window.set_child(Some(&column));
        GtkBanners(Rc::new(RefCell::new(Stack { window, column, rows: HashMap::new(), timers: HashMap::new() })))
    }

    fn add(&self, key: &str, row: GtkBox, on_click: impl Fn() + 'static) {
        self.remove(key);
        let click = GestureClick::new();
        click.connect_released(move |_, _, _, _| on_click());
        row.add_controller(click);
        let mut stack = self.0.borrow_mut();
        stack.column.append(&row);
        stack.rows.insert(key.to_string(), row);
        stack.window.present();
    }
}

impl Banners for GtkBanners {
    fn raise_banner(&mut self, banner: &Banner) {
        let session = banner.session.clone();
        let this = self.clone();
        self.add(&session.clone(), banner_row(&banner.title, &banner.message, banner.sticky), move || {
            // show takes about 60 ms: waited for on a thread of its own, the banner goes at once.
            let shown = session.clone();
            std::thread::spawn(move || {
                let _ = std::process::Command::new("cockpit").args(["show", &shown]).status();
            });
            this.remove(&session);
        });
        if !banner.sticky {
            let this = self.clone();
            let key = banner.session.clone();
            let timer = glib::timeout_add_local_once(BANNER_HIDE_AFTER, move || {
                this.0.borrow_mut().timers.remove(&key);
                this.remove(&key);
            });
            self.0.borrow_mut().timers.insert(banner.session.clone(), timer);
        }
    }

    /// A sticky banner that is not a session (budget alerts, run and guide requests); a click
    /// dismisses it. Shown on every workspace, the Claude one included.
    fn raise_notice(&mut self, notice: &Notice) {
        let key = format!("notice:{}", notice.id);
        let this = self.clone();
        let click_key = key.clone();
        self.add(&key, banner_row(&notice.title, &notice.message, true), move || this.remove(&click_key));
    }

    fn drop_banner(&mut self, key: &str) {
        self.remove(key);
    }

    fn showing(&self, key: &str) -> bool {
        self.0.borrow().rows.contains_key(key)
    }
}

impl GtkBanners {
    fn remove(&self, key: &str) {
        let mut stack = self.0.borrow_mut();
        let Some(row) = stack.rows.remove(key) else { return };
        if let Some(timer) = stack.timers.remove(key) {
            timer.remove();
        }
        stack.column.remove(&row);
        if stack.rows.is_empty() {
            stack.window.set_visible(false);
        }
    }
}

/// The volume/brightness island.
struct Island {
    window: Window,
    icon: Image,
    fill: GtkBox,
    hide_timer: Option<glib::SourceId>,
}

impl Island {
    fn new(app: &Application) -> Self {
        let window = layer_window(app, Edge::Bottom, 15);
        window.set_default_size(WIDTH, HEIGHT);
        window.set_size_request(WIDTH, HEIGHT);
        let icon = Image::new();
        icon.set_pixel_size(ICON_SIZE);
        // Two plain boxes instead of a progress bar: the GTK theme's rules would win over this
        // file's, boxes take the CSS as written.
        let track = GtkBox::new(Orientation::Horizontal, 0);
        track.set_hexpand(true);
        track.set_valign(Align::Center);
        track.add_css_class("track");
        track.set_size_request(TRACK_WIDTH, 2);
        let fill = GtkBox::new(Orientation::Horizontal, 0);
        fill.set_halign(Align::Start);
        fill.add_css_class("fill");
        track.append(&fill);
        let row = GtkBox::new(Orientation::Horizontal, ROW_SPACING);
        row.add_css_class("island");
        row.append(&icon);
        row.append(&track);
        window.set_child(Some(&row));
        Island { window, icon, fill, hide_timer: None }
    }
}

fn show_island(island: &Rc<RefCell<Island>>, kind: &str, icon_file: &str, fraction: f64) {
    let mut this = island.borrow_mut();
    for class in ["volume", "brightness", "mute"] {
        this.fill.remove_css_class(class);
    }
    this.fill.add_css_class(kind);
    this.icon.set_from_file(Some(icon_dir().join(icon_file)));
    this.fill.set_size_request((TRACK_WIDTH as f64 * fraction).round() as i32, 2);
    this.window.present();
    if let Some(timer) = this.hide_timer.take() {
        timer.remove();
    }
    let shared = island.clone();
    this.hide_timer = Some(glib::timeout_add_local_once(HIDE_AFTER, move || {
        let mut this = shared.borrow_mut();
        this.hide_timer = None;
        this.window.set_visible(false);
    }));
}

fn handle(line: &str, from_host: bool, registry: &Rc<RefCell<Registry>>, island: &Rc<RefCell<Island>>) {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    // Sandboxes send their reports and requests on the report socket, where the kernel names the
    // sender (security audit 2026-10-19, H3); on osd.fifo nothing of theirs is taken any more. The
    // FIFO stays because the health checks look for the daemon holding it.
    if !from_host && !command.is_empty() {
        eprintln!("osd: {command:?} refused on the sandboxes' FIFO: sandboxes use the report socket");
        return;
    }
    match command {
        "" => {}
        "session" => match registry::parse_session(rest) {
            Ok(report) => registry.borrow_mut().update(report),
            Err(error) => eprintln!("osd: bad session {rest:?}: {error}"),
        },
        "focus-next" => registry.borrow_mut().focus_most_urgent(),
        "sessions-clear" => registry.borrow_mut().clear(),
        "session-seen" => registry.borrow_mut().seen(rest),
        "session-pushed" => registry.borrow_mut().pushed(rest),
        "notice" => match registry::parse_notice(rest) {
            Ok(notice) => registry.borrow_mut().banners.raise_notice(&notice),
            Err(error) => eprintln!("osd: bad notice {rest:?}: {error}"),
        },
        "notice-clear" if NOTICE_ID.is_match(rest) => registry.borrow_mut().banners.drop_banner(&format!("notice:{rest}")),
        "notice-clear" => {}
        "run-phase" => {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() == 2 {
                registry.borrow_mut().run_phase(parts[0], parts[1]);
            }
        }
        "volume" | "mute" | "brightness" => {
            let level = if rest.is_empty() { Some(0) } else { rest.parse::<i32>().ok().map(|level| level.clamp(0, 100)) };
            let Some(level) = level else {
                eprintln!("osd: bad level in {line:?}");
                return;
            };
            match command {
                "volume" => show_island(island, "volume", volume_icon(level), level as f64 / 100.0),
                "mute" => show_island(island, "mute", "volume-mute.svg", 0.0),
                _ => show_island(island, "brightness", "brightness.svg", level as f64 / 100.0),
            }
        }
        _ => eprintln!("osd: unknown command {line:?}"),
    }
}

/// The FIFO, opened read-write so the daemon never sees EOF when a writer closes.
/// One command per line from a FIFO; `from_host` says whether it is the one sandboxes cannot open.
fn listen(path: &std::path::Path, from_host: bool, registry: &Rc<RefCell<Registry>>, island: &Rc<RefCell<Island>>) {
    let fifo = match desktop::fifo::open(path) {
        Ok(fifo) => fifo,
        Err(error) => {
            eprintln!("osd: cannot open {}: {error}", path.display());
            std::process::exit(1);
        }
    };
    let raw = fifo.as_raw_fd();
    // SAFETY: `fifo` is an open descriptor handed over to the File, which owns it from here on.
    let mut reader = unsafe { std::fs::File::from_raw_fd(raw) };
    std::mem::forget(fifo);
    let mut buffer = Vec::new();
    let (registry, island) = (registry.clone(), island.clone());
    glib_unix::unix_fd_add_local(raw, glib::IOCondition::IN, move |_, _| {
        let mut chunk = [0u8; 4096];
        if let Ok(count) = reader.read(&mut chunk) {
            buffer.extend_from_slice(&chunk[..count]);
        }
        if buffer.len() > FIFO_LINE_MAX && !buffer.contains(&b'\n') {
            eprintln!("osd: a line of more than {FIFO_LINE_MAX} bytes on {}, dropped", if from_host { "the host FIFO" } else { "osd.fifo" });
            buffer.clear();
        }
        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            handle(String::from_utf8_lossy(&line).trim(), from_host, &registry, &island);
        }
        glib::ControlFlow::Continue
    });
}

/// The report socket: from systemd (osd-report.socket, LISTEN_FDS) so it outlives the daemon and the
/// sandboxes' bind mount of it stays valid; bound here when started by hand.
fn report_listener() -> std::io::Result<std::os::unix::net::UnixListener> {
    use std::os::fd::FromRawFd;
    let ours = std::env::var("LISTEN_PID").ok().and_then(|pid| pid.parse::<u32>().ok()) == Some(std::process::id());
    let count = std::env::var("LISTEN_FDS").ok().and_then(|count| count.parse::<i32>().ok()).unwrap_or(0);
    if ours && count >= 1 {
        // SAFETY: systemd passes the listening socket as fd 3 (SD_LISTEN_FDS_START).
        return Ok(unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) });
    }
    let path = cockpit::paths::osd_report_socket();
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let _ = std::fs::remove_file(&path);
    std::os::unix::net::UnixListener::bind(&path)
}

/// What one connection to the report socket sent, read off the main thread (fifo::read_message): a
/// sandbox that sends slowly, or never stops, holds only its own reader thread, never the GTK loop
/// (security audit 2026-10-19, M5).
struct Received {
    stream: std::os::unix::net::UnixStream,
    sender: u32,
    text: String,
}

/// One line per message: `session <json>` reports, whose focus target comes from the sender's
/// ancestry, and session, run, guide and todo requests, answered "ok" or "refused: <why>".
fn handle_connection(received: Received, registry: &Rc<RefCell<Registry>>) {
    let Received { mut stream, sender, text } = received;
    for line in text.lines() {
        let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
        match kind {
            "session" => match registry::parse_session(rest) {
                Ok(report) => { let _ = registry.borrow_mut().report_from(report, sender); }
                Err(error) => eprintln!("osd: bad session report: {error}"),
            },
            // Explicit requests wait for acceptance; transport success alone says nothing about it.
            "run-request" | "guide-request" | "todo-update" | "session-request" => {
                let reply = match registry.borrow_mut().request_from(kind, rest, sender) {
                    Ok(()) => "ok".to_string(),
                    Err(reason) => {
                        eprintln!("osd: {kind} refused: {reason}");
                        format!("refused: {reason}")
                    }
                };
                let _ = std::io::Write::write_all(&mut stream, format!("{reply}\n").as_bytes());
            }
            // ask-to-run --check: whether a session can reach the daemon at all.
            "ping" => {
                let _ = std::io::Write::write_all(&mut stream, b"ok\n");
            }
            _ => eprintln!("osd: {kind:?} is not taken on the report socket"),
        }
    }
}

fn listen_for_reports(registry: &Rc<RefCell<Registry>>) {
    match report_listener() {
        Ok(listener) => serve(listener, registry, REPORT_MAX_BYTES, handle_connection),
        Err(error) => eprintln!("osd: no report socket: {error}"),
    }
}

/// One JSON team action per connection, answered with a TeamReply. The sender is the kernel's peer
/// pid; nothing in the request names who sent it.
fn handle_team_connection(received: Received, registry: &Rc<RefCell<Registry>>) {
    let Received { mut stream, sender, text } = received;
    let reply = match serde_json::from_str::<cockpit::teams::AgentAction>(&text) {
        Ok(action) => desktop::teams::request(&mut registry.borrow_mut(), sender, action),
        Err(error) => Err(cockpit::teams::TeamError::new(cockpit::teams::ErrorCode::Invalid, format!("invalid team action: {error}"))),
    };
    let reply = reply.unwrap_or_else(|error| cockpit::teams::TeamReply { request_id: String::new(), result: Err(error) });
    let _ = std::io::Write::write_all(&mut stream, format!("{}\n", serde_json::to_string(&reply).unwrap_or_default()).as_bytes());
}

/// The cockpit's approvals and views: refused for any peer outside the daemon's PID namespace.
fn handle_team_host_connection(received: Received, registry: &Rc<RefCell<Registry>>) {
    use cockpit::teams::{ErrorCode, HostRequest, TeamError, TeamReply};
    let Received { mut stream, sender, text } = received;
    let reply = if !desktop::teams::host_peer_allowed(&registry::Proc, &sender.to_string()) {
        Err(TeamError::new(ErrorCode::Unauthorized, "team host controls are for the cockpit outside the sandbox"))
    } else {
        match serde_json::from_str::<HostRequest>(&text) {
            Ok(request) => desktop::teams::host(&mut registry.borrow_mut(), request),
            Err(error) => Err(TeamError::new(ErrorCode::Invalid, format!("invalid team host request: {error}"))),
        }
    };
    let reply = reply.unwrap_or_else(|error| TeamReply { request_id: String::new(), result: Err(error) });
    let _ = std::io::Write::write_all(&mut stream, serde_json::to_string(&reply).unwrap_or_default().as_bytes());
}

fn listen_for_team_host(registry: &Rc<RefCell<Registry>>) {
    let path = cockpit::paths::team_host_socket();
    let _ = std::fs::remove_file(&path);
    match std::os::unix::net::UnixListener::bind(&path) {
        Ok(listener) => serve(listener, registry, 65_536, handle_team_host_connection),
        Err(error) => eprintln!("osd: no team host socket, team approvals unavailable: {error}"),
    }
}

/// The team socket lives in a 0700 host folder; each project's sandbox reaches it only through a
/// hard link in its own read-only answers mount (cockpit::teams::client::publish_socket).
fn listen_for_teams(registry: &Rc<RefCell<Registry>>) {
    use std::os::unix::fs::DirBuilderExt;
    let path = cockpit::paths::team_agent_socket();
    let folder = path.parent().expect("the team socket has a folder").to_path_buf();
    let bound = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)
        .and_then(|()| match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => std::os::unix::net::UnixListener::bind(&path),
        });
    let listener = match bound {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("osd: no team socket, linked sessions unavailable: {error}");
            return;
        }
    };
    // Sessions started before this daemon still hold links to the old socket: link them again.
    let answers = cockpit::paths::runtime_dir().join("claude-answers");
    for project in std::fs::read_dir(&answers).into_iter().flatten().flatten() {
        if let Err(error) = cockpit::teams::client::publish_socket(&path, &project.path()) {
            eprintln!("osd: team socket not linked into {}: {error}", project.path().display());
        }
    }
    serve(listener, registry, cockpit::teams::client::REQUEST_MAX, handle_team_connection);
}

fn serve(listener: std::os::unix::net::UnixListener, registry: &Rc<RefCell<Registry>>, max_bytes: usize, handler: fn(Received, &Rc<RefCell<Registry>>)) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    let _ = listener.set_nonblocking(true);
    let raw = listener.as_raw_fd();
    let finished: Arc<Mutex<Vec<Received>>> = Arc::default();
    let readers = Arc::new(AtomicUsize::new(0));
    // A reader thread writes a byte here once its message is in `finished`: the main loop wakes on it.
    let (wake_reader, wake_writer) = match std::os::unix::net::UnixStream::pair() {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("osd: no wake-up pair for the report socket: {error}");
            return;
        }
    };
    let _ = wake_reader.set_nonblocking(true);
    {
        let finished = finished.clone();
        glib_unix::unix_fd_add_local(raw, glib::IOCondition::IN, move |_, _| {
            while let Ok((stream, _)) = listener.accept() {
                let Some(sender) = desktop::fifo::peer_pid(&stream) else { continue };
                if readers.load(Ordering::SeqCst) >= REPORT_READERS_MAX {
                    eprintln!("osd: {REPORT_READERS_MAX} report connections already being read, one from pid {sender} dropped");
                    continue;
                }
                let (finished, readers) = (finished.clone(), readers.clone());
                let Ok(wake) = wake_writer.try_clone() else { continue };
                readers.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let reply_end = stream.try_clone();
                    let message = desktop::fifo::read_message(stream, REPORT_READ_DEADLINE, max_bytes);
                    if let Err(reason) = &message {
                        eprintln!("osd: a report connection from pid {sender} dropped: {reason}");
                    }
                    if let (Ok(text), Ok(stream)) = (message, reply_end) {
                        finished.lock().unwrap().push(Received { stream, sender, text });
                        let _ = std::io::Write::write_all(&mut &wake, &[1]);
                    }
                    readers.fetch_sub(1, Ordering::SeqCst);
                });
            }
            glib::ControlFlow::Continue
        });
    }
    let registry = registry.clone();
    let wake_fd = wake_reader.as_raw_fd();
    glib_unix::unix_fd_add_local(wake_fd, glib::IOCondition::IN, move |_, _| {
        let mut drained = [0u8; 256];
        while (&wake_reader).read(&mut drained).is_ok_and(|count| count > 0) {}
        let ready: Vec<Received> = std::mem::take(&mut *finished.lock().unwrap());
        for received in ready {
            handler(received, &registry);
        }
        glib::ControlFlow::Continue
    });
}

fn activate(app: &Application) {
    let provider = CssProvider::new();
    provider.load_from_path(cockpit::paths::home().join(".config/osd/style.css"));
    gtk4::style_context_add_provider_for_display(&gdk::Display::default().expect("a display"), &provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
    let island = Rc::new(RefCell::new(Island::new(app)));
    let banners = GtkBanners::new(app);
    let registry = Rc::new(RefCell::new(Registry::new(Box::new(RealSystem::new()), Box::new(banners))));
    // Held for good: the daemon has no window most of the time.
    std::mem::forget(app.hold());

    listen(&cockpit::paths::osd_fifo(), false, &registry, &island);
    listen(&cockpit::paths::osd_host_fifo(), true, &registry, &island);
    listen_for_reports(&registry);
    listen_for_teams(&registry);
    listen_for_team_host(&registry);
    {
        let registry = registry.clone();
        glib::timeout_add_seconds_local(1, move || {
            registry.borrow_mut().tick_queues();
            glib::ControlFlow::Continue
        });
    }
    glib::timeout_add_seconds_local(registry::PRUNE_INTERVAL_S, move || {
        registry.borrow_mut().prune_dead();
        glib::ControlFlow::Continue
    });
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["classify"] => {
            let mut tail = String::new();
            let _ = std::io::stdin().read_to_string(&mut tail);
            println!("{}", registry::pane_state(&tail).unwrap_or("unknown"));
        }
        ["liveness", session, pane] => {
            let verdict = if registry::live_session_ids().contains(*session) {
                "by id"
            } else if !pane.is_empty() && registry::claude_panes().contains(*pane) {
                "by pane"
            } else {
                "none"
            };
            println!("{verdict}");
        }
        ["owns-pane", session, pane, server] => {
            std::process::exit(if registry::session_owns_pane_in(&registry::Proc, session, pane, server) { 0 } else { 1 });
        }
        [] => {
            let app = Application::builder().application_id("dotfiles.osd").build();
            app.connect_activate(activate);
            app.run_with_args::<&str>(&[]);
        }
        _ => {
            eprintln!("usage: osd [classify < pane-text | liveness <session> <pane> | owns-pane <session> <pane> <server-pid>]");
            std::process::exit(2);
        }
    }
}
