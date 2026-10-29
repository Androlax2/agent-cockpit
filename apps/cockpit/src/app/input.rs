//! Terminal key decoding and prompt input.

use super::*;

/// A key or a click, from crossterm.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Char(char),
    /// Bracketed paste characters must never become panel or form shortcuts.
    Paste(String),
    Ctrl(char),
    Enter,
    Tab,
    BackTab,
    Esc,
    Backspace,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F(u8),
    Click {
        x: u16,
        y: u16,
        double: bool,
    },
    /// tmux moved its focus into the column or out of it (focus-events).
    FocusGained,
    FocusLost,
    Resize,
}

/// What a key does to a one-line prompt.
#[derive(Debug, PartialEq)]
pub(crate) enum LineEdit {
    Typing,
    Done,
    Cancelled,
}

/// One key on the prompt's line: Enter sends it, Escape and Ctrl+C cancel it.
pub(crate) fn edit_line(line: &mut String, code: KeyCode, modifiers: KeyModifiers) -> LineEdit {
    match code {
        KeyCode::Enter => LineEdit::Done,
        KeyCode::Esc => LineEdit::Cancelled,
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => LineEdit::Cancelled,
        KeyCode::Backspace => {
            line.pop();
            LineEdit::Typing
        }
        KeyCode::Char(typed) if !modifiers.contains(KeyModifiers::CONTROL) => {
            line.push(typed);
            LineEdit::Typing
        }
        _ => LineEdit::Typing,
    }
}

/// A one-line prompt under the column, read key by key in raw mode: in the terminal's own line
/// mode a Ctrl+C killed the whole column ("Pane is dead (signal 2)").
pub(super) fn input_line(screen: &mut dyn Screen, prompt: &str) -> String {
    screen
        .outside(&mut || {
            let _ = crossterm::terminal::enable_raw_mode();
            let mut line = String::new();
            let outcome = loop {
                print!("\r\x1b[2K  {prompt}{line}");
                let _ = io::stdout().flush();
                let step = match event::read() {
                    Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                        edit_line(&mut line, key.code, key.modifiers)
                    }
                    Ok(Event::Paste(pasted)) => {
                        line.push_str(&pasted.replace(['\n', '\r'], " "));
                        LineEdit::Typing
                    }
                    Ok(_) => LineEdit::Typing,
                    Err(_) => LineEdit::Cancelled,
                };
                if step != LineEdit::Typing {
                    break step;
                }
            };
            let _ = crossterm::terminal::disable_raw_mode();
            (outcome == LineEdit::Done).then(|| line.trim().to_string())
        })
        .unwrap_or_default()
}

/// Reads the next key or click from the terminal, waiting up to `timeout` (forever for None).
pub fn read_key(timeout: Option<Duration>, last_click: &mut Option<(Instant, u16)>) -> Option<Key> {
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        let wait = match deadline {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            None => Duration::from_secs(3600),
        };
        if !event::poll(wait).ok()? {
            if deadline.is_some() {
                return None;
            }
            continue;
        }
        match event::read().ok()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                return Some(match key.code {
                    KeyCode::Char(c) if control => Key::Ctrl(c.to_ascii_lowercase()),
                    KeyCode::Char(c) => Key::Char(c),
                    KeyCode::Enter => Key::Enter,
                    KeyCode::Tab => Key::Tab,
                    KeyCode::BackTab => Key::BackTab,
                    KeyCode::PageUp => Key::PageUp,
                    KeyCode::PageDown => Key::PageDown,
                    KeyCode::Esc => Key::Esc,
                    KeyCode::Backspace => Key::Backspace,
                    KeyCode::Up => Key::Up,
                    KeyCode::Down => Key::Down,
                    KeyCode::Left => Key::Left,
                    KeyCode::Right => Key::Right,
                    KeyCode::F(number) => Key::F(number),
                    _ => continue,
                });
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let double = last_click
                    .is_some_and(|(at, row)| row == mouse.row && at.elapsed() < DOUBLE_CLICK);
                *last_click = Some((Instant::now(), mouse.row));
                return Some(Key::Click {
                    x: mouse.column,
                    y: mouse.row,
                    double,
                });
            }
            Event::FocusGained => return Some(Key::FocusGained),
            Event::Paste(text) => return Some(Key::Paste(text)),
            Event::FocusLost => return Some(Key::FocusLost),
            Event::Resize(..) => return Some(Key::Resize),
            _ => continue,
        }
    }
}
