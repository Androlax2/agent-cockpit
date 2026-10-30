use super::*;

#[test]
fn an_asleep_note_shows_as_an_asleep_card() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    let app = fixture.app();
    assert_eq!(app.sessions.len(), 1);
    assert_eq!(card_text(&app, 0).lines().next(), Some("☾Asleep"));
    assert_eq!(asleep_badge(&app), "1h");
    assert_eq!(app.running_sessions.len(), 1);
}

#[test]
fn an_asleep_card_waits_in_quiet() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    let mut later = session(SESSION_B, "working", "runway", "/r/runway", now - 5000);
    later["since"] = json!(now);
    fixture.snapshot(json!([later]));
    let mut note = json!({
        "agent": "claude", "project": "runway", "cwd": "/r/runway", "title": "Fix the chart",
        "idle_since": now - 100, "slept_at": now - 50, "remote": false, "started": now - 9000,
    });
    fixture.write(
        &format!("state/cockpit/asleep/{SESSION_A}"),
        &note.to_string(),
    );
    let app = fixture.app();
    let order: Vec<&str> = app
        .sessions
        .iter()
        .map(|session| session.session.as_str())
        .collect();
    assert_eq!(
        order,
        [SESSION_B, SESSION_A],
        "an asleep card waits in QUIET, below the working one"
    );
    note.as_object_mut().unwrap().remove("started");
    fixture.write(
        &format!("state/cockpit/asleep/{SESSION_A}"),
        &note.to_string(),
    );
    let app = fixture.app();
    assert_eq!(
        app.sessions.len(),
        2,
        "a note without started still shows its card"
    );
}

#[test]
fn a_running_session_drops_its_stale_sleep_note() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        now - 60
    )]));
    let app = fixture.app();
    assert_eq!(app.sessions.len(), 1);
    assert!(app.sessions[0].asleep.is_none());
    assert!(
        !fixture
            .path(&format!("state/cockpit/asleep/{SESSION_A}"))
            .exists()
    );
}

#[test]
fn a_session_still_exiting_keeps_its_note_and_shows_one_card() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 10);
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        now - 7200
    )]));
    let app = fixture.app();
    assert_eq!(app.sessions.len(), 1);
    assert!(
        fixture
            .path(&format!("state/cockpit/asleep/{SESSION_A}"))
            .exists()
    );
}

#[test]
fn a_session_being_woken_shows_as_restarting_not_asleep() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    fixture.write(
        &format!("run/cockpit.restarting/{SESSION_A}"),
        &format!("{now}\nrunway\n/r/runway\n%1\n"),
    );
    let app = fixture.app();
    assert_eq!(app.sessions.len(), 1);
    assert!(app.sessions[0].restarting);
    assert!(app.sessions[0].asleep.is_none());
}

#[test]
fn asleep_sessions_are_not_offered_for_restore() {
    let fixture = Fixture::new();
    let boot = util::now_secs() - 3600;
    // SAFETY: the fixture's lock keeps every test that touches the environment one at a time.
    unsafe {
        std::env::set_var("COCKPIT_BOOT_TIME", boot.to_string());
    }
    fixture.write(
        "state/cockpit/recent-sessions.json",
        &json!({SESSION_A: {"cwd": "/r/runway", "project": "runway", "last_seen": boot - 100}})
            .to_string(),
    );
    assert_eq!(
        fixture.app().restore.len(),
        1,
        "without a note the session is offered"
    );
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    let app = fixture.app();
    assert!(app.restore.is_empty(), "{:?}", app.restore);
}

#[test]
fn summary_counts_asleep_sessions() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    let mut ready = session(SESSION_B, "waiting", "other", "/r/other", 2);
    ready["since"] = json!(now);
    fixture.snapshot(json!([ready]));
    let app = fixture.app();
    let summary = app.panels[1]
        .fixed_top
        .iter()
        .map(|line| line.texts())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        summary.contains("1 ready · 1 asleep"),
        "summary was {summary:?}"
    );
    assert!(!summary.contains("idle"), "summary was {summary:?}");
}

#[test]
fn asleep_cards_do_not_count_as_started_before_a_change() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    let mut app = fixture.app();
    assert!(
        app.sessions[0].started.is_some(),
        "the note carries a start time"
    );
    app.start_files = vec![("greywall", now as f64)];
    app.build();
    let summary = app.panels[1]
        .fixed_top
        .iter()
        .map(|line| line.texts())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !summary.contains("restart available"),
        "summary was {summary:?}"
    );
}

