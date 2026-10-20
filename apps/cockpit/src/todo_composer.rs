//! A TODO draft owns its text and focus until App saves or discards it.

use crate::app::{Key, BLACK, BLUE, FG, FG_DARK, RED, color, fit, put, style, width_of};
use crate::todo::{Category, Item};
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use crate::{todo_editor, todo::TodoList};
use std::{fs, io};

#[derive(Default)]
struct TextField {
    text: String,
    cursor: usize,
}

impl TextField {
    fn new(text: String) -> Self {
        Self { cursor: text.len(), text }
    }

    fn insert(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn handle(&mut self, key: Key, multiline: bool) {
        match key {
            Key::Paste(text) => {
                for character in text.replace("\r\n", "\n").chars() {
                    let character = if character.is_control() && !(multiline && character == '\n') { ' ' } else { character };
                    self.insert(character);
                }
            }
            Key::Char(character) if !character.is_control() || (multiline && character == '\n') => self.insert(character),
            Key::Enter if multiline => self.insert('\n'),
            Key::Left => self.cursor = self.text[..self.cursor].char_indices().next_back().map_or(0, |(index, _)| index),
            Key::Right => self.cursor += self.text[self.cursor..].chars().next().map_or(0, char::len_utf8),
            Key::Backspace if self.cursor > 0 => {
                let previous = self.text[..self.cursor].char_indices().next_back().unwrap().0;
                self.text.drain(previous..self.cursor);
                self.cursor = previous;
            }
            Key::Up | Key::Down if multiline => {
                let start = self.text[..self.cursor].rfind('\n').map_or(0, |index| index + 1);
                let column = self.text[start..self.cursor].chars().count();
                let target = if key == Key::Up {
                    if start == 0 { return; }
                    let previous_start = self.text[..start - 1].rfind('\n').map_or(0, |index| index + 1);
                    (previous_start, start - 1)
                } else {
                    let Some(end) = self.text[self.cursor..].find('\n').map(|offset| self.cursor + offset) else { return; };
                    let next_end = self.text[end + 1..].find('\n').map_or(self.text.len(), |offset| end + 1 + offset);
                    (end + 1, next_end)
                };
                self.cursor = target.0 + self.text[target.0..target.1].char_indices().nth(column).map_or(target.1 - target.0, |(index, _)| index);
            }
            _ => {}
        }
    }

    fn draw(&self, buffer: &mut Buffer, top: u16, left: u16, width: usize, height: usize, focused: bool) {
        // Soft-wrap by terminal cells, not bytes; the draft itself never changes on resize.
        let mut rows = vec![String::new()];
        let mut cursor = (0, 0);
        for (offset, character) in self.text.char_indices().chain(std::iter::once((self.text.len(), '\0'))) {
            let row = rows.len() - 1;
            if offset == self.cursor { cursor = (row, width_of(&rows[row])); }
            if character == '\0' { break; }
            if character == '\n' {
                rows.push(String::new());
            } else {
                let cells = width_of(&character.to_string());
                if width_of(&rows[row]) + cells > width.saturating_sub(1) {
                    rows.push(String::new());
                    if offset == self.cursor { cursor = (rows.len() - 1, 0); }
                }
                rows.last_mut().unwrap().push(character);
            }
        }
        let scroll = if focused { (cursor.0 + 1).saturating_sub(height) } else { 0 };
        for (offset, row) in rows.iter().skip(scroll).take(height).enumerate() {
            put(buffer, top + offset as u16, left, &fit(row, width), style(FG));
        }
        if focused {
            let character = self.text[self.cursor..].chars().next().filter(|character| *character != '\n').unwrap_or(' ');
            put(buffer, top + (cursor.0 - scroll) as u16, left + cursor.1 as u16, &character.to_string(), style(BLACK).bg(color(BLUE)));
        }
    }
}

pub(crate) enum Outcome { Editing, Cancel, Save }

pub(crate) struct TodoComposer {
    pub id: Option<String>,
    pub owner_session: String,
    pub project: String,
    pub category: Option<Category>,
    title: TextField,
    notes: TextField,
    focus: usize,
    pub error: String,
    is_cancelling: bool,
    draft: todo_editor::Draft,
    _editor_lock: fs::File,
}

impl TodoComposer {
    pub fn new(project: String, cwd: String, owner_session: String, item: Option<&Item>) -> io::Result<Self> {
        let editor_lock = todo_editor::lock()?;
        let draft = todo_editor::Draft::create(&project, &cwd, item)?;
        let mut composer = Self {
            id: item.map(Item::id), owner_session, project,
            category: item.map_or(Some(Category::Task), Item::category),
            title: TextField::new(item.map(Item::text).unwrap_or_default()),
            notes: TextField::new(item.map(Item::notes).unwrap_or_default()),
            focus: 1, error: String::new(), is_cancelling: false,
            draft, _editor_lock: editor_lock,
        };
        composer.checkpoint()?;
        Ok(composer)
    }

