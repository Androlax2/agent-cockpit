//! `cockpit-tui`: the two panes of the manager tmux session that `cockpit tui` opens.
//!
//! - `--column --socket PATH`: the TUI itself, talking to the daemon at `PATH`.
//! - `--viewer`: what the viewer pane shows until a session is opened in it.
//!
//! Input never waits on the daemon (report 05): keys are read on their own thread, every
//! request runs in its own task, and answers come back to the loop as they arrive.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, Reply};
use cockpit_tui::{App, FeedEvent, Key, draw, follow_feed, record_usage};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use tokio::sync::mpsc;

/// How often the TUI asks the daemon for what is not on the change feed: projects, the
/// agents' reports and the approval queue.
const REFRESH: Duration = Duration::from_secs(1);
/// Redraws this often regardless, so the pane's grace hint goes away on time.
const REDRAW: Duration = Duration::from_millis(200);
const CONNECT_DEADLINE: Duration = Duration::from_secs(2);
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

const USAGE: &str = "usage: cockpit-tui --column --socket PATH | --viewer
Start it with `cockpit tui`, which opens the manager session.";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["--column", "--socket", socket] => match column(PathBuf::from(socket)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(problem) => {
                eprintln!("cockpit-tui: {problem}");
                ExitCode::FAILURE
            }
        },
        ["--viewer"] => viewer(),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn viewer() -> ExitCode {
    println!("No session is open here. Select one on the left and press Enter.");
    loop {
        std::thread::park();
    }
}

enum Input {
    Key(Key),
    Redraw,
    Answer(Box<(Sent, Result<Reply, String>)>),
}

/// A command on its way to the daemon, and whether it is part of a refresh.
struct Sent {
    command: Command,
    is_refresh: bool,
}

fn column(socket: PathBuf) -> Result<(), String> {
    let log_dir = cockpit_diagnostics::log_dir("tui").map_err(|error| error.to_string())?;
    let directives =
        cockpit_diagnostics::Directives::from_env().map_err(|error| error.to_string())?;
    let diagnostics =
        cockpit_diagnostics::start(cockpit_diagnostics::Options::new(log_dir, directives))
            .map_err(|error| error.to_string())?;
    tracing::dispatcher::set_global_default(diagnostics.dispatch().clone())
        .map_err(|error| error.to_string())?;
    tracing::info!(socket = %socket.display(), "the TUI started");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the runtime: {error}"))?;
    let mut terminal = ratatui::init();
    let outcome = runtime.block_on(run(&mut terminal, socket));
    ratatui::restore();
    if let Err(problem) = &outcome {
        tracing::error!(problem, "the TUI stopped");
    }
    outcome
}

async fn run(terminal: &mut ratatui::DefaultTerminal, socket: PathBuf) -> Result<(), String> {
    let (inputs, mut received) = mpsc::channel::<Input>(256);
    let reader = inputs.clone();
    std::thread::spawn(move || read_keys(&reader));
    let (requests, pending) = mpsc::unbounded_channel::<Sent>();
    let (feed_events, mut feed) = mpsc::channel::<FeedEvent>(1);
    tokio::spawn(follow_feed(socket.clone(), feed_events));
    tokio::spawn(serve(socket, pending, inputs));
    let mut app = App::new();
    // A refresh is sent only once the last one is answered, so a slow daemon gets no pile.
    let mut unanswered_refreshes = 0_usize;
    let mut refresh = tokio::time::interval(REFRESH);
    let mut redraw = tokio::time::interval(REDRAW);
    loop {
        terminal
            .draw(|frame| draw(&app, frame, Instant::now()))
            .map_err(|error| format!("cannot draw: {error}"))?;
        tokio::select! {
            _ = refresh.tick(), if unanswered_refreshes == 0 => {
                for command in App::refresh_commands().into_iter().chain(app.view_refresh_commands()) {
                    unanswered_refreshes += 1;
                    send(&requests, command, true)?;
                }
            }
            _ = redraw.tick() => {}
            event = feed.recv() => match event {
                None => return Err("the change feed task stopped".to_owned()),
                Some(event) => follow(&mut app, event),
            },
            input = received.recv() => match input {
                None => return Err("the key reader stopped".to_owned()),
                Some(Input::Redraw) => {}
                Some(Input::Key(key)) => {
                    let step = app.key(key, Instant::now());
                    record_usage(&step);
                    for command in step.send {
                        send(&requests, command, false)?;
                    }
                }
                Some(Input::Answer(answered)) => {
                    let (sent, answer) = *answered;
                    if sent.is_refresh {
                        unanswered_refreshes -= 1;
                    }
                    if let Some(command) = app.answer(&sent.command, answer, Instant::now()) {
                        send(&requests, command, false)?;
                    }
                }
            },
        }
    }
}

