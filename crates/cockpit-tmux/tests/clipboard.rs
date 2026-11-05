//! The clipboard and a session (#848). Copy out: text copied in an agent's pane reaches the
//! desktop terminal through the viewer's nested client, and no pane reads a paste buffer back.
//! Paste in: CMD+V (`C-v`) in the viewer runs the image paste command; anywhere else, or
//! forwarded to the viewer, it reaches the program as before.
//!
//! The desktop terminal is a second private tmux server: its pane runs the owner's client of
//! Cockpit's server, as kitty would, and it keeps every OSC 52 it receives as a paste buffer.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use cockpit_tmux::{Keystroke, PaneId, Split, Viewing};
use common::{TestServer, eventually, wait_for};

/// Kitty's `TERM` is `xterm-kitty`; tmux gives the clipboard feature to every `xterm*`.
const DESKTOP_CONFIG: &str = "\
set-option -s set-clipboard on
set-option -g default-terminal xterm-256color
";

struct Desktop {
    socket: PathBuf,
}

impl Desktop {
    /// The owner's terminal, attached to the manager session `cockpit`.
    fn attached_to_cockpit(fixture: &TestServer) -> Desktop {
        let config = fixture.home.path().join("desktop.conf");
        fs::write(&config, DESKTOP_CONFIG).expect("the desktop config is writable");
        let desktop = Desktop {
            socket: fixture.home.path().join("desktop.sock"),
        };
        let owner_client = format!(
            "env -u TMUX tmux -S '{}' attach-session -t =cockpit",
            fixture.socket_path().display()
        );
        let started = desktop.run(&[
            "-f".as_ref(),
            config.as_os_str(),
            "-S".as_ref(),
            desktop.socket.as_os_str(),
            "new-session".as_ref(),
            "-d".as_ref(),
            "-s".as_ref(),
            "desk".as_ref(),
            "-x".as_ref(),
            "120".as_ref(),
            "-y".as_ref(),
            "40".as_ref(),
            owner_client.as_ref(),
        ]);
        assert!(started.status.success(), "{started:?}");
        desktop
    }

    fn run(&self, arguments: &[&std::ffi::OsStr]) -> Output {
        Command::new("tmux")
            .args(arguments)
            .env_remove("TMUX")
            .output()
            .expect("tmux runs")
    }

    /// Types `keys` (tmux key names) into the owner's client, as the owner would.
    fn type_keys(&self, keys: &[&str]) {
        let mut arguments: Vec<&std::ffi::OsStr> = vec![
            "-S".as_ref(),
            self.socket.as_os_str(),
            "send-keys".as_ref(),
            "-t".as_ref(),
            "=desk:".as_ref(),
        ];
        arguments.extend(keys.iter().map(std::ffi::OsStr::new));
        let typed = self.run(&arguments);
        assert!(typed.status.success(), "{typed:?}");
    }

    /// What the terminal last received with OSC 52, if anything.
    fn clipboard(&self) -> Option<String> {
        let shown = self.run(&[
            "-S".as_ref(),
            self.socket.as_os_str(),
            "show-buffer".as_ref(),
        ]);
        shown
            .status
            .success()
            .then(|| String::from_utf8_lossy(&shown.stdout).into_owned())
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.run(&[
            "-S".as_ref(),
            self.socket.as_os_str(),
            "kill-server".as_ref(),
        ]);
    }
}

/// The manager session `cockpit` with the agent shown in its viewer, and the owner's terminal
/// attached to it. Returns once both clients of Cockpit's server can write the clipboard.
fn agent_shown_on_the_desktop(fixture: &TestServer, agent_script: &str) -> (Desktop, PaneId) {
    let (_, viewer) = fixture.start_shell("cockpit", "sleep 60");
    let (agent, agent_pane) = fixture.start_shell("agent-1", agent_script);
    fixture
        .server
        .show_in_viewer(&viewer, &agent, Viewing::ReadWrite)
        .expect("respawns the viewer");
    let desktop = Desktop::attached_to_cockpit(fixture);
    eventually("both clients have the clipboard feature", || {
        clients_with_clipboard(fixture) == ["agent-1", "cockpit"]
    });
    (desktop, agent_pane)
}

/// The sessions whose clients tmux sends OSC 52 to, sorted.
fn clients_with_clipboard(fixture: &TestServer) -> Vec<String> {
    let listed = fixture.tmux(&[
        "list-clients",
        "-F",
        "#{session_name} #{client_termfeatures}",
    ]);
    let mut sessions: Vec<String> = listed
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, features)| features.split(',').any(|feature| feature == "clipboard"))
        .map(|(session, _)| session.to_string())
        .collect();
    sessions.sort();
    sessions
}