    pub fn title(&self) -> &str { &self.title.text }

    pub fn checkpoint(&mut self) -> io::Result<()> {
        if self.is_cancelling { return Ok(()); }
        self.draft.checkpoint(&self.title.text, &self.notes.text, self.category)
    }

    pub fn cancel(&mut self) -> io::Result<()> {
        self.is_cancelling = true;
        self.draft.cancel()
    }

    pub fn save(&mut self, todos: &mut TodoList) -> Result<String, String> {
        if self.is_cancelling { return Err("Cancellation pending · Esc retries cleanup".into()); }
        if crate::util::collapse(&crate::todo::without_controls(self.title())).is_empty() { return Err("Title is required".into()); }
        self.checkpoint().map_err(|error| format!("TODO draft kept for recovery: {error}"))?;
        self.draft.finish(todos).map_err(|error| format!("TODO draft kept for recovery: {error}"))?;
        Ok(self.draft.id.clone())
    }

    pub fn handle(&mut self, key: Key) -> Outcome {
        if self.is_cancelling {
            return if key == Key::Esc { Outcome::Cancel } else { Outcome::Editing };
        }
        match key {
            Key::Esc => return Outcome::Cancel,
            Key::Ctrl('s') => return Outcome::Save,
            Key::Enter if self.focus == 3 => return Outcome::Save,
            Key::Tab => self.focus = (self.focus + 1) % 4,
            Key::BackTab => self.focus = (self.focus + 3) % 4,
            Key::Left | Key::Right | Key::Up | Key::Down if self.focus == 0 => {
                let current = self.category.and_then(|category| Category::ALL.iter().position(|candidate| *candidate == category));
                let backward = matches!(key, Key::Left | Key::Up);
                let next = current.map_or(if backward { 5 } else { 0 }, |index| (index + if backward { 5 } else { 1 }) % 6);
                self.category = Some(Category::ALL[next]);
            }
            _ if self.focus == 1 => self.title.handle(key, false),
            _ if self.focus == 2 => self.notes.handle(key, true),
            _ => {}
        }
        Outcome::Editing
    }

    pub fn draw(&self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        for row in 0..height { put(buffer, row, 0, &" ".repeat(width.saturating_sub(1) as usize), style(FG).bg(color(BLACK))); }
        if width < 32 || height < 14 {
            let hint = if self.is_cancelling { "Cancelled · Esc retries cleanup" } else { "Resize to 32×14 · Esc cancels" };
            put(buffer, height / 2, 1, &fit(hint, width.saturating_sub(3) as usize), style(BLUE));
            return;
        }
        let inner = width.saturating_sub(6) as usize;
        let heading = if self.is_cancelling { "TODO cancelled · cleanup pending" } else if self.id.is_some() { "Edit TODO" } else { "Add TODO" };
        put(buffer, 1, 2, &fit(&format!("{heading} · {}", self.project), inner), style(BLUE).add_modifier(Modifier::BOLD));
        let label = |focus| style(if self.focus == focus { BLUE } else { FG_DARK }).add_modifier(Modifier::BOLD);
        let kind = self.category.map(Category::label).unwrap_or("Unknown (keep)");
        put(buffer, 3, 2, &fit(&format!("{}Kind  ‹ {kind} ›", if self.focus == 0 { "> " } else { "  " }), inner), label(0));
        put(buffer, 5, 2, "Title", label(1));
        self.title.draw(buffer, 6, 3, inner, 1, self.focus == 1 && !self.is_cancelling);
        put(buffer, 8, 2, "Notes (optional)", label(2));
        self.notes.draw(buffer, 9, 3, inner, (height - 13) as usize, self.focus == 2 && !self.is_cancelling);
        let button = if self.is_cancelling { "[ Esc Retry cleanup ]" } else if self.id.is_some() { "[ Save changes ]" } else { "[ Add TODO ]" };
        put(buffer, height - 4, 2, button, label(3));
        put(buffer, height - 3, 2, &fit(&self.error, inner), style(RED));
        let hints = if self.is_cancelling { "Esc Retry cleanup · input disabled" } else { "CMD+S Save · Esc Cancel · Tab fields" };
        put(buffer, height - 2, 2, &fit(hints, inner), style(FG_DARK));
    }
}
