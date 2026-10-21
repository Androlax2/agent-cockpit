//! Drive the native TODO form through crossterm, including real bracketed paste and resize.
mod common;

use common::{Terminal, contains, set_size};
use serde_json::{Value, json};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct ComposerTerminal {
    terminal: Terminal,
    root: PathBuf,
}

impl ComposerTerminal {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("todo-terminal-{}", cockpit::util::token_hex(6)));
        for folder in ["home", "run", "state", "bin"] { fs::create_dir_all(root.join(folder)).unwrap(); }
        for name in ["cockpit", "tmux", "hyprctl", "fzf", "pgrep", "git", "gh"] {
            let path = root.join("bin").join(name);
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(root.join("bin/cockpit"), format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", root.join("cockpit.calls").display())).unwrap();
        fs::write(root.join("run/cockpit.current"), "aaaaaaaa-0000-4000-8000-000000000000").unwrap();
        fs::write(root.join("run/cockpit.json"), json!({"sessions": [{"session": "aaaaaaaa-0000-4000-8000-000000000000", "state": "working", "project": "test-project", "cwd": root, "since": 1, "started": 1, "tmux_pane": "%1"}]}).to_string()).unwrap();
        let terminal = Self::start(&root);
        Self { terminal, root }
    }

    fn start(root: &Path) -> Terminal {
        Terminal::start(Path::new(env!("CARGO_BIN_EXE_cockpit-sidebar")), &[], &[
            ("HOME", &root.join("home")), ("XDG_RUNTIME_DIR", &root.join("run")),
            ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")),
            ("PATH", &root.join("bin")), ("CLAUDE_STATUS_URL", Path::new("")), ("CODEX_STATUS_URL", Path::new("")),
            ("CLAUDE_AUDIT_JOURNAL", Path::new("off")), ("COCKPIT_OPENCODE_DB", &root.join("missing.db")),
        ])
    }

    fn restart(&mut self) {
        let _ = self.terminal.child.kill();
        let _ = self.terminal.child.wait();
        self.terminal = Self::start(&self.root);
    }

    fn stored(&self) -> Option<Value> {
        fs::read_to_string(self.root.join("state/cockpit/todo.json")).ok().map(|text| serde_json::from_str(&text).unwrap())
    }

    /// Switches the viewed session and waits until the column has taken it over: it labels the
    /// new view's state once it shows it, and a key typed before then would act on the old view.
    fn view_session(&mut self, session: &str) {
        fs::write(self.root.join("run/cockpit.current"), session).unwrap();
        let labelled = format!("label-state working {session}");
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.last_label().as_deref() != Some(labelled.as_str()) {
            assert!(Instant::now() < deadline, "the column never showed {session}; last label: {:?}", self.last_label());
            self.terminal.pump(0.05, None);
        }
    }

    fn last_label(&self) -> Option<String> {
        let calls = fs::read_to_string(self.root.join("cockpit.calls")).unwrap_or_default();
        calls.lines().rev().find(|call| call.starts_with("label-state ")).map(str::to_string)
    }

    fn pick_kind(&mut self, choice: &str) {
        fs::write(self.root.join("pick"), format!("{choice}\n")).unwrap();
        self.terminal.screen.clear();
        self.terminal.type_keys(b"f");
        self.terminal.pump(3.0, Some(b"\x1b[6n"));
        // A PTY has no terminal emulator to answer the cursor query when the picker returns.
        self.terminal.type_keys(b"\x1b[1;1R");
        self.terminal.pump(0.3, None);
    }
}