#[test]
fn dispatch_sleep_sleeps_only_eligible_sessions() {
    let (fixture, mut app) = sleep_fixture(&[(SESSION_A, 7200), (SESSION_B, 600)]);
    app.dispatch_sleep();
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit sleep {SESSION_A}")]
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fixture.calls("cockpit"),
        vec![format!("cockpit sleep {SESSION_A}")]
    );
}

#[test]
fn dispatch_sleep_does_nothing_when_switched_off() {
    let (fixture, mut app) = sleep_fixture(&[(SESSION_A, 7200)]);
    fixture.write("state/cockpit/settings.json", r#"{"idle-sleep":false}"#);
    app.dispatch_sleep();
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn dispatch_sleep_checks_once_a_minute() {
    let (fixture, mut app) = sleep_fixture(&[(SESSION_A, 7200)]);
    app.dispatch_sleep();
    fixture.wait_for_calls("cockpit", 1);
    app.dispatch_sleep();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fixture.calls("cockpit").len(), 1);
}

#[test]
fn dispatch_sleep_asks_once_per_idle_spell() {
    // A refused sleep stays refused until the session works again: no retry every minute.
    let (fixture, mut app) = sleep_fixture(&[(SESSION_A, 7200)]);
    app.dispatch_sleep();
    fixture.wait_for_calls("cockpit", 1);
    app.sleep_checked = 0.0;
    app.dispatch_sleep();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fixture.calls("cockpit").len(), 1);
    fixture.snapshot(idle_rows(&[(SESSION_A, 3600)]));
    app.build();
    app.sleep_checked = 0.0;
    app.dispatch_sleep();
    assert_eq!(
        fixture.wait_for_calls("cockpit", 2),
        vec![format!("cockpit sleep {SESSION_A}"); 2],
        "idle again since a later time"
    );
}

#[test]
fn unknown_membership_blocks_manual_and_automatic_sleep() {
    let fixture = Fixture::new();
    fixture.snapshot(idle_rows(&[(SESSION_A, 7200)]));
    let mut app = fixture.app();
    fs::remove_file(crate::paths::team_host_socket()).unwrap();
    app.check_team();
    app.handle(Key::Char('z'), &mut FakeScreen::default());
    assert!(app.message.contains("cannot tell yet"), "{}", app.message);
    app.dispatch_sleep();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        fixture.calls("cockpit").is_empty(),
        "no team service: a team member could be put to sleep"
    );
    no_team_members(&fixture);
    app.team_checked = 0.0;
    app.check_team();
    app.sleep_checked = 0.0;
    app.dispatch_sleep();
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit sleep {SESSION_A}")]
    );
}

#[test]
fn z_puts_the_selected_session_to_sleep_without_waiting_for_the_idle_delay() {
    let (fixture, mut app) = sleep_fixture(&[(SESSION_A, 60)]);
    app.handle(Key::Char('z'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit sleep {SESSION_A}")]
    );
    assert!(app.message.contains("to sleep"), "{}", app.message);
}

#[test]
fn z_shows_why_a_session_stays_awake_without_logging_an_error() {
    let (fixture, mut app) = sleep_fixture(&[(SESSION_A, 60)]);
    fixture.shim(
        "cockpit",
        "echo 'cockpit: project-0 stays awake: text is typed in its prompt' >&2; exit 3",
    );
    app.handle(Key::Char('z'), &mut FakeScreen::default());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app.collect_manager() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        app.message,
        "cockpit: project-0 stays awake: text is typed in its prompt"
    );
    let log = fs::read_to_string(crate::paths::log()).unwrap_or_default();
    assert!(
        !log.lines()
            .any(|line| line.contains("ERROR") && line.contains("cockpit sleep")),
        "{log}"
    );
}

#[test]
fn z_can_sleep_a_known_teammate() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_B,
        "waiting",
        "project-a",
        "/r/project-a",
        1
    )]));
    team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    app.check_team();
    app.handle(Key::Char('z'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit sleep {SESSION_B}")]
    );
    assert!(app.message.contains("to sleep"), "{}", app.message);
}

#[test]
fn known_team_member_can_sleep() {
    let fixture = Fixture::new();
    fixture.snapshot(idle_rows(&[(SESSION_B, 7200)]));
    team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    app.check_team();
    app.dispatch_sleep();
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit sleep {SESSION_B}")]
    );
}

