//! The TODO editor's reload guard and recoverable drafts.

use crate::{app, paths, todo::{Item, TodoList}, util};
use serde::{Deserialize, Serialize};
use std::{fs, io};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn lock_file() -> io::Result<fs::File> {
    fs::OpenOptions::new().create(true).append(true)
        .open(paths::runtime_dir().join("claude-todo-editor.lock"))
}

pub(crate) fn lock() -> io::Result<fs::File> {
    let file = lock_file()?;
    file.lock_shared()?;
    Ok(file)
}

#[derive(Serialize, Deserialize)]
pub(crate) struct NativeText {
    pub title: String,
    pub notes: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Draft {
    pub token: String,
    pub id: String,
    pub project: String,
    pub cwd: String,
    pub is_edit: bool,
    pub previous_draft: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub native: Option<NativeText>,
    #[serde(default)]
    pub cancelled: bool,
    /// Pasted images by marker number; the saved item keeps those its notes still mention.
    #[serde(default)]
    pub images: BTreeMap<u32, String>,
}

fn directory() -> PathBuf { paths::state_dir().join("todo-drafts") }

impl Draft {
    pub fn create(project: &str, cwd: &str, item: Option<&Item>) -> io::Result<Self> {
        let draft = Self { token: util::token_hex(6), id: item.map(Item::id).unwrap_or_else(|| util::token_hex(4)),
            project: project.into(), cwd: cwd.into(), is_edit: item.is_some(),
            previous_draft: item.and_then(|item| item.0.get("editor_draft")).and_then(|value| value.as_str()).map(str::to_string),
            category: item.map(Item::category).unwrap_or(Some(crate::todo::Category::Task)).map(|category| category.as_str().into()), native: None, cancelled: false,
            images: item.map(Item::images).unwrap_or_default() };
        fs::create_dir_all(directory())?;
        let (title, notes, why) = item.map(|item| (item.text(), item.notes(), item.why())).unwrap_or_default();
        fs::write(draft.path("md"), app::editor_text(&title, &notes, &why))?;
        draft.persist()?;
        Ok(draft)
    }

    fn path(&self, extension: &str) -> PathBuf { directory().join(format!("{}.{extension}", self.token)) }

    pub fn persist(&self) -> io::Result<()> {
        util::write_json(&self.path("json"), &serde_json::to_value(self)?)
    }

    pub fn checkpoint(&mut self, title: &str, notes: &str, category: Option<crate::todo::Category>) -> io::Result<()> {
        if self.cancelled { return Err(io::Error::other("cancelled draft only permits cleanup")); }
        let category = category.map(|category| category.as_str().to_string());
        if self.category == category && self.native.as_ref().is_some_and(|text| text.title == title && text.notes == notes) {
            return Ok(());
        }
        let previous_text = self.native.replace(NativeText { title: title.into(), notes: notes.into() });
        let previous_category = std::mem::replace(&mut self.category, category);
        if let Err(error) = self.persist() {
            self.native = previous_text;
            self.category = previous_category;
            return Err(error);
        }
        Ok(())
    }

    /// Records a stored image under the next free number; that number goes in the notes' marker.
    pub fn add_image(&mut self, file: String) -> io::Result<u32> {
        if self.cancelled { return Err(io::Error::other("cancelled draft only permits cleanup")); }
        let number = self.images.keys().next_back().map_or(1, |last| last + 1);
        self.images.insert(number, file);
        if let Err(error) = self.persist() {
            self.images.remove(&number);
            return Err(error);
        }
        Ok(number)
    }

    pub fn cancel(&mut self) -> io::Result<()> {
        self.cancelled = true;
        self.persist()?;
        // A durable tombstone prevents recovery even if cleanup is interrupted.
        self.discard()
    }

    pub fn discard(&self) -> io::Result<()> {
        for extension in ["json", "md"] {
            match fs::remove_file(self.path(extension)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn finish(&self, todos: &mut TodoList) -> io::Result<bool> {
        let contents = if self.cancelled { None }
            else if let Some(text) = &self.native {
                (!util::collapse(&crate::todo::without_controls(&text.title)).is_empty()).then(|| (text.title.clone(), text.notes.clone()))
            } else { app::parse_editor_text(&fs::read_to_string(self.path("md"))?) };
        let saved = if let Some((title, notes)) = contents {
            todos.save_editor(self, &title, &notes)?;
            true
        } else { false };
        // Remove metadata first: an untracked buffer cannot be imported a second time.
        if let Err(error) = self.discard() {
            if saved && self.native.is_some() {
                // The token is consumed: keeping the form editable would silently ignore new text.
                util::log_event("WARN", &format!("TODO {} saved; draft cleanup failed: {error}", self.id));
            } else { return Err(error); }
        }
        Ok(saved)
    }
}

/// The images open drafts hold: kept while their form may still save them.
pub(crate) fn draft_images() -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory()) else { return Vec::new() };
    entries.flatten().map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .filter_map(|path| serde_json::from_str::<Draft>(&fs::read_to_string(path).ok()?).ok())
        .flat_map(|draft| draft.images.into_values())
        .collect()
}

pub(crate) fn recover(todos: &mut TodoList) -> io::Result<Option<usize>> {
    if !directory().exists() { return Ok(Some(0)) }
    let lock = lock_file()?;
    match lock.try_lock() {
        Ok(()) => (),
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }
    let mut recovered = 0;
    let mut failure = None;
    for entry in fs::read_dir(directory())? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") { continue }
        let result = (|| -> io::Result<bool> {
            let draft: Draft = serde_json::from_str(&fs::read_to_string(&path)?)?;
            if draft.token.len() != 12 || !draft.token.bytes().all(|byte| byte.is_ascii_hexdigit())
                || path.file_stem().and_then(|stem| stem.to_str()) != Some(&draft.token) {
                return Err(io::Error::other("invalid draft filename"));
            }
            draft.finish(todos)
        })();
        match result {
            Ok(true) => recovered += 1,
            Ok(false) => (),
            Err(error) => {
                util::log_event("WARN", &format!("TODO draft kept at {}: {error}", path.display()));
                failure = Some(error);
            }
        }
    }
    if let Some(error) = failure { return Err(error) }
    Ok(Some(recovered))
}
