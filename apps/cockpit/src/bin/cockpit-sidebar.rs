//! cockpit-sidebar: the left column of the Claude session manager (cockpit), and
//! three reports it prints for the manager: usage (the plan's usage, redrawn in the session pane),
//! costs (the API-price estimate) and digest (the morning digest).

use cockpit::app::refresh::{self, Changes, Schedule, Watcher};
use cockpit::app::{self, App, Key, Screen};
use cockpit::{costs, usage, util};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
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
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange,
        EnableBracketedPaste,
        cursor::Hide
    )
}

fn leave() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    )
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
    let mut screen = TerminalScreen {
        terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
        last_click: None,
    };
    let mut app = App::new();
    let _ = Command::new("cockpit")
        .arg("sweep")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    app.history.refresh();
    let mut watcher = match Watcher::new(refresh::inputs()) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            util::log_event(
                "WARN",
                &format!("sidebar input watch: {error}; using 250 ms polling"),
            );
            None
        }
    };
    let started = Instant::now();
    let mut repaired_at = Instant::now();
    let mut schedule = Schedule::default();
    loop {
        let mut changes = match watcher.as_mut().map(Watcher::changes).transpose() {
            Ok(changes) => changes.unwrap_or_default(),
            Err(error) => {
                util::log_event(
                    "WARN",
                    &format!("sidebar input watch: {error}; using 250 ms polling"),
                );
                watcher = None;
                Changes {
                    refresh: true,
                    immediate: true,
                }
            }
        };
        if repaired_at.elapsed() >= refresh::SAFETY_INTERVAL {
            let replacement = Watcher::new(refresh::inputs());
            match replacement {
                Ok(replacement) => {
                    if watcher.is_none() {
                        util::log_event("INFO", "sidebar input watch restored");
                    }
                    watcher = Some(replacement);
                }
                Err(error) => {
                    if watcher.is_some() {
                        util::log_event(
                            "WARN",
                            &format!("sidebar input watch: {error}; using 250 ms polling"),
                        );
                    }
                    watcher = None;
                }
            }
            repaired_at = Instant::now();
        }
        // Watching normally replaces this small file read. During degraded coverage preserve
        // the 40 ms session-switch check, independently of the full-refresh polling cooldown.
        if watcher.is_none() && app.viewed_changed() {
            changes.immediate = true;
        }
        let full = schedule.due(started.elapsed(), changes, watcher.is_some());
        let maintenance = schedule.maintenance_due(started.elapsed());
        let background = refresh::take_background_changed();
        if full {
            app.tick();
            screen.draw(&mut app);
            schedule.finished(started.elapsed());
        } else if maintenance {
            app.maintain();
            screen.draw(&mut app);
        } else if background {
            app.collect_team();
            app.redraw();
            screen.draw(&mut app);
        }
        if let Some(key) = screen.next_key(Some(app::VIEW_CHECK_INTERVAL)) {
            app.handle(key, &mut screen);
            app.redraw();
            screen.draw(&mut app);
        }
        if app.collect_manager() {
            app.tick();
            screen.draw(&mut app);
            schedule.finished(started.elapsed());
        }
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => run().map_err(|error| error.to_string()),
        ["usage"] => usage::report_forever(),
        ["costs"] => costs::print_costs().map_err(|error| error.to_string()),
        ["digest"] => costs::print_digest().map_err(|error| error.to_string()),
        ["sessions"] => costs::print_sessions().map_err(|error| error.to_string()),
        _ => Err("usage: cockpit-sidebar [usage|costs|digest|sessions]".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
