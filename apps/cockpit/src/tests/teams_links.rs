use super::*;

#[test]
fn team_sidebar_groups_members_without_duplicate_cards() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    let shown = texts(&app, 1).join("\n");
    for expected in ["› 1d3fd318", "▾ 1d3fd318", "Designer", "Developer"] {
        assert!(shown.contains(expected), "missing {expected}: {shown}");
    }
    for name in ["Design session", "Build session", "Independent session"] {
        assert_eq!(
            app.panels[1]
                .lines
                .iter()
                .filter(|line| line.item.is_some() && line.texts().contains(name))
                .count(),
            1,
            "{name}: {shown}"
        );
    }
    assert_eq!(app.sessions.len(), 3);
    render(&mut app, 80, 60);
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(" open ") || call.contains(" view "))
    );
}

#[test]
fn team_sidebar_hides_teams_without_available_members() {
    for has_members in [false, true] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([session(
            SESSION_B,
            "waiting",
            "project-a",
            "/r/project-a",
            3
        )]));
        let mut state = sidebar_snapshot();
        if !has_members {
            state["members"] = json!([]);
        }
        state["qa_plans"] =
            json!([{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318","agent":"claude"}]);
        state["handoffs"] = json!([{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318","handoff_id":"h1","status":"current"}]);
        team_host(&fixture, state, vec![]);
        let mut app = fixture.app();
        let shown = texts(&app, 1).join("\n");
        assert!(!shown.contains("Team:"), "{shown}");
        assert_eq!(app.sessions.len(), 1);
        open_team_view(&mut app);
        assert_eq!(
            app.team_view.as_ref().unwrap().snapshot["handoffs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn team_sidebar_keeps_identical_titles_in_separate_tasks() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut state = sidebar_snapshot();
    state["members"][1]["task_id"] = json!("0123abcd");
    team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    app.todos.items = ["1d3fd318", "0123abcd"]
        .iter()
        .map(|id| {
            Item(
                json!({"id":id,"project":"project-a","text":"Shared title","cwd":"/r/project-a"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
        })
        .collect();
    app.redraw();
    let shown = texts(&app, 1).join("\n");
    assert_eq!(
        app.panels[1]
            .lines
            .iter()
            .filter(|line| line.texts().contains("▾ Shared title"))
            .count(),
        2,
        "{shown}"
    );
}

#[test]
fn team_sidebar_hides_missing_members_but_keeps_planned_qa() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut state = sidebar_snapshot();
    state["members"][0]["session_id"] = json!("missing-designer");
    state["qa_plans"] = json!([{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318","agent":"claude"}]);
    team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    let shown = texts(&app, 1).join("\n");
    for expected in [
        "Developer",
        "▾ 1d3fd318",
        "QA",
        "Starts at the next handoff",
    ] {
        assert!(shown.contains(expected), "missing {expected}: {shown}");
    }
    assert!(
        !shown.contains("Designer")
            && !shown.contains("Unavailable")
            && !shown.contains("missing-designer"),
        "{shown}"
    );
    assert_eq!(
        app.sessions.len(),
        3,
        "a planned or unavailable member is not a live session"
    );
    open_team_view(&mut app);
    assert_eq!(
        app.team_view.as_ref().unwrap().member(0)["session_id"],
        "missing-designer"
    );
}

#[test]
fn team_sidebar_hides_completed_teams_only_when_members_are_inactive() {
    for (state, background, command, task_cwd, task_status, is_visible) in [
        ("waiting", 0, "", "/r/project-a", "done", false),
        ("working", 0, "", "/r/project-a", "done", true),
        ("needs-you", 0, "", "/r/project-a", "done", true),
        ("waiting", 1, "", "/r/project-a", "done", true),
        ("waiting", 0, "running", "/r/project-a", "done", true),
        ("waiting", 0, "", "/another/project-a", "done", true),
        ("waiting", 0, "", "/r/project-a", "doing", true),
    ] {
        let fixture = Fixture::new();
        let mut developer = session(OPENCODE_SESSION, state, "project-a", "/r/project-a", 2);
        developer["background"] = json!(background);
        developer["ask_to_run"] = json!(command);
        fixture.snapshot(json!([developer]));
        team_host(&fixture, sidebar_snapshot(), vec![]);
        let mut app = fixture.app();
        app.todos.items = vec![Item(json!({"id":"1d3fd318","project":"project-a","cwd":task_cwd,"status":task_status,"text":"Team task"}).as_object().unwrap().clone())];
        app.redraw();
        let shown = texts(&app, 1).join("\n");
        assert_eq!(
            shown.contains("› 1d3fd318"),
            is_visible,
            "{state}/{background}/{command}/{task_cwd}/{task_status}: {shown}"
        );
    }
}

#[test]
fn team_sidebar_completed_sleeping_team_keeps_unresolved_alerts_visible() {
    for has_alert in [false, true] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([]));
        asleep_note(
            &fixture,
            OPENCODE_SESSION,
            util::now_secs() - 120,
            util::now_secs() - 60,
        );
        let mut state = sidebar_snapshot();
        for member in state["members"].as_array_mut().unwrap() {
            member["project"] = json!(TEAM_PROJECT);
        }
        for session in state["sessions"].as_array_mut().unwrap() {
            session["project"] = json!(TEAM_PROJECT);
        }
        if has_alert {
            state["escalations"] = json!([{"project":TEAM_PROJECT,"task_id":"1d3fd318","thread_id":"t1","status":"open","reason":"Review blocker"}]);
        }
        team_host(&fixture, state, vec![]);
        let mut app = fixture.app();
        app.todos.items = vec![Item(json!({"id":"1d3fd318","project":"project-a","cwd":"/r/project-a","status":"done","text":"Team task"}).as_object().unwrap().clone())];
        app.redraw();
        let shown = texts(&app, 1).join("\n");
        assert_eq!(shown.contains("▾ Team task"), has_alert, "{shown}");
        if has_alert {
            assert!(shown.contains("Review blocker"), "{shown}");
            app.handle(Key::Char('n'), &mut FakeScreen::default());
            app.handle(Key::Char('i'), &mut FakeScreen::default());
            assert_eq!(app.focus, 5, "i on a team opens the Teams tab");
            assert!(
                texts(&app, 5)
                    .join("\n")
                    .contains("! Escalated: Review blocker"),
                "{:?}",
                texts(&app, 5)
            );
        }
        assert!(fixture.calls("cockpit").is_empty());
    }
}

#[test]
fn team_sidebar_omits_completed_inactive_members_but_keeps_active_teams() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(OPENCODE_SESSION, "waiting", "project-a", "/r/project-a", 2),
        session(SESSION_A, "waiting", "project-a", "/r/project-a", 1)
    ]));
    let mut state = sidebar_snapshot();
    state["members"][0]["task_id"] = json!("0123abcd");
    team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    app.todos.items = vec![Item(json!({"id":"1d3fd318","project":"project-a","cwd":"/r/project-a","status":"done","text":"Completed team"}).as_object().unwrap().clone())];
    app.redraw();
    sidebar_select(&mut app, "› 0123abcd");
    app.redraw();
    let choices = texts(&app, 1).join("\n");
    assert!(!choices.contains("Developer"), "{choices}");
    assert!(choices.contains("Designer"), "{choices}");
}

#[test]
fn team_sidebar_hiding_a_completed_team_cannot_retarget_session_actions() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(OPENCODE_SESSION, "waiting", "project-a", "/r/project-a", 2),
        session(SESSION_B, "waiting", "project-a", "/r/project-a", 3)
    ]));
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_select(&mut app, "Developer");
    app.todos.items = vec![Item(json!({"id":"1d3fd318","project":"project-a","cwd":"/r/project-a","status":"done","text":"Team task"}).as_object().unwrap().clone())];
    app.redraw();
    assert!(app.selected_session().is_none());
    for key in [Key::Enter, Key::Char('x'), Key::Char('U')] {
        app.handle(key, &mut FakeScreen::answering(&[Key::Char('y')]));
    }
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn team_sidebar_reports_unavailable_links_without_hiding_sessions() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut app = fixture.app();
    let shown = texts(&app, 1).join("\n");
    assert!(shown.contains("Team links unavailable"), "{shown}");
    assert!(
        !shown.contains("Independent sessions"),
        "unknown membership isn't independent"
    );
    assert_eq!(app.sessions.len(), 3);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    app.team_checked = 0.0;
    app.check_team();
    app.redraw();
    assert!(texts(&app, 1).join("\n").contains("› 1d3fd318"));
    fs::remove_file(crate::paths::team_host_socket()).unwrap();
    app.team_checked = 0.0;
    app.check_team();
    app.redraw();
    let shown = texts(&app, 1).join("\n");
    assert!(
        shown.contains("Team links unavailable") && !shown.contains("› 1d3fd318"),
        "{shown}"
    );
}

#[test]
fn team_sidebar_does_not_infer_activity_or_completion() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut state = sidebar_snapshot();
    state["handoffs"] = json!([{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318","handoff_id":"h1","status":"current","created_at":1,"payload":{"commit":"abc","tests":"all passed"}}]);
    team_host(&fixture, state, vec![]);
    let app = fixture.app();
    let shown = texts(&app, 1).join("\n");
    assert!(shown.contains("Handoff submitted"), "{shown}");
    for forbidden in [
        "QA approved",
        "handoff complete",
        "deployed",
        "Waiting for developer",
    ] {
        assert!(!shown.contains(forbidden), "{shown}");
    }
}

#[test]
fn team_sidebar_title_uses_task_entry_or_exact_id() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    app.todos.items = vec![Item(json!({"id":"1d3fd318","project":"project-a","cwd":"/another/project-a","text":"Wrong project title"}).as_object().unwrap().clone())];
    app.redraw();
    assert!(texts(&app, 1).join("\n").contains("› 1d3fd318"));
    app.todos.items[0]
        .0
        .insert("cwd".into(), json!("/r/project-a"));
    app.redraw();
    assert!(texts(&app, 1).join("\n").contains("▾ Wrong project title"));
}

#[test]
fn team_sidebar_attention_group_keeps_team_context() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut state = sidebar_snapshot();
    state["escalations"] = json!([{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318","thread_id":"t1","status":"open","reason":"Need Theo to choose frame"}]);
    team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    app.redraw();
    let shown = texts(&app, 1).join("\n");
    for expected in [
        "▾ 1d3fd318",
        "Design session",
        "Build session",
        "Need Theo to choose frame",
    ] {
        assert!(shown.contains(expected), "{shown}");
    }
    assert!(
        shown.contains("Independent session") && !shown.contains("Unavailable"),
        "{shown}"
    );
}

#[test]
fn team_sidebar_selected_details_keep_full_member_identity_and_handoff() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    app.panels[1].selected = app.panels[1]
        .lines
        .iter()
        .find(|line| line.texts().contains("Build session"))
        .unwrap()
        .item
        .unwrap();
    render(&mut app, 40, 60);
    let details = app.panels[1]
        .fixed_bottom
        .iter()
        .map(|line| line.texts())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        OPENCODE_SESSION,
        "opencode",
        "Developer",
        "No handoff recorded",
        "Build session",
    ] {
        assert!(details.contains(expected), "missing {expected}: {details}");
    }
}

#[test]
fn team_sidebar_enter_toggles_without_opening_or_waking() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_fold(&mut app);
    assert!(
        texts(&app, 1)
            .iter()
            .any(|text| text.contains("▸ 1d3fd318"))
    );
    assert!(
        !app.panels[1]
            .lines
            .iter()
            .any(|line| line.texts().contains("Build session"))
    );
    assert!(app.selected_session().is_none());
    app.handle(Key::Enter, &mut FakeScreen::default());
    app.redraw();
    assert!(
        app.panels[1]
            .lines
            .iter()
            .any(|line| line.texts().contains("Build session"))
    );
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
    sidebar_select(&mut app, "Build session");
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit open {OPENCODE_SESSION}")]
    );
    app.handle(Key::Right, &mut FakeScreen::default());
    assert!(
        fixture
            .calls("cockpit")
            .contains(&"cockpit focus-viewer".into())
    );
}

#[test]
fn team_sidebar_next_attention_expands_the_correct_team() {
    let fixture = Fixture::new();
    let mut developer = session(
        OPENCODE_SESSION,
        "needs-you",
        "project-a",
        "/r/project-a",
        2,
    );
    developer["request"] = json!("Which test should run?");
    developer["label"] = json!("Build session");
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "project-a", "/r/project-a", 1),
        developer
    ]));
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_fold(&mut app);
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    app.redraw();
    assert_eq!(app.selected_session().unwrap().session, OPENCODE_SESSION);
    assert!(
        texts(&app, 1)
            .iter()
            .any(|text| text.contains("▾ 1d3fd318"))
    );
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit attend {OPENCODE_SESSION}")]
    );
}