#[test]
fn z_on_an_asleep_card_says_it_already_sleeps() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    no_team_members(&fixture);
    let mut app = fixture.app();
    app.check_team();
    app.handle(Key::Char('z'), &mut FakeScreen::default());
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
    assert!(app.message.contains("already asleep"), "{}", app.message);
}

#[test]
fn x_on_an_asleep_card_ends_it_through_the_manager() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    let mut app = fixture.app();
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit exit {SESSION_A}")]
    );
    assert!(
        fixture.calls("osd-show").is_empty(),
        "{:?}",
        fixture.calls("osd-show")
    );
}

#[test]
fn queued_prompts_wait_for_an_asleep_session_to_wake() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    crate::prompts::add(SESSION_A, "run the tests").unwrap();
    let mut app = fixture.app();
    app.dispatch_prompts();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn the_selection_moving_onto_an_asleep_card_does_not_wake_it() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "opusline", "/o", 1)]));
    asleep_note(&fixture, SESSION_B, now - 7200, now - 3600);
    let mut app = fixture.app();
    app.panels[1].selected = app
        .sessions
        .iter()
        .position(|session| session.session == SESSION_A)
        .unwrap();
    fixture.snapshot(json!([]));
    app.build();
    assert_eq!(
        app.selected_session()
            .map(|session| session.session.clone()),
        Some(SESSION_B.to_string())
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn dispatch_sleep_expires_week_old_notes() {
    let (fixture, mut app) = sleep_fixture(&[]);
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 9 * 86_400, now - 8 * 86_400);
    app.dispatch_sleep();
    assert!(
        !fixture
            .path(&format!("state/cockpit/asleep/{SESSION_A}"))
            .exists()
    );
}

#[test]
fn the_queue_gives_an_asleep_session_its_todo() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    asleep_note(&fixture, SESSION_A, now - 7200, now - 3600);
    let mut app = fixture.app();
    app.gaming = || false;
    let key = app.todos.add("runway", "/r/runway", "next job");
    app.todos.toggle_queued(&key);
    app.dispatch_queue();
    let calls = fixture.wait_for_calls("cockpit", 1);
    assert!(
        calls[0].starts_with(&format!("cockpit say {SESSION_A} ")),
        "{calls:?}"
    );
    assert_eq!(app.todos.status_of(&key), "doing");
}

#[test]
fn exit_goes_through_the_manager_when_there_is_a_pane() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit exit {SESSION_A}")]
    );
}

#[test]
fn an_exiting_session_leaves_the_list_at_once() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    fixture.shim("cockpit", "sleep 2");
    let mut app = fixture.app();
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    app.build();
    assert_eq!(
        app.sessions
            .iter()
            .map(|s| s.project.as_str())
            .collect::<Vec<_>>(),
        ["opusline"]
    );
}

#[test]
fn an_exiting_session_stays_hidden_when_its_pane_gets_a_fallback_card() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    fixture.shim("cockpit", "sleep 2");
    fixture.shim(
        "tmux",
        "if [ \"$1\" = list-panes ]; then printf 'claude-retry-runway\t%%1\t/r\trunway\t1\n'; fi",
    );
    let mut app = fixture.app();
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    app.build();
    assert_eq!(
        app.sessions
            .iter()
            .map(|s| s.project.as_str())
            .collect::<Vec<_>>(),
        ["opusline"]
    );

    fixture.snapshot(json!([session(SESSION_B, "waiting", "opusline", "/o", 2)]));
    app.build();
    assert_eq!(
        app.sessions
            .iter()
            .map(|s| s.project.as_str())
            .collect::<Vec<_>>(),
        ["opusline"]
    );

    fixture.shim("tmux", "");
    app.build();
    assert_eq!(
        app.sessions
            .iter()
            .map(|s| s.project.as_str())
            .collect::<Vec<_>>(),
        ["opusline"]
    );
}

#[test]
fn exiting_the_selected_session_previews_its_replacement_once() {
    for selected in [0, 1] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([
            session(SESSION_A, "waiting", "runway", "/r", 1),
            session(SESSION_B, "waiting", "opusline", "/o", 2)
        ]));
        fixture.shim("cockpit", "sleep 2");
        let mut app = fixture.app();
        app.panels[1].selected = selected;
        let exiting = app.sessions[selected].session.clone();
        let replacement = if selected == 0 { SESSION_B } else { SESSION_A };

        app.handle(
            Key::Char('x'),
            &mut FakeScreen::answering(&[Key::Char('y')]),
        );
        app.redraw();
        app.redraw();

        assert_eq!(app.sessions[app.panels[1].selected].session, replacement);
        let mut calls = fixture.wait_for_calls("cockpit", 2);
        calls.sort();
        assert_eq!(
            calls,
            vec![
                format!("cockpit exit {exiting}"),
                format!("cockpit view {replacement}")
            ]
        );
    }
}

