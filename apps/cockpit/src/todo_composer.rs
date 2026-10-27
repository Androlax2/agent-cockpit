//! A TODO draft owns its text and focus until App saves or discards it.

use crate::app::{Key, BG_HIGHLIGHT, BLACK, BLUE, FG, FG_DARK, RED, YELLOW, color, fit, put, style, width_of};
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
            Key::Ctrl('v') => self.paste_image(),
            Key::Tab => self.focus = (self.focus + 1) % 3,
            Key::BackTab => self.focus = (self.focus + 2) % 3,
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

    /// The clipboard's image goes into the notes as "[Image N]", at the notes' cursor.
    fn paste_image(&mut self) {
        let number = crate::todo_images::paste_from_clipboard()
            .and_then(|file| self.draft.add_image(file).map_err(|error| format!("Cannot keep the image: {error}")));
        match number {
            Ok(number) => {
                self.focus = 2;
                self.notes.handle(Key::Paste(crate::todo_images::marker(number)), true);
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    /// A card over the column (page 09, frame 1958): the kind as chips, outlined title and notes, key caps.
    pub fn draw(&self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        for row in 0..height { put(buffer, row, 0, &" ".repeat(width.saturating_sub(1) as usize), style(FG)); }
        if width < 32 || height < 14 {
            let hint = if self.is_cancelling { "Cancelled · Esc retries cleanup" } else { "Resize to 32×14 · Esc cancels" };
            put(buffer, height / 2, 1, &fit(hint, width.saturating_sub(3) as usize), style(BLUE));
            return;
        }
        let (left, right) = (1u16, width.saturating_sub(2));
        let inner = (right - left).saturating_sub(3) as usize;
        let border = style(FG_DARK);
        put(buffer, 0, left, &format!("╭{}╮", "─".repeat((right - left - 1) as usize)), border);
        for row in 1..height - 1 { put(buffer, row, left, "│", border); put(buffer, row, right, "│", border); }
        put(buffer, height - 1, left, &format!("╰{}╯", "─".repeat((right - left - 1) as usize)), border);
        let text_left = left + 2;
        let heading = if self.is_cancelling { "Todo cancelled · cleanup pending" } else if self.id.is_some() { "Edit a todo" } else { "Add a todo" };
        put(buffer, 1, text_left, &fit(&format!("{heading} · {}", self.project), inner), style(FG).add_modifier(Modifier::BOLD));
        let label = |focus: usize| style(if self.focus == focus { BLUE } else { FG_DARK }).add_modifier(Modifier::BOLD);
        put(buffer, 3, text_left, "KIND", label(0));
        let mut row = 4u16;
        let mut column = 0usize;
        for kind in Category::ALL {
            let chip = format!(" {} ", kind.label());
            if column > 0 && column + width_of(&chip) > inner { row += 1; column = 0; }
            let chip_style = if Some(kind) == self.category { style(BLACK).bg(color(BLUE)).add_modifier(Modifier::BOLD) } else { style(FG_DARK).bg(color(BG_HIGHLIGHT)) };
            put(buffer, row, text_left + column as u16, &chip, chip_style);
            column += width_of(&chip) + 1;
        }
        if self.category.is_none() { put(buffer, row + 1, text_left, "Unknown kind (kept)", style(FG_DARK)); row += 1; }
        let title_top = row + 2;
        put(buffer, title_top, text_left, "TITLE", label(1));
        let outline = |buffer: &mut Buffer, top: u16, rows: u16, focused: bool| {
            let line = style(if focused { BLUE } else { FG_DARK });
            put(buffer, top, text_left, &format!("┌{}┐", "─".repeat(inner.saturating_sub(2))), line);
            for offset in 1..=rows { put(buffer, top + offset, text_left, "│", line); put(buffer, top + offset, text_left + inner as u16 - 1, "│", line); }
            put(buffer, top + rows + 1, text_left, &format!("└{}┘", "─".repeat(inner.saturating_sub(2))), line);
        };
        outline(buffer, title_top + 1, 1, self.focus == 1);
        self.title.draw(buffer, title_top + 2, text_left + 2, inner.saturating_sub(4), 1, self.focus == 1 && !self.is_cancelling);
        let notes_top = title_top + 4;
        put(buffer, notes_top, text_left, "NOTES (optional)", label(2));
        // The keys wrap between keys onto a second line when the card is narrow.
        let keys = [("⌘S", "save", true), ("tab", "next field", false), ("←→", "kind", false), ("⌘V", "image", false), ("esc", "cancel", false)];
        let mut key_rows: Vec<Vec<(String, &str, bool)>> = vec![Vec::new()];
        let mut column = 0usize;
        for (key, what, primary) in keys {
            let cap = format!(" {key} ");
            let item = width_of(&cap) + 1 + width_of(what) + 2;
            if column > 0 && column + item > inner { key_rows.push(Vec::new()); column = 0; }
            key_rows.last_mut().unwrap().push((cap, what, primary));
            column += item;
        }
        let keys_top = height - 1 - key_rows.len() as u16;
        let notes_rows = keys_top.saturating_sub(notes_top + 4).max(1);
        outline(buffer, notes_top + 1, notes_rows, self.focus == 2);
        self.notes.draw(buffer, notes_top + 2, text_left + 2, inner.saturating_sub(4), notes_rows as usize, self.focus == 2 && !self.is_cancelling);
        put(buffer, keys_top - 1, text_left, &fit(&self.error, inner), style(RED));
        if self.is_cancelling {
            put(buffer, keys_top, text_left, &fit("Esc retries cleanup · input disabled", inner), style(FG_DARK));
            return;
        }
        for (offset, row_keys) in key_rows.iter().enumerate() {
            let mut column = 0usize;
            for (cap, what, primary) in row_keys {
                let cap_style = if *primary { style(BLACK).bg(color(YELLOW)) } else { style(FG).bg(color(BG_HIGHLIGHT)) }.add_modifier(Modifier::BOLD);
                put(buffer, keys_top + offset as u16, text_left + column as u16, cap, cap_style);
                put(buffer, keys_top + offset as u16, text_left + (column + width_of(cap) + 1) as u16, what, style(FG_DARK));
                column += width_of(cap) + 1 + width_of(what) + 2;
            }
        }
    }
}
