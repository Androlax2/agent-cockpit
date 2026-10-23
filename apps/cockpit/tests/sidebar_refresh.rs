//! Exercise idle scheduling through the real terminal entry point.
mod common;

use common::{Terminal, contains};
use std::{fs, path::{Path, PathBuf}};
use common::sandbox::write_executable;

struct Sidebar {
    terminal: Terminal,
    root: PathBuf,
}

impl Sidebar {
    fn new() -> Self {
        Self::with_status("")
    }

    fn with_status(status_url: &str) -> Self {
        let root = std::env::temp_dir().join(format!("sidebar-refresh-{}", cockpit::util::token_hex(6)));
        for directory in ["home", "run", "state", "bin"] { fs::create_dir_all(root.join(directory)).unwrap(); }
        for name in ["cockpit", "hyprctl", "pgrep", "git", "gh"] {
            write_executable(&root.join("bin").join(name), "#!/bin/sh\nexit 0\n");
        }
        write_executable(&root.join("bin/tmux"), &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", root.join("tmux.calls").display()));
        fs::write(root.join("run/cockpit.json"), "{\"sessions\":[]}").unwrap();
        let terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_cockpit-sidebar")), &[], &[
            ("HOME", &root.join("home")), ("XDG_RUNTIME_DIR", &root.join("run")),
            ("COCKPIT_STATE_HOME", &root.join("state")), ("XDG_STATE_HOME", &root.join("state")),
            ("PATH", &root.join("bin")), ("CLAUDE_STATUS_URL", Path::new(status_url)), ("CODEX_STATUS_URL", Path::new("")),
            ("CLAUDE_AUDIT_JOURNAL", Path::new("off")), ("COCKPIT_OPENCODE_DB", &root.join("missing.db")),
            ("COCKPIT_DISK_MOUNTS", Path::new("")),
        ]);
        Self { root, terminal }
    }

    fn gathers(&self) -> usize {
        fs::read_to_string(self.root.join("tmux.calls")).unwrap_or_default().lines().filter(|line| line.starts_with("list-panes ")).count()
    }
}

impl Drop for Sidebar {
    fn drop(&mut self) {
        let _ = self.terminal.child.kill();
        let _ = self.terminal.child.wait();
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn an_idle_sidebar_does_not_gather_sessions_four_times_per_second() {
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    assert!(contains(&sidebar.terminal.screen, b"Todo"), "sidebar did not start");
    let before = sidebar.gathers();
    assert!(before > 0, "initial session gather never ran");
    sidebar.terminal.pump(2.0, None);
    assert!(sidebar.gathers() - before <= 1, "idle sidebar still polls tmux: {} gathers", sidebar.gathers() - before);
}

#[test]
fn a_background_provider_result_appears_before_the_next_maintenance_tick() {
    use std::{io::{Read, Write}, net::TcpListener, sync::mpsc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (send, receive) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        stream.read(&mut request).unwrap();
        receive.recv().unwrap();
        let body = r#"{"status":{"indicator":"minor","description":"Worker-ready"}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let mut sidebar = Sidebar::with_status(&url);
    sidebar.terminal.pump(1.2, None);
    send.send(()).unwrap();
    server.join().unwrap();
    sidebar.terminal.pump(0.4, Some(b"Worker-ready"));
    assert!(contains(&sidebar.terminal.screen, b"Worker-ready"), "background result waited for maintenance");
}