#[test]
fn a_restarting_session_keeps_its_card_and_the_selection_while_it_is_gone() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.panels[1].selected = 0;

    note_restart(&fixture, SESSION_A, util::now() as i64, "runway", "/r");
    fixture.snapshot(json!([session(SESSION_B, "waiting", "opusline", "/o", 2)]));
    app.build();

    let selected = &app.sessions[app.panels[1].selected];
    assert_eq!(
        (
            selected.session.as_str(),
            sessions::activity_label(selected)
        ),
        (SESSION_A, "Restarting")
    );
    assert!(
        fixture.calls("cockpit").is_empty(),
        "no other session is viewed: {:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn a_restarted_session_reporting_again_ends_the_hold() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    let since = util::now() as i64;
    note_restart(&fixture, SESSION_A, since, "runway", "/r");
    fixture.snapshot(json!([]));
    app.build();

    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r",
        since + 1
    )]));
    app.build();

    assert!(!app.sessions[0].restarting);
    assert!(
        !fixture
            .path(&format!("run/cockpit.restarting/{SESSION_A}"))
            .exists()
    );
}

#[test]
fn a_fresh_session_in_the_folder_of_a_restart_takes_its_selection() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    let mut app = fixture.app();
    app.panels[1].selected = 0;
    let since = util::now() as i64;
    note_restart(&fixture, SESSION_A, since, "runway", "/r");
    fixture.snapshot(json!([session(SESSION_B, "waiting", "opusline", "/o", 2)]));
    app.build();

    let fresh = "cccccccc-0000-4000-8000-000000000000";
    fixture.snapshot(json!([
        session(SESSION_B, "waiting", "opusline", "/o", 2),
        session(fresh, "waiting", "runway", "/r", since + 1)
    ]));
    app.build();

    assert_eq!(app.sessions[app.panels[1].selected].session, fresh);
    assert!(
        fixture.calls("cockpit").is_empty(),
        "nothing is viewed: {:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn the_unreported_launch_of_a_restart_has_no_card_of_its_own() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    let since = util::now() as i64;
    note_restart(&fixture, SESSION_A, since, "runway", "/r");
    fixture.snapshot(json!([]));
    fixture.shim("tmux", &format!("if [ \"$1\" = list-panes ]; then printf 'claude-retry-restart-1\\t%%5\\t/r\\trunway\\t{}\\t\\n'; fi", since + 1));
    app.build();

    assert_eq!(
        app.sessions
            .iter()
            .map(|s| s.session.as_str())
            .collect::<Vec<_>>(),
        [SESSION_A]
    );
}

#[test]
fn a_restart_that_never_reports_drops_its_card_and_says_so() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([]));
    note_restart(
        &fixture,
        SESSION_A,
        util::now() as i64 - 600,
        "runway",
        "/r",
    );
    let app = fixture.app();

    assert!(app.sessions.is_empty());
    assert!(
        app.message.contains("runway did not come back"),
        "{}",
        app.message
    );
    assert!(
        !fixture
            .path(&format!("run/cockpit.restarting/{SESSION_A}"))
            .exists()
    );
}

#[test]
fn a_restarting_card_refuses_the_keys_that_act_on_its_process() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    note_restart(&fixture, SESSION_A, util::now() as i64, "runway", "/r");
    fixture.snapshot(json!([]));
    app.build();

    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );

    assert_eq!(
        app.message,
        "runway is restarting: wait for it to come back"
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn exiting_the_last_session_does_not_try_to_preview_a_replacement() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    fixture.shim("cockpit", "sleep 2");
    let mut app = fixture.app();

    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    app.redraw();

    assert!(app.sessions.is_empty());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit exit {SESSION_A}")]
    );
}

#[test]
fn a_failed_exit_brings_the_card_back_and_says_so() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    fixture.shim("cockpit", "exit 1");
    let mut app = fixture.app();
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    for _ in 0..100 {
        app.build();
        if !app.sessions.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(app.sessions.len(), 1);
    assert!(
        app.message.starts_with("⚠ runway did not exit"),
        "{}",
        app.message
    );
}

#[test]
fn exit_closes_the_window_of_a_session_without_a_pane() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([{"session": SESSION_A, "state": "waiting", "project": "runway", "since": 1, "kitty_pid": "4242"}]));
    let mut app = fixture.app();
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert!(
        fixture
            .calls("hyprctl")
            .iter()
            .any(|call| call.contains("pid:4242"))
    );
}

