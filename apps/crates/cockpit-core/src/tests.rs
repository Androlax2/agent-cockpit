use crate::teams::store::Connection as LinkConnection;
use crate::{todo::TodoList, util};
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

#[path = "../../../cockpit/tests/common/link_fixture.rs"]
mod link_fixture;
mod session_link_saves;

const SESSION_A: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const SESSION_B: &str = "bbbbbbbb-0000-4000-8000-000000000000";

static ENVIRONMENT: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let lock = ENVIRONMENT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!("sidebar-test-{}", util::token_hex(6)));
        for folder in ["home", "run", "state", "bin"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let path = format!("{}:/usr/bin:/bin", root.join("bin").display());
        // SAFETY: the lock above keeps every test that touches the environment one at a time.
        unsafe {
            std::env::set_var("HOME", root.join("home"));
            std::env::set_var("XDG_RUNTIME_DIR", root.join("run"));
            std::env::set_var("XDG_STATE_HOME", root.join("state"));
            // The audit log goes under state/; never into the real journal.
            std::env::set_var("CLAUDE_AUDIT_JOURNAL", "off");
            std::env::set_var("PATH", path);
            std::env::set_var("CLAUDE_STATUS_URL", "");
            std::env::set_var("CODEX_STATUS_URL", "");
            // The real disk's fill would add a warning to the sessions panel; a test sets its own.
            std::env::set_var("COCKPIT_DISK_MOUNTS", "");
            // H and the costs read opencode's database: never Theo's, wherever XDG_DATA_HOME points.
            std::env::set_var(
                "COCKPIT_OPENCODE_DB",
                root.join("home/.local/share/opencode/opencode.db"),
            );
            // COCKPIT_STATE_HOME too: set in opencode sessions, it would win over the
            // XDG_STATE_HOME above and point the tests at Theo's real state.
            for name in [
                "COCKPIT_STATE_HOME",
                "COCKPIT_HISTORY",
                "COCKPIT_TRANSCRIPTS",
                "CLAUDE_TODO_UPDATES",
                "CLAUDE_REVIEWS_FIXTURE",
                "COCKPIT_APPROVAL_ID",
                "COCKPIT_ANSWERS_KEY",
                "COCKPIT_ANSWERS_DIR",
                "COCKPIT_RUNS_DIR",
                "COCKPIT_REMOTE_DIR",
                "COCKPIT_REMOTE_ADDRESS",
                "COCKPIT_TODO_PATH",
                "COCKPIT_TODO_UPDATES",
                "COCKPIT_DESIGNS_DIR",
                "COCKPIT_DOTFILES_WORK",
                "COCKPIT_GREYWALL_WORK",
                "COCKPIT_GREYPROXY_WORK",
                "COCKPIT_REVIEWS_FIXTURE",
                "COCKPIT_CLONES",
                "COCKPIT_BOOT_TIME",
                "COCKPIT_GREYPROXY_DB",
                "CLAUDE_GREYWALL_WORK",
                "CLAUDE_GREYPROXY_WORK",
                "HYPRLAND_INSTANCE_SIGNATURE",
            ] {
                std::env::remove_var(name);
            }
        }
        let fixture = Fixture { root, _lock: lock };
        // fzf too: the real one opens a picker on the terminal running the tests and waits there
        // (make test on the host). Its stand-in picks nothing: a test that needs a pick shims its own.
        for name in ["cockpit", "hyprctl", "osd-show", "xdg-open", "tmux", "fzf"] {
            fixture.shim(name, "");
        }
        fixture
    }

    fn shim(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        let log = self.root.join("calls.log");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"{name} $*\" >> '{}'\n{body}\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

thread_local! {
    static TODO_SAVE_BARRIER: std::cell::RefCell<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn pause_todo_before_json() {
    if let Some((reached, resume)) = TODO_SAVE_BARRIER.with(|barrier| barrier.borrow_mut().take()) {
        reached.send(()).unwrap();
        resume.recv().unwrap();
    }
}

#[test]
fn todo_terminal_mutations_exclude_reopening_until_json_is_durable() {
    use crate::teams::{TaskKey, links, store};
    let mut violations = Vec::new();
    for action in ["done", "remove", "retention"] {
        let fixture = Fixture::new();
        let project = fixture.root.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let mut todos = TodoList::load();
        let id = todos.add("project", project.to_str().unwrap(), "Joint work");
        let task = TaskKey {
            project: format!("path:{}", project.display()),
            task_id: id.clone(),
        };
        let mut connection = store::open(&crate::paths::team_database()).unwrap();
        link_fixture::approve(&connection, &task.project, &id);
        let mut reopening = todos.clone();
        let (reached, before_json) = std::sync::mpsc::channel();
        let (resume, released) = std::sync::mpsc::channel();
        let terminal_id = id.clone();
        let terminal = std::thread::spawn(move || {
            TODO_SAVE_BARRIER.with(|barrier| *barrier.borrow_mut() = Some((reached, released)));
            match action {
                "done" => todos.set_status(&terminal_id, "done", ""),
                "remove" => todos.remove(&terminal_id).is_ok(),
                "retention" => {
                    todos.items[0].0.insert("status".into(), json!("done"));
                    todos.items[0].0.insert("updated".into(), json!(0));
                    todos.try_save().is_ok()
                }
                _ => unreachable!(),
            }
        });
        before_json.recv_timeout(Duration::from_secs(5)).unwrap();
        let generation: i64 = connection
            .query_row("SELECT generation FROM link_work", [], |row| row.get(0))
            .unwrap();
        assert_eq!(generation, 5, "barrier must follow durable invalidation");
        assert_eq!(
            TodoList::load().status_of(&id),
            "todo",
            "barrier must precede terminal JSON"
        );
        let reopened_during_mutation = reopening.set_status(&id, "todo", "");
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate);
        assert!(
            matches!(transaction, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::DatabaseBusy),
            "enrollment and final claim must wait until terminal JSON is durable"
        );
        drop(transaction);
        resume.send(()).unwrap();
        assert!(
            terminal.join().unwrap(),
            "{action} must complete after the barrier is released"
        );
        if action == "done" {
            assert_eq!(TodoList::load().status_of(&id), "done");
        } else {
            assert!(TodoList::load().items.is_empty());
        }
        assert_eq!(
            reopening.set_status(&id, "todo", ""),
            action == "done",
            "deleted work cannot be resurrected by a stale snapshot"
        );
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        assert!(links::require_open_work(&transaction, &task, 4).is_err());
        drop(transaction);
        if reopened_during_mutation {
            violations.push(format!(
                "{action}: concurrent reopen={reopened_during_mutation}"
            ));
        }
        assert_eq!(link_fixture::state(&connection, &id), "closed");
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn todo_reopening_excludes_terminal_mutations_across_its_json_boundary() {
    for action in ["done", "remove", "retention"] {
        let fixture = Fixture::new();
        let project = fixture.root.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let mut todos = TodoList::load();
        let id = todos.add("project", project.to_str().unwrap(), "Joint work");
        let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
        link_fixture::approve(&connection, &format!("path:{}", project.display()), &id);
        assert!(todos.set_status(&id, "done", ""));
        let mut reopening = todos.clone();
        let reopen_id = id.clone();
        let (reached, before_json) = std::sync::mpsc::channel();
        let (resume, released) = std::sync::mpsc::channel();
        let opener = std::thread::spawn(move || {
            TODO_SAVE_BARRIER.with(|barrier| *barrier.borrow_mut() = Some((reached, released)));
            reopening.set_status(&reopen_id, "todo", "")
        });
        before_json.recv_timeout(Duration::from_secs(5)).unwrap();
        let concurrent_terminal_succeeded = match action {
            "done" => todos.set_status(&id, "done", ""),
            "remove" => todos.remove(&id).is_ok(),
            "retention" => {
                todos.items[0].0.insert("updated".into(), json!(0));
                todos.try_save().is_ok()
            }
            _ => unreachable!(),
        };
        let generation: i64 = connection
            .query_row("SELECT generation FROM link_work", [], |row| row.get(0))
            .unwrap();
        let status_before_release = TodoList::load().status_of(&id);
        resume.send(()).unwrap();
        assert!(opener.join().unwrap());
        assert!(
            !concurrent_terminal_succeeded,
            "{action} bypassed the reopen guard"
        );
        assert_eq!(generation, 5, "{action} changed lifecycle during reopening");
        assert_eq!(
            status_before_release, "done",
            "{action} changed JSON during reopening"
        );
        assert_eq!(TodoList::load().status_of(&id), "todo");
        assert_eq!(link_fixture::state(&connection, &id), "closed");
    }
}