fn follow(app: &mut App, event: FeedEvent) {
    match event {
        FeedEvent::Batch { batch, applied } => {
            // The feed task waits on this answer; it is gone only when the TUI stops.
            let _ = applied.send(app.apply_changes(&batch));
        }
        FeedEvent::Lost(problem) => app.feed_lost(problem),
    }
}

fn send(
    requests: &mpsc::UnboundedSender<Sent>,
    command: Command,
    is_refresh: bool,
) -> Result<(), String> {
    requests
        .send(Sent {
            command,
            is_refresh,
        })
        .map_err(|_| "the request task stopped".to_owned())
}

/// Reads key presses only: WSL and Windows terminals also report releases (report 05).
fn read_keys(inputs: &mpsc::Sender<Input>) {
    loop {
        let input = match event::read() {
            Ok(Event::Key(press)) if press.kind == KeyEventKind::Press => {
                match key_of(press.code) {
                    Some(key) => Input::Key(key),
                    None => continue,
                }
            }
            Ok(Event::Resize(..)) => Input::Redraw,
            Ok(_) => continue,
            Err(error) => {
                tracing::error!(%error, "cannot read the terminal");
                return;
            }
        };
        if inputs.blocking_send(input).is_err() {
            return;
        }
    }
}

fn key_of(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Backspace => Key::Backspace,
        _ => return None,
    })
}

/// Sends each command on one shared connection, opening it again after it was lost; each
/// request runs in its own task so a slow one holds up nothing.
async fn serve(
    socket: PathBuf,
    mut pending: mpsc::UnboundedReceiver<Sent>,
    answers: mpsc::Sender<Input>,
) {
    let connection: Arc<Mutex<Option<Arc<Connection>>>> = Arc::default();
    while let Some(sent) = pending.recv().await {
        let open = connection.lock().expect("never poisoned").clone();
        let open = match open {
            Some(open) => open,
            None => match Connection::open(&socket, CONNECT_DEADLINE).await {
                Ok(opened) => {
                    let opened = Arc::new(opened);
                    *connection.lock().expect("never poisoned") = Some(Arc::clone(&opened));
                    opened
                }
                Err(error) => {
                    // Everything already waiting fails with it, rather than each waiting
                    // out its own connect deadline in turn.
                    let problem = describe(&error);
                    let mut failed = vec![sent];
                    while let Ok(waiting) = pending.try_recv() {
                        failed.push(waiting);
                    }
                    for sent in failed {
                        let answer = Input::Answer(Box::new((sent, Err(problem.clone()))));
                        let _ = answers.send(answer).await;
                    }
                    continue;
                }
            },
        };
        let answers = answers.clone();
        let connection = Arc::clone(&connection);
        tokio::spawn(async move {
            let answer = open.request(sent.command.clone(), REQUEST_DEADLINE).await;
            if let Err(ClientError::Closed(reason)) = &answer {
                tracing::warn!(?reason, "lost the daemon connection");
                let mut current = connection.lock().expect("never poisoned");
                // Only this one: another request may have opened a new connection since.
                if current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &open))
                {
                    current.take();
                }
            }
            let _ = answers
                .send(Input::Answer(Box::new((
                    sent,
                    answer.map_err(|error| describe(&error)),
                ))))
                .await;
        });
    }
}

fn describe(error: &ClientError) -> String {
    match error {
        ClientError::Daemon(error) => format!("{}: {}", error.code.as_str(), error.message),
        error => error.to_string(),
    }
}