#[test]
fn the_viewed_sessions_state_is_pushed_only_when_it_changes() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "working", "runway", "/r", 1)]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.tick();
    app.tick();
    assert_eq!(fixture.wait_for_calls("cockpit label-state", 1).len(), 1);
}

#[test]
fn a_manager_command_started_in_the_background_is_reaped_once_it_ends() {
    let fixture = Fixture::new();
    fixture.shim(
        "cockpit",
        &format!("echo $$ > '{}'", fixture.path("cockpit.pid").display()),
    );
    util::manager_in_background(&["label-state", "working", SESSION_A]);
    assert!(
        is_reaped(pid_written_to(&fixture, "cockpit.pid")),
        "cockpit stays a zombie"
    );
}

#[test]
fn a_session_that_leaves_before_its_exit_ends_does_not_leave_a_zombie() {
    let fixture = Fixture::new();
    fixture.shim(
        "cockpit",
        &format!(
            "echo $$ > '{}'\nsleep 0.5",
            fixture.path("cockpit.pid").display()
        ),
    );
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let pid = pid_written_to(&fixture, "cockpit.pid");
    fixture.snapshot(json!([]));
    app.build();
    assert!(is_reaped(pid), "cockpit exit stays a zombie");
}

#[test]
fn answering_an_explicit_chat_question_updates_the_viewed_state() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "runway", "/r", 1);
    row["needs_input"] = json!(true);
    fixture.snapshot(json!([row.clone()]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit attend", 1),
        [format!("cockpit attend {SESSION_A}")]
    );
    app.tick();
    assert_eq!(
        fixture.wait_for_calls("cockpit label-state", 1),
        [format!("cockpit label-state needs-you {SESSION_A}")]
    );
    row["needs_input"] = json!(false);
    fixture.snapshot(json!([row]));
    app.build();
    app.tick();
    assert_eq!(
        fixture.wait_for_calls("cockpit label-state", 2).last(),
        Some(&format!("cockpit label-state waiting {SESSION_A}"))
    );
}

#[test]
fn when_the_shown_session_ends_the_next_one_is_shown() {
    let fixture = Fixture::new();
    vanish_selected_session(&fixture, None);
    assert!(
        fixture
            .wait_for_calls("cockpit", 1)
            .iter()
            .any(|call| call.starts_with("cockpit view "))
    );
}

#[test]
fn a_session_restarting_in_place_keeps_the_viewer() {
    // 2026-10-21: showing the next card replaced the viewer pane, killing the Claude restarting there.
    let fixture = Fixture::new();
    vanish_selected_session(
        &fixture,
        Some("Restarting runway …\n/r/runway\n\n1\nstart\n"),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.starts_with("cockpit view ")),
        "{:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn sessions_started_before_a_global_change_are_told_to_restart() {
    let fixture = Fixture::new();
    fixture.write("home/.claude/CLAUDE.md", "rules");
    let changed = fs::metadata(fixture.path("home/.claude/CLAUDE.md"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", changed - 100),
        session(SESSION_B, "waiting", "homelab", "/h", changed + 100)
    ]));
    let app = fixture.app();
    assert!(
        app.panels[1]
            .fixed_top
            .iter()
            .any(|line| line.texts().contains("1 restart available"))
    );
    assert!(selected_session_badges(&app).contains(&"⟳ restart".into()));
    assert!(sessions::changed_since_start(&app.sessions[1], &app.start_files).is_empty());
}

#[test]
fn greyproxy_requests_retried_lately_count_as_held_with_the_sandbox_that_asked() {
    let fixture = Fixture::new();
    let database = fixture.path("greyproxy.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("create table pending_requests (container_name text, destination_host text, destination_port integer, last_seen text);
        insert into pending_requests values ('claude-runway', 'api.example.com', 443, datetime('now')),
                                            ('claude-opusline', 'old.example.com', 443, datetime('now', '-1 hour'));").unwrap();
    assert_eq!(
        sessions::greyproxy_held(&database),
        vec!["claude-runway → api.example.com:443".to_string()]
    );
}

#[test]
fn ctrl_r_restarts_the_stale_waiting_sessions_and_names_the_busy_ones() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "working", "opusline", "/o", 2)
    ]));
    let mut app = fixture.app();
    // CLAUDE.md changed after both started: both need a restart.
    app.start_files = vec![("CLAUDE.md", 50.0)];
    app.handle(
        Key::Ctrl('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit restart-all {SESSION_A}")]
    );
    assert!(
        app.message.contains("at the end of their turn: opusline"),
        "{}",
        app.message
    );
}