/// Whether this tmux has `get-clipboard` (3.6 has none), the option Cockpit's config needs
/// before it lets panes' OSC 52 through. Without it, the copy tests have nothing to prove.
fn can_copy_out(fixture: &TestServer) -> bool {
    let shown = fixture.tmux(&["show-options", "-sq", "get-clipboard"]);
    if shown.is_empty() {
        eprintln!(
            "skipped: {} has no get-clipboard, so copy out stays off",
            tmux_version()
        );
        return false;
    }
    true
}

fn tmux_version() -> String {
    let printed = Command::new("tmux").arg("-V").output().expect("tmux runs");
    String::from_utf8_lossy(&printed.stdout).trim().to_owned()
}

#[test]
fn panes_osc_52_is_taken_only_where_tmux_can_refuse_clipboard_queries() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-1", "sleep 60");

    let get_clipboard = fixture.tmux(&["show-options", "-sqv", "get-clipboard"]);
    let set_clipboard = fixture.tmux(&["show-options", "-sv", "set-clipboard"]);

    let expected = if get_clipboard.is_empty() {
        ("", "external\n")
    } else {
        ("off\n", "on\n")
    };
    assert_eq!(
        (get_clipboard.as_str(), set_clipboard.as_str()),
        expected,
        "{}",
        tmux_version()
    );
}

#[test]
fn text_an_agent_copies_with_osc_52_reaches_the_desktop_terminal() {
    let fixture = TestServer::new();
    let copy = "printf '\\033]52;c;%s\\007' \"$(printf agent-copied | base64)\"";
    let (desktop, _) =
        agent_shown_on_the_desktop(&fixture, &format!("{}; {copy}; sleep 60", wait_for("copy")));
    if !can_copy_out(&fixture) {
        return;
    }

    fixture.release("copy");

    eventually("the desktop clipboard holds the agent's text", || {
        desktop.clipboard().as_deref() == Some("agent-copied")
    });
}

#[test]
fn a_line_copied_in_copy_mode_reaches_the_desktop_terminal() {
    let fixture = TestServer::new();
    let (desktop, agent_pane) =
        agent_shown_on_the_desktop(&fixture, "echo owner-selected; sleep 60");
    if !can_copy_out(&fixture) {
        return;
    }
    eventually("the agent printed its line", || {
        fixture
            .tmux(&["capture-pane", "-p", "-t", agent_pane.as_str()])
            .contains("owner-selected")
    });

    // The owner's keys: the prefix twice sends one prefix on to the viewer's nested client,
    // whose copy mode then opens on the agent's pane rather than on the viewer.
    desktop.type_keys(&["C-b", "C-b", "["]);
    eventually("the agent's pane is in copy mode", || {
        fixture.tmux(&[
            "display-message",
            "-p",
            "-t",
            agent_pane.as_str(),
            "#{pane_in_mode}",
        ]) == "1\n"
    });
    fixture.tmux(&[
        "send-keys",
        "-t",
        agent_pane.as_str(),
        "-X",
        "history-top",
        ";",
        "send-keys",
        "-t",
        agent_pane.as_str(),
        "-X",
        "select-line",
        ";",
        "send-keys",
        "-t",
        agent_pane.as_str(),
        "-X",
        "copy-selection-and-cancel",
    ]);

    eventually("the desktop clipboard holds the selected line", || {
        desktop.clipboard().as_deref() == Some("owner-selected")
    });
}

#[test]
fn a_pane_asking_for_the_clipboard_gets_no_answer() {
    let fixture = TestServer::new();
    // Raw, unechoed input, read until a second passes without any. The device-attributes
    // query after the clipboard one is always answered, and tmux answers in order, so its
    // answer proves the clipboard query was read and had its chance to be answered.
    let ask = "stty -icanon -echo min 0 time 10; printf '\\033]52;c;?\\007\\033[c'; \
               dd bs=1 count=4096 of=answer.part 2>/dev/null; mv answer.part answer";
    fixture.start_shell("agent-1", &format!("{}; {ask}; sleep 60", wait_for("ask")));
    fixture.tmux(&["set-buffer", "owner-copied-elsewhere"]);
    let answer = fixture.home.path().join("answer");

    fixture.release("ask");

    eventually("the pane stopped reading answers", || answer.exists());
    let answer = String::from_utf8_lossy(&fs::read(&answer).expect("readable")).into_owned();
    assert!(
        answer.starts_with("\u{1b}[?"),
        "the device attributes come back: {answer:?}"
    );
    assert!(!answer.contains("]52;"), "no clipboard answer: {answer:?}");
}

