use super::*;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

fn hold_snapshot(fixture: &Fixture) -> (Receiver<()>, Sender<()>) {
    use std::io::{Read, Write};
    let (arrived, received) = channel();
    let (release, held) = channel();
    fixture.serve(move |mut stream| {
        let mut wire = String::new();
        stream.read_to_string(&mut wire).unwrap();
        let request: Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(request["request"], "snapshot");
        arrived.send(()).unwrap();
        held.recv_timeout(Duration::from_secs(5)).unwrap();
        let reply = crate::teams::TeamReply {
            request_id: "snapshot".into(),
            result: Ok(sidebar_snapshot()),
        };
        let _ = stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes());
    });
    (received, release)
}

#[test]
fn a_pending_team_snapshot_does_not_delay_app_drop() {
    let fixture = Fixture::new();
    let (received, release) = hold_snapshot(&fixture);
    let mut app = App::new();
    app.check_team();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    let started = Instant::now();
    drop(app);
    let elapsed = started.elapsed();
    release.send(()).unwrap();
    assert!(
        elapsed < Duration::from_millis(500),
        "drop waited for team IPC: {elapsed:?}"
    );
}

#[test]
fn an_explicit_refresh_rejects_the_already_pending_display_result() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let (received, release) = hold_snapshot(&fixture);
    let mut app = App::new();
    app.build();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    app.team_checked = 0.0;
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !app.collect_team() {
        assert!(Instant::now() < deadline, "worker result never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.redraw();
    let shown = texts(&app, 1).join("\n");
    assert!(
        shown.contains("Team links unavailable"),
        "old result was treated as current: {shown}"
    );
    assert!(
        !shown.contains("› 1d3fd318"),
        "invalidated membership reached the display: {shown}"
    );
}

#[test]
fn changing_the_viewed_session_rejects_the_pending_display_result() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    fixture.view(SESSION_A);
    let (received, release) = hold_snapshot(&fixture);
    let mut app = App::new();
    app.build();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    fixture.view(OPENCODE_SESSION);
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !app.collect_team() {
        assert!(Instant::now() < deadline, "worker result never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.redraw();
    let shown = texts(&app, 1).join("\n");
    assert!(
        shown.contains("Team links unavailable"),
        "the replacement view accepted an old result: {shown}"
    );
}

fn recover_after_team_deadline(app: &mut App, fixture: &Fixture) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !app.collect_team() {
        assert!(
            Instant::now() < deadline,
            "team IPC retained the refresh slot beyond its deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    app.redraw();
    assert!(texts(app, 1).join("\n").contains("Team links unavailable"));

    team_host(fixture, sidebar_snapshot(), vec![]);
    app.team_checked = 0.0;
    app.check_team();
    settle_team(app);
    assert!(
        texts(app, 1).join("\n").contains("› 1d3fd318"),
        "the replacement daemon could not refresh team membership"
    );
}

#[test]
fn a_trickling_team_reply_releases_the_refresh_slot_for_recovery() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let path = crate::paths::team_host_socket();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(path).unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::scope(|scope| {
        let (release, held) = channel::<()>();
        let (arrived, received) = channel();
        scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(14);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "team worker never connected");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept trickling fixture: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap()["request"],
                "snapshot"
            );
            arrived.send(()).unwrap();
            while held.recv_timeout(Duration::from_millis(100))
                == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                && Instant::now() < deadline
            {
                if stream.write_all(b" ").is_err() {
                    break;
                }
            }
        });
        let mut app = App::new();
        app.build();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        recover_after_team_deadline(&mut app, &fixture);
        drop(release);
    });
}

#[test]
fn a_full_team_connect_queue_releases_the_refresh_slot_for_recovery() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let path = crate::paths::team_host_socket();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    // SAFETY: the listener owns a live socket fd; listen only changes its queue limit.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let _queued = UnixStream::connect(path).unwrap();
    let mut app = App::new();
    app.build();
    recover_after_team_deadline(&mut app, &fixture);
}

#[test]
fn slow_focus_and_snapshot_share_one_refresh_deadline() {
    use std::io::{Read, Write};
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    fixture.view(SESSION_A);
    fixture.shim("hyprctl", "printf '%s\\n' '{\"id\":11}'");
    let (arrived, received) = channel();
    let mut request_count = 0;
    fixture.serve(move |mut stream| {
        let mut wire = String::new();
        stream.read_to_string(&mut wire).unwrap();
        let request: Value = serde_json::from_str(&wire).unwrap();
        request_count += 1;
        assert_eq!(
            request["request"],
            if request_count == 1 {
                "chief_focus"
            } else {
                "snapshot"
            }
        );
        arrived.send(request_count).unwrap();
        // Each phase stays active below the per-I/O timeout, but together exceeds ten seconds.
        let until = Instant::now() + Duration::from_secs(6);
        while Instant::now() < until {
            if stream.write_all(b" ").is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let reply = crate::teams::TeamReply {
            request_id: "slow-phase".into(),
            result: Ok(if request_count == 1 {
                json!({"recorded": true})
            } else {
                sidebar_snapshot()
            }),
        };
        let _ = stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes());
    });
    let mut app = App::new();
    let started = Instant::now();
    app.build();
    assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
    assert_eq!(received.recv_timeout(Duration::from_secs(8)).unwrap(), 2);
    let limit = started + Duration::from_millis(11_500);
    while !app.collect_team() && Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.collect_team(),
        "snapshot received a fresh deadline after slow focus"
    );
    app.redraw();
    assert!(texts(&app, 1).join("\n").contains("Team links unavailable"));
    fixture.shim("hyprctl", "");
    recover_after_team_deadline(&mut app, &fixture);
}