#[test]
fn ctrl_r_restarts_a_busy_stale_session_once_its_turn_ends() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    let mut app = fixture.app();
    app.start_files = vec![("launcher", 50.0)];
    app.handle(
        Key::Ctrl('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    app.dispatch_prompts();
    assert!(fixture.calls("cockpit").is_empty(), "restarted mid-turn");
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    app.build();
    app.dispatch_prompts();
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit restart {SESSION_A}  detached")]
    );
}

#[test]
fn ctrl_r_declined_leaves_a_busy_stale_session_alone() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    let mut app = fixture.app();
    app.start_files = vec![("launcher", 50.0)];
    app.handle(
        Key::Ctrl('r'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert!(!crate::prompts::restart_wanted(SESSION_A));
}

#[test]
fn plus_starts_another_session_on_the_cards_repository() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.handle(
        Key::Char('+'),
        &mut FakeScreen::typing(&["fix-login", "claude"]),
    );
    assert_eq!(app.message, "cloning runway for fix-login …");
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec!["cockpit --agent claude parallel /r/runway fix-login".to_string()]
    );
}

#[test]
fn restart_all_leaves_terminal_sessions_alone() {
    let fixture = Fixture::new();
    let mut terminal = session(
        "bbbbbbbb-0000-4000-8000-000000000000",
        "waiting",
        "crema",
        "/r/crema",
        100,
    );
    terminal["terminal"] = json!(true);
    fixture.snapshot(json!([
        session(
            "aaaaaaaa-0000-4000-8000-000000000000",
            "waiting",
            "dotfiles",
            "/r/dotfiles",
            100
        ),
        terminal
    ]));
    let mut app = fixture.app();
    app.start_files = vec![("CLAUDE.md", 500.0)];
    app.handle(
        Key::Ctrl('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec!["cockpit restart-all aaaaaaaa-0000-4000-8000-000000000000".to_string()]
    );
}

#[test]
fn external_sessions_are_hidden_without_a_terminal_target() {
    let fixture = Fixture::new();
    for state in ["working", "waiting", "needs-you"] {
        fixture.snapshot(json!([
            session(SESSION_A, "waiting", "runway", "/r/runway", 1),
            external_session(OPENCODE_SESSION, state, "crema", "/r/crema", 2)
        ]));
        let mut app = fixture.app();
        assert_eq!(
            app.sessions
                .iter()
                .map(|session| session.session.as_str())
                .collect::<Vec<_>>(),
            [SESSION_A]
        );
        let buffer = render(&mut app, 100, 45);
        let rows: Vec<String> = (0..45)
            .map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("crema") || row.contains("external")),
            "{rows:?}"
        );
        assert_eq!(
            sessions::reported().len(),
            2,
            "hidden sessions must not be deleted"
        );
    }
}

#[test]
fn external_sessions_reject_terminal_controls_without_running_commands() {
    let fixture = Fixture::new();
    for agent in ["claude", "opencode"] {
        let mut row = external_session(SESSION_A, "waiting", "crema", "/r/crema", 1);
        row["agent"] = json!(agent);
        fixture.snapshot(json!([row]));
        let mut app = fixture.app();
        app.sessions = sessions::reported();
        app.sidebar_rows = vec![crate::teams::sidebar::SidebarRow::Session {
            session_id: SESSION_A.into(),
        }];
        assert_eq!(app.sessions.len(), 1);
        for key in [
            Key::Enter,
            Key::Char('r'),
            Key::Char('e'),
            Key::Char('$'),
            Key::Char('C'),
            Key::Char('x'),
        ] {
            app.handle(key.clone(), &mut FakeScreen::answering(&[Key::Char('y')]));
            assert!(
                app.message.contains("external") && app.message.contains("terminal"),
                "{agent}: {key:?}: {}",
                app.message
            );
            assert!(app.menu.is_none());
        }
    }
    assert!(fixture.calls("cockpit").is_empty());
    assert!(fixture.calls("osd-show").is_empty());
    assert!(
        fixture
            .calls("hyprctl")
            .iter()
            .all(|call| call == "hyprctl -j activeworkspace")
    );
}

