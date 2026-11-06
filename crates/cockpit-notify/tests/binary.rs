//! #62: `torlek-notify` run as the owner runs it, against a scripted daemon on a real Unix
//! socket, with fake `notify-send` and sound programs on `PATH` that record how they were
//! called. Nothing here reaches the owner's desktop or sound server.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_protocol::{
    AgentState, Capability, ChangeBatch, Command, EntityKind, Hello, Outcome, QuestionSource,
    Reply, Request, Response, SUPPORTED_VERSIONS, ServerFrame, SessionReport, SessionStateView,
    SessionView, negotiate,
};
use common::{after_snapshot, report, session, snapshot, upsert};

const WORKER: &str = "s00000000000a";
const GENEROUS: Duration = Duration::from_secs(15);

/// A daemon that answers the feed with a snapshot of one working session, the reports request
/// with `reports`, the queue and the network with an empty list, and once all three were
/// answered, sends `later` on the feed.
fn scripted_daemon(socket: &Path, reports: Vec<SessionReport>, later: ChangeBatch) {
    let listener = UnixListener::bind(socket).unwrap();
    let answered = Arc::new(AtomicUsize::new(0));
    let has_sent_later = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let answered = Arc::clone(&answered);
            let has_sent_later = Arc::clone(&has_sent_later);
            let later = later.clone();
            let reports = reports.clone();
            std::thread::spawn(move || serve(stream, &answered, &has_sent_later, reports, later));
        }
    });
}