#[test]
fn team_sidebar_expansion_reaches_a_collapsed_member() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_fold(&mut app);
    app.handle(Key::Enter, &mut FakeScreen::default());
    app.redraw();
    sidebar_select(&mut app, "Build session");
    assert_eq!(app.selected_session().unwrap().session, OPENCODE_SESSION);
    assert!(
        texts(&app, 1)
            .iter()
            .any(|text| text.contains("▾ 1d3fd318"))
    );
    assert!(fixture.calls("cockpit").is_empty());
    app.handle(Key::Char('U'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        ["cockpit push /r/project-a"]
    );
}

#[test]
fn team_sidebar_grouped_navigation_acts_only_on_the_selected_member() {
    for (key, expected) in [
        (Key::Enter, format!("cockpit open {SESSION_A}")),
        (Key::Char('x'), format!("cockpit exit {SESSION_A}")),
        (Key::Char('U'), "cockpit push /r/design".into()),
    ] {
        let fixture = Fixture::new();
        let mut designer = session(SESSION_A, "waiting", "project-a", "/r/design", 1);
        designer["label"] = json!("Design session");
        let mut developer = session(OPENCODE_SESSION, "needs-you", "project-a", "/r/build", 2);
        developer["label"] = json!("Build session");
        developer["request"] = json!("Choose frame");
        fixture.snapshot(json!([designer, developer]));
        let mut state = sidebar_snapshot();
        state["members"][1]["task_id"] = json!("0123abcd");
        team_host(&fixture, state, vec![]);
        let mut app = fixture.app();
        sidebar_select(&mut app, "Developer: Choose frame");
        sidebar_select(&mut app, "Design session");
        app.redraw();
        assert_eq!(
            app.selected_session().map(|session| session.session),
            Some(SESSION_A.into())
        );
        assert!(
            app.panels[1]
                .lines
                .iter()
                .any(|line| line.texts().contains("Design session"))
        );
        app.handle(key, &mut FakeScreen::answering(&[Key::Char('y')]));
        assert_eq!(fixture.wait_for_calls("cockpit", 1), [expected]);
    }
}