#[test]
fn external_sessions_do_not_advertise_or_expose_terminal_controls() {
    let fixture = Fixture::new();
    let mut row = external_session(SESSION_A, "waiting", "crema", "/r/crema", 1);
    row["agent"] = json!("claude");
    row["update"] = json!(true);
    fixture.snapshot(json!([row]));
    fixture.write(
        &format!("home/.cache/claude/statusline-{SESSION_A}.json"),
        r#"{"effort":"high","context":95}"#,
    );
    let mut app = fixture.app();
    app.sessions = sessions::reported();
    app.sidebar_rows = vec![crate::teams::sidebar::SidebarRow::Session {
        session_id: SESSION_A.into(),
    }];
    assert_eq!(app.sessions.len(), 1);
    let hints = app.sessions_hints();
    assert!(
        hints
            .iter()
            .all(|(key, _)| !["⏎", "r", "e", "C", "x", "n"].contains(key)),
        "{hints:?}"
    );
    assert!(
        hints.contains(&("g", "git")),
        "repository actions must stay available"
    );
    render(&mut app, 100, 45);
    assert!(
        !app.hit_actions
            .values()
            .any(|(_, _, action)| *action == "effort"),
        "external effort still clickable"
    );
}

#[test]
fn external_sessions_become_controllable_when_a_terminal_target_is_known() {
    let fixture = Fixture::new();
    let mut row = external_session(OPENCODE_SESSION, "waiting", "crema", "/r/crema", 1);
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    assert!(app.sessions.is_empty());
    row["tmux_pane"] = json!("%9");
    fixture.snapshot(json!([row]));
    app.build();
    assert_eq!(app.sessions.len(), 1);
    assert!(
        !app.panels[1]
            .lines
            .iter()
            .any(|line| line.right.iter().any(|part| part.text == "external"))
    );
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit open {OPENCODE_SESSION}")]
    );
}

#[test]
fn external_sessions_are_skipped_when_peeking_at_sessions_needing_attention() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "working", "runway", "/r/runway", 1),
        external_session(OPENCODE_SESSION, "needs-you", "crema", "/r/crema", 2),
        session(SESSION_B, "needs-you", "opusline", "/r/opusline", 3)
    ]));
    let mut app = fixture.app();
    assert_eq!(app.sessions.len(), 2);
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit attend {SESSION_B}")]
    );
}

#[test]
fn external_sessions_keep_queued_prompts_pending_without_dispatching_them() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([external_session(
        OPENCODE_SESSION,
        "waiting",
        "crema",
        "/r/crema",
        100
    )]));
    let mut app = fixture.app();
    assert!(app.sessions.is_empty());
    crate::prompts::add(OPENCODE_SESSION, "next thought").unwrap();
    app.dispatch_prompts();
    assert_eq!(crate::prompts::count(OPENCODE_SESSION), 1);
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn external_sessions_do_not_receive_automatic_todo_assignments() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([external_session(
        OPENCODE_SESSION,
        "waiting",
        "crema",
        "/r/crema",
        util::now_secs() - 600
    )]));
    let mut app = fixture.app();
    app.gaming = || false;
    assert!(app.sessions.is_empty());
    let id = app.todos.add("crema", "/r/crema", "fix the header");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    assert_ne!(app.todos.status_of(&id), "doing");
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn external_sessions_are_not_restarted_by_bulk_or_deferred_restarts() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([external_session(
        OPENCODE_SESSION,
        "waiting",
        "crema",
        "/r/crema",
        100
    )]));
    let mut app = fixture.app();
    assert!(app.sessions.is_empty());
    app.start_files = vec![("opencode plugin", 500.0)];
    app.handle(
        Key::Ctrl('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    want_restart(OPENCODE_SESSION);
    app.dispatch_prompts();
    assert!(crate::prompts::restart_wanted(OPENCODE_SESSION));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn external_sessions_refuse_todo_handoffs_without_marking_items_in_progress() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([external_session(
        OPENCODE_SESSION,
        "waiting",
        "crema",
        "/r/crema",
        100
    )]));
    let mut app = fixture.app();
    assert!(app.sessions.is_empty());
    let id = app.todos.add("crema", "/r/crema", "fix the header");
    app.build();
    let entry = app
        .todo_entries
        .iter()
        .find(|entry| entry.key() == id)
        .unwrap()
        .clone();
    app.running_sessions = sessions::reported();
    let message = app.hand_to_claude(&entry, &mut FakeScreen::default());
    assert!(message.contains("external"), "{message}");
    assert_ne!(app.todos.status_of(&id), "doing");
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn external_sessions_do_not_block_handoffs_to_a_controllable_session_of_the_same_project() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        external_session(OPENCODE_SESSION, "waiting", "crema", "/r/crema", 100),
        session(SESSION_A, "waiting", "crema", "/r/crema", 200)
    ]));
    let mut app = fixture.app();
    assert_eq!(app.sessions.len(), 1);
    let id = app.todos.add("crema", "/r/crema", "fix the header");
    app.build();
    let entry = app
        .todo_entries
        .iter()
        .find(|entry| entry.key() == id)
        .unwrap()
        .clone();
    let message = app.hand_to_claude(&entry, &mut FakeScreen::default());
    assert_eq!(message, "sent to crema");
    assert!(fixture.calls("cockpit")[0].starts_with(&format!("cockpit say {SESSION_A} ")));
    assert_eq!(app.todos.status_of(&id), "doing");
}