/// A shell line that writes the first byte the pane's program reads, raw, to `name`.
fn first_byte_to(name: &str) -> String {
    format!("stty raw -echo; dd bs=1 count=1 of={name}.part 2>/dev/null; mv {name}.part {name}")
}

fn read(fixture: &TestServer, name: &str) -> Option<String> {
    fs::read_to_string(fixture.home.path().join(name)).ok()
}

/// The agent shown in the viewer of `cockpit`, the paste command bound, and the owner's
/// terminal attached. Returns the terminal and the viewer.
fn viewer_with_image_paste(fixture: &TestServer, command: &[&str]) -> (Desktop, PaneId) {
    let (_, viewer) = fixture.start_shell("cockpit", "sleep 60");
    let (agent, _) = fixture.start_shell(
        "agent-1",
        &format!("{}; sleep 60", first_byte_to("agent-read")),
    );
    fixture
        .server
        .show_in_viewer(&viewer, &agent, Viewing::ReadWrite)
        .expect("respawns the viewer");
    let command: Vec<std::ffi::OsString> = command.iter().map(Into::into).collect();
    fixture
        .server
        .bind_image_paste(&viewer, &command)
        .expect("binds the key");
    let desktop = Desktop::attached_to_cockpit(fixture);
    eventually("the owner and the viewer are attached", || {
        fixture
            .tmux(&["list-clients", "-F", "#{session_name}"])
            .lines()
            .count()
            == 2
    });
    (desktop, viewer)
}

#[test]
fn ctrl_v_in_the_viewer_runs_the_image_paste_command_instead_of_reaching_the_agent() {
    let fixture = TestServer::new();
    let (desktop, _) = viewer_with_image_paste(&fixture, &["sh", "-c", "echo ran > pasted"]);

    desktop.type_keys(&["C-v"]);

    eventually("the paste command ran", || {
        read(&fixture, "pasted").is_some()
    });
    assert_eq!(read(&fixture, "agent-read"), None);
}

#[test]
fn a_ctrl_v_forwarded_to_the_viewer_reaches_the_agent() {
    let fixture = TestServer::new();
    let (_desktop, viewer) = viewer_with_image_paste(&fixture, &["sh", "-c", "echo ran > pasted"]);

    fixture
        .server
        .send_keys(&viewer, &[Keystroke::Control('v')])
        .expect("typed");

    eventually("the agent read the key", || {
        read(&fixture, "agent-read").as_deref() == Some("\u{16}")
    });
    assert_eq!(read(&fixture, "pasted"), None);
}

#[test]
fn ctrl_v_in_the_column_reaches_the_column() {
    let fixture = TestServer::new();
    let (_, column) = fixture.start_shell(
        "cockpit",
        &format!("{}; sleep 60", first_byte_to("column-read")),
    );
    let viewer = fixture
        .server
        .split_pane(
            &column,
            Split::SideBySide { percent: 70 },
            &common::shell("sleep 60"),
        )
        .expect("splits");
    fixture
        .server
        .bind_image_paste(
            &viewer,
            &["sh".into(), "-c".into(), "echo ran > pasted".into()],
        )
        .expect("binds the key");
    let desktop = Desktop::attached_to_cockpit(&fixture);
    eventually("the owner is attached", || {
        !fixture.tmux(&["list-clients"]).is_empty()
    });

    desktop.type_keys(&["C-v"]);

    eventually("the column read the key", || {
        read(&fixture, "column-read").as_deref() == Some("\u{16}")
    });
    assert_eq!(read(&fixture, "pasted"), None);
}

#[test]
fn the_image_paste_command_gets_its_words_unchanged() {
    let fixture = TestServer::new();
    let word = "it's one word; $HOME `id`";
    let (desktop, _) = viewer_with_image_paste(
        &fixture,
        &[
            "sh",
            "-c",
            "printf %s \"$1\" > pasted.part; mv pasted.part pasted",
            "sh",
            word,
        ],
    );

    desktop.type_keys(&["C-v"]);

    eventually("the paste command ran", || {
        read(&fixture, "pasted").is_some()
    });
    assert_eq!(read(&fixture, "pasted").as_deref(), Some(word));
}
