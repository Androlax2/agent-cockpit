//! The TODO editor's reload guard and recoverable drafts.

use crate::{app, paths, todo::{Item, TodoList}, util};
use serde::{Deserialize, Serialize};
use std::{fs, io};
use std::path::PathBuf;
use std::process::Command;

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
pub(crate) struct Draft {
    pub token: String,
    pub id: String,
    pub project: String,
    pub cwd: String,
    pub is_edit: bool,
    pub previous_draft: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

fn directory() -> PathBuf { paths::state_dir().join("todo-drafts") }

impl Draft {
    pub fn create(project: &str, cwd: &str, item: Option<&Item>) -> io::Result<Self> {
        let draft = Self { token: util::token_hex(6), id: item.map(Item::id).unwrap_or_else(|| util::token_hex(4)),
            project: project.into(), cwd: cwd.into(), is_edit: item.is_some(),
            previous_draft: item.and_then(|item| item.0.get("editor_draft")).and_then(|value| value.as_str()).map(str::to_string),
            category: item.map(Item::category).unwrap_or(Some(crate::todo::Category::Task)).map(|category| category.as_str().into()) };
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

    pub fn has_title(&self) -> io::Result<bool> {
        Ok(app::parse_editor_text(&fs::read_to_string(self.path("md"))?).is_some())
    }

    pub fn discard(&self) -> io::Result<()> {
        fs::remove_file(self.path("json"))?;
        fs::remove_file(self.path("md"))
    }

    pub fn edit(&self, heading: &str) -> io::Result<()> {
        let status = Command::new("tmux").args(["display-popup", "-E", "-w", "80%", "-h", "70%", "-T", &format!(" {heading} · save and quit keeps it ")])
            // Recovery can retire a draft before a delayed popup starts. Check ownership under
            // the same lock before opening the editor; it must never recreate a retired buffer.
            .args(["sh", "-c", "exec flock -s \"$1\" sh -c '[ -f \"$1\" ] || exit 1; exec \"${EDITOR:-nvim}\" \"$2\"' sh \"$2\" \"$3\"", "sh"])
            .arg(paths::runtime_dir().join("claude-todo-editor.lock")).arg(self.path("json")).arg(self.path("md")).status()?;
        if !status.success() {
            return Err(io::Error::other(format!("editor exited with {status}")));
        }
        Ok(())
    }

    pub fn finish(&self, todos: &mut TodoList) -> io::Result<bool> {
        let text = fs::read_to_string(self.path("md"))?;
        let saved = if let Some((title, notes)) = app::parse_editor_text(&text) {
            todos.save_editor(self, &title, &notes)?;
            true
        } else { false };
        // Remove metadata first: an untracked buffer cannot be imported a second time.
        self.discard()?;
        Ok(saved)
    }
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