#[test]
fn a_restart_craft_open_asked_for_waits_for_the_end_of_the_turn() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "opusline",
        "/r/opusline",
        100
    )]));
    want_restart(SESSION_A);
    let mut app = fixture.app();
    app.dispatch_prompts();
    assert!(fixture.calls("cockpit").is_empty(), "restarted mid-turn");
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        200
    )]));
    app.build();
    app.dispatch_prompts();
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit restart {SESSION_A}  detached")]
    );
    assert!(
        !crate::prompts::restart_wanted(SESSION_A),
        "restarted twice"
    );
}

#[test]
fn a_terminal_session_is_not_restarted_from_the_cockpit() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "opusline", "/r/opusline", 100);
    row["terminal"] = json!(true);
    fixture.snapshot(json!([row]));
    want_restart(SESSION_A);
    let mut app = fixture.app();
    app.dispatch_prompts();
    assert!(
        fixture
            .calls("cockpit")
            .iter()
            .all(|call| !call.contains("restart"))
    );
}

#[test]
fn ctrl_r_leaves_a_session_started_after_the_change_alone() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 100)
    ]));
    let mut app = fixture.app();
    app.start_files = vec![("launcher", 50.0)];
    app.handle(
        Key::Ctrl('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit restart-all {SESSION_A}")]
    );
}

#[test]
fn ctrl_r_with_nothing_stale_restarts_nothing() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 100)]));
    let mut app = fixture.app();
    app.start_files = vec![("CLAUDE.md", 50.0)];
    app.handle(
        Key::Ctrl('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(app.message, "no session needs a restart");
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn enter_opens_the_session_without_waiting_and_shows_a_failure_once_it_ends() {
    // Performance 2: open took 42 ms (up to 500) with the column frozen.
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        100
    )]));
    fixture.shim(
        "cockpit",
        "sleep 0.3; echo 'cockpit: session has no tmux pane' >&2; exit 1",
    );
    let mut app = fixture.app();
    let started = std::time::Instant::now();
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "waited {:?}",
        started.elapsed()
    );
    assert!(app.message.is_empty());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app.collect_manager() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(app.message, "cockpit: session has no tmux pane");
}

#[test]
fn an_opencode_session_restarts_for_its_own_files_not_claudes() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        opencode_session("waiting", "runway", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    let mut app = fixture.app();
    let restarts = |app: &App| -> Vec<(String, Vec<&'static str>)> {
        app.sessions
            .iter()
            .map(|s| {
                (
                    s.project.clone(),
                    sessions::changed_since_start(s, &app.start_files),
                )
            })
            .collect()
    };
    app.start_files = vec![
        ("settings", 50.0),
        ("opencode plugin", 50.0),
        ("shared launcher", 50.0),
    ];
    assert_eq!(
        restarts(&app),
        vec![
            (
                "runway".to_string(),
                vec!["opencode plugin", "shared launcher"]
            ),
            ("opusline".to_string(), vec!["settings", "shared launcher"])
        ]
    );
}

// make greywall-patch replaced the binary and no card said to restart (2026-10-21): sessions kept
// the old mounts, with greyproxy's keys readable.
#[test]
fn every_agent_restarts_when_the_greywall_binary_changes() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        opencode_session("waiting", "runway", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    let mut app = fixture.app();
    app.start_files = vec![("greywall", 50.0)];
    for session in &app.sessions {
        assert_eq!(
            sessions::changed_since_start(session, &app.start_files),
            vec!["greywall"],
            "{}",
            session.project
        );
    }
    for agent in &crate::agents::AGENTS {
        assert!(
            agent.start_files.contains(&"greywall"),
            "{} does not restart for a new greywall",
            agent.name
        );
    }
}
