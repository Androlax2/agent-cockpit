//! claude-sessions-sidebar: the left column of the Claude session manager (claude-sessions), and
//! three reports it prints for the manager: usage (the plan's usage, redrawn in the session pane),
//! costs (the API-price estimate) and digest (the morning digest).


use cockpit::app::{self, App, Key, Screen};
use cockpit::{costs, usage, util};
use crossterm::event::{DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Stdout};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct TerminalScreen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    last_click: Option<(Instant, u16)>,
}

fn enter() -> io::Result<()> {
    enable_raw_mode()?; // Ctrl-C arrives as a key (same as q) instead of killing the column
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, EnableFocusChange, cursor::Hide)
}

fn leave() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), DisableMouseCapture, DisableFocusChange, LeaveAlternateScreen, cursor::Show)
}

impl Screen for TerminalScreen {
    fn draw(&mut self, app: &mut App) {
        let _ = self.terminal.draw(|frame| app.render(frame.buffer_mut()));
    }

    fn next_key(&mut self, timeout: Option<Duration>) -> Option<Key> {
        app::read_key(timeout, &mut self.last_click)
    }

    fn outside(&mut self, action: &mut dyn FnMut() -> Option<String>) -> Option<String> {
        let _ = leave();
        let result = action();
        let _ = enter();
        let _ = self.terminal.clear();
        result
    }
}

fn run() -> io::Result<()> {
    util::log_event("INFO", &format!("started (pid {})", std::process::id()));
    // A crash puts the terminal back and leaves its reason on one line in the log.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = leave();
        util::log_event("ERROR", &format!("crashed: {info}"));
        default_hook(info);
    }));
    enter()?;
    let mut screen = TerminalScreen { terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?, last_click: None };
    let mut app = App::new();
    let _ = Command::new("claude-sessions").arg("sweep").stdout(Stdio::null()).stderr(Stdio::null()).status();
    app.history.refresh();
    let mut last_tick: Option<Instant> = None;
    loop {
        if last_tick.is_none_or(|at| at.elapsed() >= app::POLL_INTERVAL) || app.viewed_changed() {
            app.tick();
            screen.draw(&mut app);
            last_tick = Some(Instant::now());
        }
        if let Some(key) = screen.next_key(Some(app::VIEW_CHECK_INTERVAL)) {
            app.handle(key, &mut screen);
            // Drawn from what the last tick read; the next tick, within POLL_INTERVAL, reads again.
            app.redraw();
            screen.draw(&mut app);
        }
        if app.collect_manager() {
            app.redraw();
            screen.draw(&mut app);
        }
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => run().map_err(|error| error.to_string()),
        ["usage"] => usage::report_forever(),
        ["costs"] => costs::print_costs().map_err(|error| error.to_string()),
        ["digest"] => costs::print_digest().map_err(|error| error.to_string()),
        _ => Err("usage: claude-sessions-sidebar [usage|costs|digest]".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