#[test]
fn team_sidebar_disappeared_member_cannot_reuse_previous_actions() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_select(&mut app, "Build session");
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "project-a",
        "/r/project-a",
        1
    )]));
    app.build();
    assert!(app.selected_session().is_none());
    for key in [Key::Enter, Key::Char('x'), Key::Char('U')] {
        app.handle(key, &mut FakeScreen::answering(&[Key::Char('y')]));
    }
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn team_sidebar_old_attention_request_shows_state_age() {
    let fixture = Fixture::new();
    let mut developer = session(
        OPENCODE_SESSION,
        "needs-you",
        "project-a",
        "/r/project-a",
        util::now_secs() - 7200,
    );
    developer["request"] = json!("Choose old frame");
    fixture.snapshot(json!([developer]));
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let app = fixture.app();
    assert!(
        app.panels[1]
            .lines
            .iter()
            .any(|line| line.texts().contains("Choose old frame"))
    );
    let member = app.panels[1]
        .lines
        .iter()
        .find(|line| line.texts().starts_with("!Developer"))
        .unwrap();
    let status: String = member.right.iter().map(|part| part.text.as_str()).collect();
    assert!(status.ends_with("2h"), "{status}");
}

#[test]
fn team_sidebar_failed_delivery_can_be_cancelled_from_task_attention() {
    let fixture = Fixture::new();
    let mut state = sidebar_snapshot();
    state["members"] = json!([]);
    state["messages"] = json!([{"message_id":"failed-m1","state":"failed","error":"native inbox unavailable",
        "envelope":json!({"task":{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318"},"kind":"notice","body":"Review frame","sender":{"role":"developer"},"recipient":{"role":"qa"}}).to_string()}]);
    let requests = team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    app.redraw();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    assert!(
        texts(&app, 5).join("\n").contains("! Message to QA failed"),
        "{:?}",
        texts(&app, 5)
    );
    app.handle(Key::Enter, &mut FakeScreen::default());
    app.redraw();
    app.handle(
        Key::Char('y'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert!(
        controls(&requests).is_empty(),
        "failed delivery cannot be marked received"
    );
    app.handle(
        Key::Char('c'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert!(
        controls(&requests).is_empty(),
        "cancellation needs confirmation"
    );
    app.handle(
        Key::Char('c'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let actions = controls(&requests);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["action"], "reconcile");
    assert_eq!(actions[0]["message_id"], "failed-m1");
    assert_eq!(actions[0]["decision"], "cancel");
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn team_sidebar_refresh_preserves_selected_session_identity() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_select(&mut app, "Build session");
    app.handle(Key::Char('p'), &mut FakeScreen::default());
    app.build();
    assert_eq!(app.selected_session().unwrap().session, OPENCODE_SESSION);
    sidebar_select(&mut app, "Build session");
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "project-a", "/r/project-a", 1),
        session(SESSION_B, "waiting", "project-a", "/r/project-a", 3)
    ]));
    app.build();
    assert!(
        app.selected_session().is_none(),
        "disappeared member inherited another session target"
    );
    for key in [Key::Enter, Key::Char('x'), Key::Char('U')] {
        app.handle(key, &mut FakeScreen::answering(&[Key::Char('y')]));
    }
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn team_sidebar_non_session_rows_cannot_run_session_actions() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut state = sidebar_snapshot();
    state["members"][0]["session_id"] = json!("missing-designer");
    state["qa_plans"] = json!([{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318","agent":"claude"}]);
    team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    for target in ["› 1d3fd318", "Starts at the next handoff"] {
        sidebar_select(&mut app, target);
        for key in [
            Key::Char('x'),
            Key::Char('r'),
            Key::Char('s'),
            Key::Char('U'),
            Key::Char('e'),
            Key::Char('$'),
            Key::Char('C'),
            Key::Char('+'),
        ] {
            app.handle(key, &mut FakeScreen::answering(&[Key::Char('y')]));
        }
    }
    assert!(fixture.calls("cockpit").is_empty());
    assert!(app.menu.is_none());
    render(&mut app, 80, 60);
    let rows = buffer_rows(&render(&mut app, 80, 60));
    let heading_y = rows
        .iter()
        .position(|row| row.contains("› 1d3fd318"))
        .unwrap() as u16;
    app.handle(
        Key::Click {
            x: 8,
            y: heading_y,
            double: true,
        },
        &mut FakeScreen::default(),
    );
    app.redraw();
    assert!(
        texts(&app, 1)
            .iter()
            .any(|text| text.contains("▸ 1d3fd318"))
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn team_sidebar_next_attention_reaches_task_only_blockers() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut state = sidebar_snapshot();
    state["escalations"] = json!([{"project":SIDEBAR_PROJECT,"task_id":"1d3fd318","thread_id":"t1","status":"open","reason":"Choose the frame"}]);
    team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    sidebar_fold(&mut app);
    sidebar_select(&mut app, "Independent session");
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    app.redraw();
    assert!(app.selected_session().is_none());
    assert!(
        texts(&app, 1)
            .iter()
            .any(|text| text.contains("▾ 1d3fd318"))
    );
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    assert_eq!(
        app.focus, 5,
        "task attention must be actionable in the Teams tab"
    );
    assert!(
        texts(&app, 5)
            .join("\n")
            .contains("Escalated: Choose the frame"),
        "{:?}",
        texts(&app, 5)
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn team_sidebar_failed_wake_stays_reachable_when_collapsed() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "project-a",
        "/r/project-a",
        1
    )]));
    fixture.write(&format!("state/cockpit/asleep/{OPENCODE_SESSION}"),&json!({
            "agent":"opencode","project":"project-a","cwd":"/r/project-a","title":"Sleeping developer",
            "idle_since":util::now_secs()-7200,"slept_at":util::now_secs()-3600,"remote":false,"started":1
        }).to_string());
    let mut state = sidebar_snapshot();
    state["sleeps"] =
        json!([{"session_id":OPENCODE_SESSION,"state":"failed","error":"launcher refused"}]);
    team_host(&fixture, state, vec![]);
    let mut app = fixture.app();
    sidebar_fold(&mut app);
    let shown = texts(&app, 1).join("\n");
    assert!(
        shown.contains("▸ 1d3fd318")
            && shown.contains("› 1d3fd318")
            && shown.contains("Developer wake failed: launcher refused"),
        "{shown}"
    );
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    app.redraw();
    assert!(app.selected_session().is_none());
    assert!(
        texts(&app, 1)
            .iter()
            .any(|line| line.contains("▾ 1d3fd318"))
    );
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    assert_eq!(app.focus, 5);
    let details = texts(&app, 5).join("\n");
    assert!(
        details.contains("! Developer did not wake") && details.contains("launcher refused"),
        "{details}"
    );
    assert!(
        fixture.calls("cockpit").is_empty(),
        "no wake/open/attend: {:?}",
        fixture.calls("cockpit")
    );
}

#[test]
fn team_sidebar_unicode_and_small_buffers_preserve_attention() {
    let fixture = Fixture::new();
    let mut developer = session(
        OPENCODE_SESSION,
        "needs-you",
        "project-a",
        "/r/project-a",
        2,
    );
    developer["label"] = json!(format!("Review 界{}\u{1b}[2J", "界".repeat(50)));
    developer["request"] = json!(format!("Choose frame {}\u{1b}[2J", "界".repeat(50)));
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "project-a", "/r/project-a", 1),
        developer
    ]));
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_select(&mut app, "Developer");
    for width in [80, 40, 24, 8, 1] {
        let buffer = render(&mut app, width, 60);
        let rows = buffer_rows(&buffer).join("\n");
        assert!(!rows.contains('\u{1b}'), "{rows:?}");
        if width >= 40 {
            assert!(
                rows.contains("NEEDS YOU") && rows.contains("Choose frame"),
                "{rows}"
            );
            assert!(rows.contains('…'), "{rows}");
            let selected = (0..60)
                .find(|y| buffer_rows(&buffer)[*y].contains("Developer"))
                .unwrap() as u16;
            let cell = &buffer[(8, selected)];
            assert_eq!(cell.fg, ratatui::style::Color::Indexed(crate::app::FG));
            assert!(!cell.modifier.contains(ratatui::style::Modifier::DIM));
        }
    }
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    let details = buffer_rows(&render(&mut app, 120, 60)).join("\n");
    assert!(!details.contains('\u{1b}'), "{details:?}");
    assert!(details.contains(OPENCODE_SESSION), "{details}");
}

#[test]
fn team_sidebar_unlink_refresh_cannot_target_an_unrelated_session() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    team_host(&fixture, sidebar_snapshot(), vec![]);
    let mut app = fixture.app();
    sidebar_select(&mut app, "Build session");
    fs::remove_file(crate::paths::team_host_socket()).unwrap();
    app.team_checked = 0.0;
    app.check_team();
    fixture.snapshot(json!([session(
        SESSION_B,
        "waiting",
        "project-a",
        "/r/project-a",
        3
    )]));
    app.build();
    for _ in 0..2 {
        for key in [Key::Enter, Key::Char('x'), Key::Char('U')] {
            app.handle(key, &mut FakeScreen::answering(&[Key::Char('y')]));
        }
        app.redraw();
    }
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
    app.handle(Key::Down, &mut FakeScreen::default());
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit open {SESSION_B}")]
    );
}

#[test]
fn team_sidebar_fold_retains_sleeping_and_restarting_members_without_side_effects() {
    for state in ["asleep", "restarting"] {
        let fixture = Fixture::new();
        sidebar_sessions(&fixture);
        team_host(&fixture, sidebar_snapshot(), vec![]);
        let mut app = fixture.app();
        sidebar_select(&mut app, "Build session");
        if state == "asleep" {
            asleep_note(
                &fixture,
                OPENCODE_SESSION,
                util::now_secs() - 120,
                util::now_secs() - 60,
            );
        } else {
            note_restart(
                &fixture,
                OPENCODE_SESSION,
                util::now_secs(),
                "project-a",
                "/r/project-a",
            );
        }
        fixture.snapshot(json!([session(
            SESSION_A,
            "waiting",
            "project-a",
            "/r/project-a",
            1
        )]));
        app.build();
        assert_eq!(app.selected_session().unwrap().session, OPENCODE_SESSION);
        sidebar_fold(&mut app);
        app.handle(Key::Enter, &mut FakeScreen::default());
        app.redraw();
        assert!(
            texts_with_badges(&app, 1)
                .iter()
                .any(|text| text.contains(if state == "asleep" {
                    "Asleep"
                } else {
                    "Restarting"
                })),
            "{:?}",
            texts_with_badges(&app, 1)
        );
        assert!(
            fixture.calls("cockpit").is_empty(),
            "{state}: {:?}",
            fixture.calls("cockpit")
        );
    }
}

#[test]
fn link_ui_approval_confirms_the_exact_mapping_and_keeps_navigation() {
    let fixture = Fixture::new();
    let (live, task, _) = link_ui_host(&fixture);
    fixture.snapshot(json!([
        session(
            SESSION_A,
            "waiting",
            "first",
            live[0].project.trim_start_matches("path:"),
            1
        ),
        session(
            OPENCODE_SESSION,
            "waiting",
            "second",
            live[1].project.trim_start_matches("path:"),
            2
        )
    ]));
    let mut app = fixture.app();
    app.focus_panel(1);
    let focus = app.focus;
    let selected = app.selected_session().unwrap().session.clone();
    open_team_view(&mut app);
    let shown = buffer_rows(&render(&mut app, 160, 55)).join("\n");
    assert!(
        shown.contains("Claude unknown") && shown.contains("Codex unknown"),
        "{shown}"
    );
    for expected in [
        SESSION_A,
        OPENCODE_SESSION,
        &live[0].project,
        &live[1].project,
        &task.task_id,
        "first-runtime",
        "messaging-only",
        "Coordinate builds",
    ] {
        assert!(shown.contains(expected), "missing {expected}: {shown}");
    }
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM links", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert!(app.message.contains("link approved"), "{}", app.message);
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM links WHERE state='active'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM members", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert_eq!(app.focus, focus);
    assert_eq!(app.selected_session().unwrap().session, selected);
    assert!(app.team_view.is_none());
}

#[test]
fn link_ui_changed_revision_is_refused_without_rebinding_or_approval() {
    let fixture = Fixture::new();
    let (live, task, pending) = link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    let mut connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    crate::teams::store::control(
        &mut connection,
        &crate::teams::HostAction::LinkEdit {
            request_id: "other-edit".into(),
            proposal_id: pending["proposal_id"].as_str().unwrap().into(),
            expected_revision: 1,
            replacement: crate::teams::LinkProposal {
                submission_key: "other".into(),
                task,
                peer_session_id: live[1].session_id.clone(),
                purpose: "Changed purpose".into(),
            },
        },
        &live,
        1,
    )
    .unwrap();
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert!(app.message.contains("changed"), "{}", app.message);
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM links", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        app.team_view
            .as_ref()
            .unwrap()
            .detail()
            .join("\n")
            .contains("Changed purpose")
    );
}

#[test]
fn link_ui_edits_require_a_fresh_confirmation() {
    let fixture = Fixture::new();
    let (live, task, _) = link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('e'),
        &mut FakeScreen::typing(&[
            &format!("{}\tpeer", live[1].session_id),
            &format!("{}\towner", task.project),
            &task.task_id,
            "New purpose",
        ]),
    );
    assert!(app.message.contains("new revision"), "{}", app.message);
    let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM links", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let detail = app.team_view.as_ref().unwrap().detail().join("\n");
    assert!(
        detail.contains("Revision 2") && detail.contains("New purpose"),
        "{detail}"
    );
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM links", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        connection
            .query_row("SELECT revision FROM links", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn link_ui_unreadable_todo_never_reports_successful_approval() {
    let fixture = Fixture::new();
    link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    fs::write(crate::paths::todo(), "not json").unwrap();
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert!(
        !app.message.contains("link approved") && !app.message.is_empty(),
        "{}",
        app.message
    );
    let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM links", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        app.team_view
            .as_ref()
            .unwrap()
            .detail()
            .join("\n")
            .contains("Blocked")
    );
}

#[test]
fn link_ui_revoke_requires_yes_cancels_queued_messages_and_retains_history() {
    let fixture = Fixture::new();
    let (live, _, _) = link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let (link_id, message_id) = queue_link_note(&live);
    app.handle(
        Key::Char('u'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT state FROM links WHERE id=?1", [&link_id], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "active"
    );
    app.handle(
        Key::Char('u'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT state FROM messages WHERE id=?1",
                [message_id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "cancelled"
    );
    app.handle(Key::Tab, &mut FakeScreen::default());
    let shown = buffer_rows(&render(&mut app, 160, 55)).join("\n");
    assert!(
        shown.contains(&link_id) && shown.contains("Closed/rejected: revoked"),
        "{shown}"
    );
    assert!(
        !app.team_view
            .as_ref()
            .unwrap()
            .detail()
            .join("\n")
            .contains("u revoke")
    );
}

#[test]
fn link_ui_todo_done_closes_link_and_reopening_does_not_restore_it() {
    let fixture = Fixture::new();
    let (live, task, _) = link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let (link_id, message_id) = queue_link_note(&live);
    app.handle(Key::Esc, &mut FakeScreen::default());
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('d'), &mut FakeScreen::default());
    assert_eq!(app.todos.status_of(&task.task_id), "done");
    assert!(app.todos.set_status(&task.task_id, "todo", ""));
    open_team_view(&mut app);
    assert!(app.team_view.as_ref().unwrap().rows().is_empty());
    app.handle(Key::Tab, &mut FakeScreen::default());
    let shown = buffer_rows(&render(&mut app, 160, 55)).join("\n");
    assert!(
        shown.contains(&link_id) && shown.contains("Closed/rejected: done"),
        "{shown}"
    );
    let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT state FROM messages WHERE id=?1",
                [message_id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "cancelled"
    );
}

#[test]
fn link_ui_uncertain_submission_is_reconciled_never_resent_even_after_revoke() {
    for decision in ['h', 'c'] {
        let fixture = Fixture::new();
        let (live, _, _) = link_ui_host(&fixture);
        let mut app = fixture.app();
        open_team_view(&mut app);
        app.handle(
            Key::Char('a'),
            &mut FakeScreen::answering(&[Key::Char('y')]),
        );
        let (_, message_id) = queue_link_note(&live);
        let mut connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
        let claimed =
            crate::teams::store::claim(&mut connection, &live[1], "ui-idle-turn", &live, 200_000)
                .unwrap()
                .expect("claim queued note");
        crate::teams::store::finish_for(
            &mut connection,
            &live[1],
            &claimed.id,
            &crate::teams::TransportOutcome::Uncertain {
                reason: "lost acknowledgement".into(),
            },
            200_001,
        )
        .unwrap();
        app.handle(
            Key::Char('u'),
            &mut FakeScreen::answering(&[Key::Char('y')]),
        );
        let shown = buffer_rows(&render(&mut app, 160, 55)).join("\n");
        assert!(
            shown.contains("uncertain") && shown.contains(&message_id),
            "{shown}"
        );
        for key in [Key::Char('a'), Key::Char('e'), Key::Char('r')] {
            app.handle(key, &mut FakeScreen::default());
        }
        app.handle(
            Key::Char(decision),
            &mut FakeScreen::answering(&[Key::Char('y')]),
        );
        let (state, attempts): (String, i64) = connection
            .query_row(
                "SELECT state,attempts FROM messages WHERE id=?1",
                [&message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            if decision == 'h' {
                "received"
            } else {
                "cancelled"
            }
        );
        assert_eq!(attempts, 1);
        assert!(
            crate::teams::store::claim(&mut connection, &live[1], "ui-next-turn", &live, 300_000)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn link_ui_host_creation_and_rejection_grant_no_team_role_or_messaging() {
    let fixture = Fixture::new();
    let (live, task, _) = link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(Key::Tab, &mut FakeScreen::default());
    app.handle(
        Key::Char('l'),
        &mut FakeScreen::typing(&[
            &format!("{}\tfirst", live[0].session_id),
            &format!("{}\tpeer", live[1].session_id),
            &format!("{}\towner", task.project),
            &task.task_id,
            "Host requested work",
        ]),
    );
    assert!(
        app.message.contains("link request created"),
        "{}",
        app.message
    );
    assert!(
        app.team_view
            .as_ref()
            .unwrap()
            .detail()
            .join("\n")
            .contains("Host requested work")
    );
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM link_proposals WHERE status='pending'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    app.handle(
        Key::Char('x'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM link_proposals WHERE status='rejected'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    for table in ["links", "members"] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn link_ui_refresh_failure_is_visible_instead_of_silently_showing_old_status() {
    let fixture = Fixture::new();
    link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    fs::remove_file(fixture.path("run/claude-team-host/host.sock")).unwrap();
    app.handle(Key::Char('r'), &mut FakeScreen::default());
    let shown = buffer_rows(&render(&mut app, 160, 55)).join("\n");
    assert!(
        shown.contains("unavailable") && shown.contains("stale"),
        "{shown}"
    );
}

#[test]
fn link_ui_narrow_screen_can_scroll_the_entire_exact_request_without_losing_selection() {
    let fixture = Fixture::new();
    let (_, task, _) = link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    let mut all_text = String::new();
    for _ in 0..80 {
        all_text.push_str(&buffer_rows(&render(&mut app, 40, 16)).join("\n"));
        app.handle(Key::Right, &mut FakeScreen::default());
    }
    for expected in [
        &task.task_id,
        "first-runtime",
        "second-runtime",
        "Coordinate builds",
        "a approve exactly this",
    ] {
        assert!(
            all_text.contains(expected),
            "missing {expected}: {all_text}"
        );
    }
    assert_eq!(app.team_view.as_ref().unwrap().selected, 0);
    app.handle(Key::Down, &mut FakeScreen::default());
    assert_eq!(app.team_view.as_ref().unwrap().detail_scroll, 0);
}

#[test]
fn link_ui_never_accepts_free_form_or_duplicate_participants_from_the_picker() {
    for peer in ["invented-session", SESSION_A] {
        let fixture = Fixture::new();
        let (live, _, _) = link_ui_host(&fixture);
        let mut app = fixture.app();
        open_team_view(&mut app);
        app.handle(
            Key::Char('l'),
            &mut FakeScreen::typing(&[
                &format!("{}\tfirst", live[0].session_id),
                &format!("{peer}\tpeer"),
            ]),
        );
        assert!(
            app.message.contains("verified live session"),
            "{}",
            app.message
        );
        let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM link_proposals", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn link_ui_ambiguous_host_reply_is_not_retried_or_reported_as_approval() {
    let fixture = Fixture::new();
    link_ui_host(&fixture);
    let mut app = fixture.app();
    open_team_view(&mut app);
    let snapshot = app.team_view.as_ref().unwrap().snapshot.clone();
    fs::remove_file(fixture.path("run/claude-team-host/host.sock")).unwrap();
    let requests = team_host(
        &fixture,
        snapshot,
        vec![json!({"Err":{"code":"unavailable","message":"reply lost after submission"}})],
    );
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert!(
        app.message.contains("unknown") && !app.message.contains("link approved"),
        "{}",
        app.message
    );
    app.handle(Key::Char('r'), &mut FakeScreen::default());
    assert_eq!(controls(&requests).len(), 1);
}

#[test]
fn a_failed_team_start_is_dismissed_after_a_yes() {
    // Todo 9397cffb.
    let fixture = Fixture::new();
    let mut snapshot = team_snapshot();
    snapshot["proposals"] = json!([]);
    snapshot["members"] = json!([]);
    snapshot["spawns"] = json!([{"token": "t1", "task_key": "k", "role": "designer", "agent": "claude", "status": "refused", "directory": "/p", "created_at": 1}]);
    let requests = team_host(&fixture, snapshot, vec![]);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('d'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert!(controls(&requests).is_empty());
    app.handle(
        Key::Char('d'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let dismiss = controls(&requests).pop().expect("no dismiss");
    assert_eq!(
        (dismiss["action"].clone(), dismiss["token"].clone()),
        (json!("dismiss"), json!("t1"))
    );
}

#[test]
fn approving_sends_exactly_the_displayed_revision_and_mapping() {
    let fixture = Fixture::new();
    let requests = team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(
        controls(&requests),
        [
            json!({"action": "approve", "request_id": controls(&requests)[0]["request_id"],
        "proposal_id": "p1", "expected_revision": 2, "mapping_hash": "h1"})
        ]
    );
}

#[test]
fn declining_the_confirmation_approves_nothing() {
    let fixture = Fixture::new();
    let requests = team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert!(controls(&requests).is_empty());
}

#[test]
fn after_a_channel_restart_the_same_mapping_is_rebound_for_a_fresh_review() {
    let fixture = Fixture::new();
    let changed = json!({"Err": {"code": "conflict", "message": "a requested session runtime changed; request fresh approval"}});
    let requests = team_host(&fixture, team_snapshot(), vec![changed]);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('a'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let edit = controls(&requests).pop().unwrap();
    assert_eq!(edit["action"], "edit");
    assert_eq!(edit["proposal_id"], "p1");
    assert_eq!(
        edit["replacement"]["task"],
        json!({"project": TEAM_PROJECT, "task_id": "1d3fd318"})
    );
    assert_eq!(
        edit["replacement"]["members"],
        json!([{"session_id": SESSION_A, "role": "designer"}, {"session_id": OPENCODE_SESSION, "role": "developer"}])
    );
    assert_eq!(
        controls(&requests)
            .iter()
            .filter(|action| action["action"] == "approve")
            .count(),
        1,
        "the rebound request needs its own approval"
    );
}

#[test]
fn unlinking_a_member_takes_a_yes_and_names_its_task() {
    let fixture = Fixture::new();
    let requests = team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(Key::Char('j'), &mut FakeScreen::default());
    app.handle(
        Key::Char('u'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let unlink = controls(&requests).pop().unwrap();
    assert_eq!(
        (
            unlink["action"].clone(),
            unlink["session_id"].clone(),
            unlink["task"].clone()
        ),
        (
            json!("unlink"),
            json!(SESSION_B),
            json!({"project": TEAM_PROJECT, "task_id": "0123abcd"})
        )
    );
}

#[test]
fn theo_creates_a_team_from_live_sessions_and_still_approves_it_separately() {
    let fixture = Fixture::new();
    let requests = team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    open_team_view(&mut app);
    app.handle(
        Key::Char('n'),
        &mut FakeScreen::typing(&[
            "1d3fd318",
            &format!("{SESSION_A}\tx"),
            &format!("{OPENCODE_SESSION}\tx"),
            "-\t(no one)",
        ]),
    );
    let propose = controls(&requests).pop().unwrap();
    assert_eq!(propose["action"], "propose");
    assert_eq!(
        propose["proposal"]["task"],
        json!({"project": TEAM_PROJECT, "task_id": "1d3fd318"})
    );
    assert_eq!(
        propose["proposal"]["members"],
        json!([{"session_id": SESSION_A, "role": "designer"}, {"session_id": OPENCODE_SESSION, "role": "developer"}])
    );
    assert!(
        !controls(&requests)
            .iter()
            .any(|action| action["action"] == "approve")
    );
}

#[test]
fn a_missing_team_service_is_said_and_nothing_opens() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    fs::remove_file(crate::paths::team_host_socket()).unwrap();
    {
        app.focus_panel(1);
        app.handle(Key::Char('k'), &mut FakeScreen::default());
    }
    assert!(app.team_view.is_none());
    assert!(app.message.contains("unavailable"), "{}", app.message);
}

#[test]
fn the_team_view_draws_full_ids_and_closes_with_escape() {
    let fixture = Fixture::new();
    team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    open_team_view(&mut app);
    let buffer = render(&mut app, 120, 40);
    let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(
        text.contains(&format!("designer: {SESSION_A}")),
        "full id missing"
    );
    let _ = render(&mut app, 30, 12);
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(app.team_view.is_none());
}

#[test]
fn the_sessions_summary_says_when_the_team_needs_theo() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_B, "waiting", "project-a", "/p", 1)]));
    team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    app.tick();
    let summary: Vec<String> = app.panels[1]
        .fixed_top
        .iter()
        .map(|line| line.texts())
        .collect();
    assert!(
        summary
            .iter()
            .any(|line| line.contains("team: 1 needs you")),
        "{summary:?}"
    );
}

#[test]
fn a_linked_session_shows_its_team_role() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_B, "waiting", "project-a", "/p", 1)]));
    team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    app.tick();
    assert!(
        selected_session_badges(&app)
            .iter()
            .any(|badge| badge == "team qa · 0123abcd"),
        "{:?}",
        selected_session_badges(&app)
    );
}

#[test]
fn the_team_view_has_one_background_behind_its_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.team_view = Some(crate::teams::view::TeamView::new(json!({})));
    assert_one_background(
        &render(&mut app, 80, 30),
        2..28,
        2..78,
        &[DESIGNED_HIGHLIGHT],
    );
}