impl Drop for ComposerTerminal {
    fn drop(&mut self) {
        let _ = self.terminal.child.kill();
        let _ = self.terminal.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn native_todo_form_decodes_paste_resize_cancel_and_command_s() {
    let mut fixture = ComposerTerminal::new();
    fixture.terminal.pump(5.0, Some(b"Todo"));
    assert!(contains(&fixture.terminal.screen, b"\x1b[?2004h"), "bracketed paste was not enabled");
    fixture.terminal.type_keys(b"3t");
    fixture.terminal.pump(5.0, Some(b"Add TODO"));
    assert!(contains(&fixture.terminal.screen, b"Add TODO"), "{}", String::from_utf8_lossy(&fixture.terminal.screen));
    fixture.terminal.type_keys("\x1b[200~qtdjk123 café 界\x1b[201~".as_bytes());
    fixture.terminal.type_keys(b"\x1b[Z\x1b[C\x1b[C\t\t");
    fixture.terminal.type_keys(b"\x1b[200~first\nsecond\x1b[201~");
    fixture.terminal.pump(0.3, None);
    set_size(fixture.terminal.master.as_raw_fd(), 10, 28);
    fixture.terminal.pump(1.0, Some(b"Resize"));
    assert!(contains(&fixture.terminal.screen, b"Resize"));
    assert!(fixture.stored().is_none(), "draft persisted before save");
    set_size(fixture.terminal.master.as_raw_fd(), 30, 60);
    fixture.terminal.pump(0.4, None);
    fixture.terminal.type_keys(b"\x13");
    fixture.terminal.pump(2.0, None);
    let stored = fixture.stored().expect("CMD+S did not save");
    assert_eq!(stored["items"].as_array().unwrap().len(), 1);
    assert_eq!(stored["items"][0]["text"], "qtdjk123 café 界");
    assert_eq!(stored["items"][0]["notes"], "first\nsecond");
    assert_eq!(stored["items"][0]["category"], "bug");
    fixture.terminal.type_keys(b"e changed\x1b");
    fixture.terminal.pump(0.5, None);
    assert_eq!(fixture.stored().unwrap(), stored, "Esc changed the saved item");
}

#[test]
fn reviewed_native_form_saves_to_its_origin_after_the_view_changes() {
    for edit in [false, true] {
        let mut fixture = ComposerTerminal::new();
        let other_session = "bbbbbbbb-0000-4000-8000-000000000000";
        fs::write(fixture.root.join("run/cockpit.json"), json!({"sessions": [
            {"session": "aaaaaaaa-0000-4000-8000-000000000000", "state": "working", "project": "test-project", "cwd": fixture.root, "since": 1, "started": 1, "tmux_pane": "%1"},
            {"session": other_session, "state": "working", "project": "other-project", "cwd": fixture.root.join("other"), "since": 2, "started": 2, "tmux_pane": "%2"}
        ]}).to_string()).unwrap();
        fs::create_dir_all(fixture.root.join("state/cockpit")).unwrap();
        fs::write(fixture.root.join("state/cockpit/todo.json"), json!({"items": [
            {"id": "0123abcd", "project": "test-project", "cwd": fixture.root, "text": "original", "category": "feature", "status": "doing"}
        ]}).to_string()).unwrap();
        fs::write(fixture.root.join("bin/fzf"), format!("#!/bin/sh\nIFS= read -r choice < '{}'\nprintf '%s\\n' \"$choice\"\n", fixture.root.join("pick").display())).unwrap();
        // The column reads todo.json once, at startup.
        fixture.restart();
        fixture.view_session("aaaaaaaa-0000-4000-8000-000000000000");
        fixture.view_session(other_session);
        fixture.terminal.type_keys(b"3");
        fixture.terminal.pump(0.3, None);
        fixture.pick_kind("Bug");
        fixture.terminal.type_keys(b"]]]");
        fixture.terminal.pump(0.5, None);
        fixture.view_session("aaaaaaaa-0000-4000-8000-000000000000");
        fixture.pick_kind("Feature");
        fixture.terminal.screen.clear();
        let form_title: &[u8] = if edit { b"Edit TODO" } else { b"Add TODO" };
        fixture.terminal.type_keys(if edit { b"e" } else { b"t" });
        fixture.terminal.pump(5.0, Some(form_title));
        assert!(contains(&fixture.terminal.screen, form_title), "{}", String::from_utf8_lossy(&fixture.terminal.screen));
        fixture.terminal.type_keys(b"\x1b[200~ saved-cross-view\x1b[201~");
        fixture.terminal.pump(0.3, None);
        fs::write(fixture.root.join("run/cockpit.current"), other_session).unwrap();
        fixture.terminal.pump(0.4, None);
        fixture.terminal.screen.clear();
        fixture.terminal.type_keys(b"\x13");
        fixture.terminal.pump(0.6, None);
        let stored = fixture.stored().unwrap();
        let saved = stored["items"].as_array().unwrap().iter().find(|item| item["text"].as_str().unwrap().ends_with("saved-cross-view")).unwrap_or_else(|| panic!("draft not saved: {stored}; screen {}", String::from_utf8_lossy(&fixture.terminal.screen)));
        assert_eq!(saved["project"], "test-project");
        assert!(contains(&fixture.terminal.screen, b"saved: test-project"), "{}", String::from_utf8_lossy(&fixture.terminal.screen));
        assert!(contains(&fixture.terminal.screen, b"saved-cross-view"));
        assert_eq!(fs::read_to_string(fixture.root.join("run/cockpit.current")).unwrap(), other_session);
        fixture.terminal.screen.clear();
        fixture.terminal.type_keys(b"\x1b");
        fixture.terminal.pump(0.6, None);
        set_size(fixture.terminal.master.as_raw_fd(), 30, 80);
        fixture.terminal.pump(0.4, None);
        assert!(contains(&fixture.terminal.screen, b"kind: Bug"), "{}", String::from_utf8_lossy(&fixture.terminal.screen));
        assert!(contains(&fixture.terminal.screen, b"In progress 0"));
    }
}

#[test]
fn integrated_native_terminal_recovers_a_crashed_draft_and_releases_the_reload_lock() {
    let mut fixture = ComposerTerminal::new();
    fixture.terminal.pump(5.0, Some(b"Todo"));
    fixture.terminal.type_keys(b"3t");
    fixture.terminal.pump(5.0, Some(b"Add TODO"));
    fixture.terminal.type_keys("\x1b[200~Recovered café 界\x1b[201~".as_bytes());
    fixture.terminal.type_keys(b"\x1b[Z\x1b[C\x1b[C\t\t");
    fixture.terminal.type_keys(b"\x1b[200~first\nsecond\x1b[201~");
    fixture.terminal.pump(0.5, None);
    let lock = fs::File::open(fixture.root.join("run/claude-todo-editor.lock")).unwrap();
    assert!(lock.try_lock().is_err(), "reload could interrupt live native input");
    assert!(fixture.stored().is_none(), "draft became work before save/recovery");
    fixture.terminal.child.kill().unwrap();
    fixture.terminal.child.wait().unwrap();
    assert!(lock.try_lock().is_ok(), "crashed sidebar retained the reload lock");
    drop(lock);
    fs::write(fixture.root.join("run/cockpit.current"), "different-view").unwrap();
    fixture.restart();
    fixture.terminal.pump(5.0, Some(b"recovered 1 TODO"));
    let stored = fixture.stored().expect("crashed native draft was lost");
    assert_eq!(stored["items"].as_array().unwrap().len(), 1);
    assert_eq!(stored["items"][0]["text"], "Recovered café 界");
    assert_eq!(stored["items"][0]["notes"], "first\nsecond");
    assert_eq!(stored["items"][0]["category"], "bug");
    assert_eq!(stored["items"][0]["project"], "test-project");
    fixture.restart();
    fixture.terminal.pump(0.5, None);
    assert_eq!(fixture.stored().unwrap(), stored, "repeated startup duplicated or rewrote the recovered TODO");
}

#[test]
fn reviewed_native_terminal_cancel_cleanup_rejects_input_and_save_for_add_and_edit() {
    for (edit, retry) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut fixture = ComposerTerminal::new();
        fixture.terminal.pump(5.0, Some(b"Todo"));
        fixture.terminal.type_keys(b"3t");
        fixture.terminal.pump(5.0, Some(b"Add TODO"));
        if edit {
            fixture.terminal.type_keys(b"original title\x13");
            fixture.terminal.pump(0.4, None);
            fixture.terminal.type_keys(b"e");
            fixture.terminal.pump(0.4, None);
        }
        let before = fixture.stored();
        fixture.terminal.type_keys(b" cancelled text");
        fixture.terminal.pump(0.4, None);
        let directory = fixture.root.join("state/cockpit/todo-drafts");
        let buffer = fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().path()).find(|path| path.extension().is_some_and(|extension| extension == "md")).unwrap();
        fs::remove_file(&buffer).unwrap();
        fs::create_dir(&buffer).unwrap();
        fixture.terminal.screen.clear();
        fixture.terminal.type_keys(b"\x1b");
        fixture.terminal.pump(0.4, None);
        assert!(contains(&fixture.terminal.screen, b"cleanup pending"));
        fixture.terminal.type_keys(b"\t\x1b[C\x1b[Atyped\x1b[200~pasted\x1b[201~\x13\r");
        fixture.terminal.pump(0.4, None);
        assert_eq!(fixture.stored(), before, "cancelled native form allowed a save");
        if retry {
            fs::remove_dir(&buffer).unwrap();
            fixture.terminal.type_keys(b"\x1b");
            fixture.terminal.pump(0.4, None);
            let lock = fs::File::open(fixture.root.join("run/claude-todo-editor.lock")).unwrap();
            assert!(lock.try_lock().is_ok(), "Esc retry did not release the reload lock");
            assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        }
        fixture.restart();
        fixture.terminal.pump(5.0, Some(b"Todo"));
        assert_eq!(fixture.stored(), before, "restart recovered cancelled native text");
    }
}