fn serve(
    stream: UnixStream,
    answered: &Arc<AtomicUsize>,
    has_sent_later: &Arc<AtomicBool>,
    reports: Vec<SessionReport>,
    later: ChangeBatch,
) {
    let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
    let mut lines = BufReader::new(stream).lines();
    let Some(Ok(hello)) = lines.next() else {
        return;
    };
    let hello: Hello = serde_json::from_str(&hello).unwrap();
    send_frame(
        &writer,
        &negotiate(&hello, &SUPPORTED_VERSIONS, &[Capability::ChangeFeed]),
    );
    let first = snapshot(vec![upsert(
        EntityKind::Session,
        WORKER,
        &session(WORKER, 1, 1),
    )]);
    for line in lines {
        let Ok(line) = line else { return };
        let request: Request = serde_json::from_str(&line).unwrap();
        let reply = match request.command {
            Command::Subscribe { .. } => Reply::Subscribed {
                cursor: first.cursor.clone(),
            },
            Command::AgentReports => Reply::AgentReports {
                reports: reports.clone(),
            },
            Command::ApprovalQueue => Reply::ApprovalQueue {
                runs: Vec::new(),
                guides: Vec::new(),
            },
            Command::NetworkList => Reply::NetworkRequests {
                requests: Vec::new(),
            },
            other => panic!("the notifier sent {other:?}"),
        };
        let is_subscribe = matches!(reply, Reply::Subscribed { .. });
        if !is_subscribe {
            answered.fetch_add(1, Ordering::SeqCst);
        }
        send_frame(
            &writer,
            &ServerFrame::Response(Response {
                id: request.id,
                outcome: Outcome::Ok(reply),
            }),
        );
        if is_subscribe {
            send_frame(&writer, &ServerFrame::Changes(first.clone()));
            if !has_sent_later.swap(true, Ordering::SeqCst) {
                let writer = Arc::clone(&writer);
                let answered = Arc::clone(answered);
                let later = later.clone();
                std::thread::spawn(move || {
                    while answered.load(Ordering::SeqCst) < 3 {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    send_frame(&writer, &ServerFrame::Changes(later));
                });
            }
        }
    }
}

fn send_frame(writer: &Mutex<UnixStream>, frame: &impl serde::Serialize) {
    let mut line = serde_json::to_vec(frame).unwrap();
    line.push(b'\n');
    let _ = writer.lock().unwrap().write_all(&line);
}

fn finished() -> ChangeBatch {
    let view = SessionView {
        finished: true,
        ..session(WORKER, 1, 1)
    };
    after_snapshot(vec![upsert(EntityKind::Session, WORKER, &view)])
}

/// A program on the fake `PATH` that appends its name and arguments to `record`.
fn fake_program(bin: &Path, name: &str, record: &Path) {
    let script = bin.join(name);
    fs::write(
        &script,
        format!(
            // One write per call: the two programs run at once and append to one file.
            "#!/bin/sh\nline=\"${{0##*/}}\"\nfor argument in \"$@\"; do line=\"$line [$argument]\"; done\nprintf '%s\\n' \"$line\" >> '{}'\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Setup {
    dir: tempfile::TempDir,
    bin: PathBuf,
    record: PathBuf,
}

impl Setup {
    fn new() -> Setup {
        Setup::with_later(finished())
    }

    /// A daemon that sends `later` once the notifier loaded its start state.
    fn with_later(later: ChangeBatch) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let record = dir.path().join("record");
        let setup = Setup { dir, bin, record };
        setup.bind(Vec::new(), later);
        setup
    }

    /// Starts the notifier; `is_desktop` gives it a session bus and a display.
    fn start(&self, extra: &[&str], is_desktop: bool) -> Running {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek-notify"));
        command
            .arg("--socket")
            .arg(self.dir.path().join("daemon.sock"))
            .arg("--state-dir")
            .arg(self.dir.path().join("state"))
            .args(extra)
            .env_clear()
            .env("PATH", &self.bin)
            .env("XDG_DATA_DIRS", self.dir.path().join("share"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if is_desktop {
            command
                .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
                .env("WAYLAND_DISPLAY", "wayland-test");
        }
        Running(command.spawn().unwrap())
    }

    /// Starts no daemon yet: [`Setup::bind`] does.
    fn without_daemon() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let record = dir.path().join("record");
        Setup { dir, bin, record }
    }

    fn bind(&self, reports: Vec<SessionReport>, later: ChangeBatch) {
        scripted_daemon(&self.dir.path().join("daemon.sock"), reports, later);
    }

    /// The recorded calls once `count` are in, or what is there after [`GENEROUS`].
    fn calls(&self, count: usize) -> Vec<String> {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let calls: Vec<String> = fs::read_to_string(&self.record)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect();
            if calls.len() >= count || Instant::now() > deadline {
                return calls;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// What the notifier logged, all files together.
    fn log(&self) -> String {
        let logs = self.dir.path().join("state/logs/notify");
        let mut text = String::new();
        for entry in fs::read_dir(logs).into_iter().flatten().flatten() {
            text.push_str(&fs::read_to_string(entry.path()).unwrap_or_default());
        }
        text
    }
}

#[test]
fn a_finished_session_shows_a_notification_and_plays_its_sound() {
    let setup = Setup::new();
    fake_program(&setup.bin, "notify-send", &setup.record);
    fake_program(&setup.bin, "canberra-gtk-play", &setup.record);
    let _notifier = setup.start(&[], true);

    let mut calls = setup.calls(2);
    calls.sort();

    assert_eq!(
        calls,
        [
            "canberra-gtk-play [--id=complete] [--description=Torlek: finished]".to_owned(),
            format!("notify-send [--app-name=Torlek] [--urgency=normal] [--] [{WORKER} finished]"),
        ]
    );
}

#[test]
fn an_event_turned_off_for_sound_still_shows_its_notification() {
    let setup = Setup::new();
    fake_program(&setup.bin, "notify-send", &setup.record);
    fake_program(&setup.bin, "canberra-gtk-play", &setup.record);
    let _notifier = setup.start(&["--sound", "failure,hold"], true);

    let calls = setup.calls(1);
    std::thread::sleep(Duration::from_millis(300));
    let calls_after = setup.calls(calls.len());

    assert_eq!(calls_after.len(), 1, "{calls_after:?}");
    assert!(calls_after[0].starts_with("notify-send"), "{calls_after:?}");
}

#[test]
fn without_canberra_the_player_plays_the_themes_file() {
    let setup = Setup::new();
    let stereo = setup.dir.path().join("share/sounds/freedesktop/stereo");
    fs::create_dir_all(&stereo).unwrap();
    fs::write(stereo.join("complete.oga"), b"").unwrap();
    fake_program(&setup.bin, "pw-play", &setup.record);
    let _notifier = setup.start(&["--desktop", "none"], true);

    let calls = setup.calls(1);

    assert_eq!(
        calls,
        [format!(
            "pw-play [{}]",
            stereo.join("complete.oga").display()
        )]
    );
}

#[test]
fn a_headless_host_runs_the_notifier_with_both_outlets_off() {
    let setup = Setup::new();
    // Nothing on PATH and no session bus.
    let mut notifier = setup.start(&[], false);

    let deadline = Instant::now() + GENEROUS;
    while !setup.log().contains("sounds are off") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(500));

    assert_eq!(
        notifier.0.try_wait().unwrap(),
        None,
        "the notifier keeps running"
    );
    let log = setup.log();
    assert!(log.contains("desktop notifications are off"), "{log}");
    assert!(log.contains("no session bus"), "{log}");
    assert!(log.contains("sounds are off"), "{log}");
}

#[test]
fn events_that_land_together_make_one_notification_and_one_sound() {
    const OTHER: &str = "s00000000000b";
    let failed = SessionView {
        state: SessionStateView::Failed,
        reason: Some("the launch failed".to_owned()),
        ..session(OTHER, 1, 1)
    };
    let finished = SessionView {
        finished: true,
        ..session(WORKER, 1, 1)
    };
    let setup = Setup::with_later(after_snapshot(vec![
        upsert(EntityKind::Session, WORKER, &finished),
        upsert(EntityKind::Session, OTHER, &failed),
    ]));
    fake_program(&setup.bin, "notify-send", &setup.record);
    fake_program(&setup.bin, "canberra-gtk-play", &setup.record);
    let _notifier = setup.start(&[], true);

    // The body's second line lands on a record line of its own.
    let mut calls = setup.calls(3);
    std::thread::sleep(Duration::from_millis(300));
    calls = setup.calls(calls.len());
    calls.sort();

    assert_eq!(
        calls,
        [
            "canberra-gtk-play [--id=dialog-error] [--description=Torlek: failure]".to_owned(),
            format!(
                "notify-send [--app-name=Torlek] [--urgency=critical] [--] [{OTHER} failed (1 more)] \
                 [{OTHER} failed: the launch failed"
            ),
            format!("{WORKER} finished]"),
        ]
    );
}

#[test]
fn without_a_display_the_player_plays_instead_of_canberra() {
    let setup = Setup::new();
    let stereo = setup.dir.path().join("share/sounds/freedesktop/stereo");
    fs::create_dir_all(&stereo).unwrap();
    fs::write(stereo.join("complete.oga"), b"").unwrap();
    fake_program(&setup.bin, "canberra-gtk-play", &setup.record);
    fake_program(&setup.bin, "paplay", &setup.record);
    let _notifier = setup.start(&[], false);

    let calls = setup.calls(1);

    assert_eq!(
        calls,
        [format!(
            "paplay [{}]",
            stereo.join("complete.oga").display()
        )]
    );
}

#[test]
fn what_waited_before_the_daemon_could_be_reached_is_not_news() {
    let setup = Setup::without_daemon();
    fake_program(&setup.bin, "notify-send", &setup.record);
    let _notifier = setup.start(&["--sound", "none"], true);
    // The notifier tried the socket and found nothing there.
    let deadline = Instant::now() + GENEROUS;
    while !setup.log().contains("cannot reach the daemon") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let waiting = SessionReport {
        state: Some(AgentState::Waiting),
        needs_input: Some("Which branch?".to_owned()),
        question_source: Some(QuestionSource::Agent),
        ..report(WORKER)
    };

    setup.bind(vec![waiting], finished());
    let calls = setup.calls(1);

    assert_eq!(
        calls,
        [format!(
            "notify-send [--app-name=Torlek] [--urgency=normal] [--] [{WORKER} finished]"
        )]
    );
}
