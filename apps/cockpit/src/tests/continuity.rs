use super::*;

#[test]
fn continuity_cancel_preserves_a_note_and_clear_explicitly_removes_it() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let mut app = fixture.app();
    write_note(&mut app, "Keep me");
    write_note(&mut app, "");
    assert!(texts(&app, 1).join("\n").contains("Keep me"));
    write_note(&mut app, "-");
    assert!(!texts(&fixture.app(), 1).join("\n").contains("Keep me"));
}

#[test]
fn continuity_note_validation_preserves_the_previous_note() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let mut app = fixture.app();
    let accepted = "é".repeat(280);
    write_note(&mut app, &accepted);
    for rejected in [
        "é".repeat(281),
        "bad\u{1b}note".into(),
        "bad\nnote".into(),
        "#{pane}".into(),
        "#(run)".into(),
    ] {
        write_note(&mut app, &rejected);
        assert!(
            app.message.contains("invalid return note"),
            "{}",
            app.message
        );
    }
    let saved: Value = serde_json::from_str(
        &fs::read_to_string(fixture.path("state/cockpit/continuity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["sessions"][SESSION_A]["note"], accepted);
}

#[test]
fn continuity_invalid_state_is_preserved_and_warns_after_other_keys() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    for contents in ["not json", "{\"version\":2}", "{}"] {
        let path = fixture.write("state/cockpit/continuity.json", contents);
        let mut app = fixture.app();
        write_note(&mut app, "Do not overwrite");
        app.handle(Key::Down, &mut FakeScreen::default());
        let rows = buffer_rows(&render(&mut app, 100, 40)).join("\n");
        assert!(rows.contains("Continuity unavailable"), "{rows}");
        assert_eq!(fs::read_to_string(path).unwrap(), contents);
    }
}

#[test]
fn continuity_symlink_and_oversized_state_are_not_overwritten() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let target = fixture.write("private.json", "{\"version\":1}");
    let path = fixture.path("state/cockpit/continuity.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let mut app = fixture.app();
    write_note(&mut app, "Must fail");
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "{\"version\":1}");
    assert!(app.message.contains("Continuity unavailable"));
    fs::remove_file(&path).unwrap();
    fs::write(&path, " ".repeat(16 * 1024 * 1024 + 1)).unwrap();
    let mut app = fixture.app();
    write_note(&mut app, "Must fail");
    assert!(app.message.contains("Continuity unavailable"));
    assert_eq!(fs::metadata(path).unwrap().len(), 16 * 1024 * 1024 + 1);
}

#[test]
fn continuity_notes_preserve_unknown_state_and_are_not_written_during_render() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let path = fixture.write(
        "state/cockpit/continuity.json",
        &json!({"version":1,"sessions":{},"tasks":{},"prs":{},"future":{"keep":true}}).to_string(),
    );
    let mut app = fixture.app();
    write_note(&mut app, "Keep me");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    for (width, height) in [(30, 12), (60, 24), (100, 40)] {
        render(&mut app, width, height);
    }
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    let saved: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(saved["future"]["keep"], true);
}

#[test]
fn continuity_named_pipe_state_is_refused_without_blocking() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let path = fixture.path("state/cockpit/continuity.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let mut app = fixture.app();
    write_note(&mut app, "Must fail");
    assert!(app.message.contains("Continuity unavailable"));
    use std::os::unix::fs::FileTypeExt;
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_fifo());
}

#[test]
fn continuity_session_exit_preserves_the_note_without_giving_it_to_a_new_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let mut app = fixture.app();
    write_note(&mut app, "Only for the old conversation");
    fixture.snapshot(json!([session(SESSION_B, "waiting", "shop", "/r/shop", 2)]));
    app.build();
    assert!(
        !texts(&app, 1)
            .join("\n")
            .contains("Only for the old conversation")
    );
    let saved: Value = serde_json::from_str(
        &fs::read_to_string(fixture.path("state/cockpit/continuity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved["sessions"][SESSION_A]["note"],
        "Only for the old conversation"
    );
    assert!(saved["sessions"][SESSION_B]["note"].is_null());
}

#[test]
fn continuity_failed_note_save_keeps_the_in_memory_note() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let mut app = fixture.app();
    write_note(&mut app, "Original");
    let path = fixture.path("state/cockpit/continuity.json");
    fs::rename(&path, path.with_extension("backup")).unwrap();
    fs::create_dir(&path).unwrap();
    write_note(&mut app, "Replacement");
    assert!(app.message.contains("cannot save continuity"));
    assert!(texts(&app, 1).join("\n").contains("Your note: Original"));
}

#[test]
fn continuity_deferral_boundary_is_exactly_twenty_minutes() {
    let fixture = Fixture::new();
    let question = continuity_question_row(SESSION_A, "Which provider?");
    fixture.snapshot(json!([question.clone()]));
    let mut app = fixture.app();
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    let saved = continuity_saved(&fixture);
    let started = saved["deferrals"][format!("question:{SESSION_A}")]["created_at"]
        .as_i64()
        .unwrap();
    let target = crate::continuity::DeferralTarget::question(
        &sessions::Session::from_value(&question).unwrap(),
    )
    .unwrap();
    let state = app.continuity.as_ref().unwrap();
    assert!(state.is_deferred(&target, started + 1199));
    assert!(!state.is_deferred(&target, started + 1200));
    assert!(!state.is_deferred(&target, started - 1));
}

#[test]
fn continuity_deferred_questions_still_block_automatic_queue_work() {
    let fixture = Fixture::new();
    let mut question = continuity_question_row(SESSION_A, "Which provider?");
    question["state"] = json!("waiting");
    question["since"] = json!(util::now_secs() - 600);
    fixture.snapshot(json!([question]));
    let mut app = fixture.app();
    app.gaming = || false;
    let key = app.todos.add("shop", "/r/shop", "Queued job");
    app.todos.toggle_queued(&key);
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    assert_eq!(app.sessions.len(), 1);
    app.dispatch_queue();
    assert_eq!(app.todos.status_of(&key), "todo");
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn continuity_opening_a_session_marks_its_result_seen() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    assert!(
        texts_with_badges(&app, 1)
            .join("\n")
            .contains("Finished · new result")
    );
    app.handle(Key::FocusLost, &mut FakeScreen::default());
    app.handle(Key::Enter, &mut FakeScreen::default());
    app.build();
    assert!(
        !texts_with_badges(&fixture.app(), 1)
            .join("\n")
            .contains("Finished · new result")
    );
}

#[test]
fn continuity_opening_a_session_leaves_its_question_unanswered() {
    let fixture = Fixture::new();
    let mut row = continuity_result_row("11111111111111111111111111111111", "needs-you");
    row["needs_input"] = json!(true);
    row["full_request"] = json!("Which provider should we use?");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    app.handle(Key::FocusLost, &mut FakeScreen::default());
    app.handle(Key::Enter, &mut FakeScreen::default());
    app.build();
    assert!(
        texts_with_badges(&app, 1)
            .join("\n")
            .contains("new question")
    );
}

#[test]
fn continuity_a_reviewed_result_is_unseen_again_after_the_next_turn() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    continuity_action(&mut app, "1\treviewed", &[]);
    assert!(
        !texts_with_badges(&fixture.app(), 1)
            .join("\n")
            .contains("Finished · new result")
    );
    fixture.snapshot(json!([continuity_result_row(
        "22222222222222222222222222222222",
        "working"
    )]));
    app.build();
    // A working card shows its work; the result reads "Finished · new result" once the turn ends.
    let updates = app.continuity.as_ref().unwrap().updates();
    assert!(
        updates.iter().any(|update| update.session == SESSION_A
            && update.kind == crate::continuity::UpdateKind::Result),
        "{updates:?}"
    );
    assert_eq!(app.sessions[0].state, "working");
}

#[test]
fn continuity_a_result_and_a_question_are_separate_updates() {
    let fixture = Fixture::new();
    let mut row = continuity_result_row("11111111111111111111111111111111", "needs-you");
    row["needs_input"] = json!(true);
    row["full_request"] = json!("Which provider should we use?");
    fixture.snapshot(json!([row.clone()]));
    let mut app = fixture.app();
    let before = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(before.contains("2 unseen"), "{before}");
    continuity_action(&mut app, "1\treviewed", &[]);
    assert!(app.sessions[0].needs_attention());
    row["full_request"] = json!("Should failures be retried?");
    fixture.snapshot(json!([row.clone()]));
    app.build();
    assert!(
        texts_with_badges(&app, 1)
            .join("\n")
            .contains("new question")
    );
    row["needs_input"] = json!(false);
    row["state"] = json!("working");
    fixture.snapshot(json!([row]));
    app.build();
    assert!(
        !texts_with_badges(&app, 1)
            .join("\n")
            .contains("new question")
    );
}

#[test]
fn continuity_old_producers_do_not_invent_result_updates() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "shop", "/r/shop", 1);
    row["outcome"] = json!("tests passed");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    let rows = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(rows.contains("Result tracking unavailable"), "{rows}");
    assert!(!rows.contains("Finished · new result"));
}

#[test]
fn continuity_a_current_session_without_a_result_yet_is_not_told_to_restart() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "notes", "/r/notes", 1);
    row["reports_turns"] = json!(true);
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    let rows = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(!rows.contains("Result tracking unavailable"), "{rows}");
}

#[test]
fn continuity_archived_results_can_be_reviewed_without_opening_an_exited_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    write_note(&mut app, "My next step");
    fixture.snapshot(json!([]));
    app.build();
    continuity_action(&mut app, "-\tupdates", &["0"]);
    let rows = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(
        rows.contains("Latest turn")
            && rows.contains("tests passed")
            && rows.contains("My next step"),
        "{rows}"
    );
    app.handle(Key::Enter, &mut FakeScreen::typing(&["0"]));
    app.redraw();
    assert!(
        !buffer_rows(&render(&mut app, 100, 40))
            .join("\n")
            .contains("1 unseen")
    );
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(" view ") || call.contains(" resume "))
    );
}

#[test]
fn continuity_reviewing_a_captured_result_does_not_acknowledge_a_later_result() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    continuity_action(&mut app, "-\tupdates", &["0"]);
    fixture.snapshot(json!([continuity_result_row(
        "22222222222222222222222222222222",
        "waiting"
    )]));
    app.build();
    app.handle(Key::Enter, &mut FakeScreen::typing(&["0"]));
    app.redraw();
    assert!(
        texts_with_badges(&app, 1)
            .join("\n")
            .contains("Finished · new result")
    );
}

#[test]
fn continuity_a_failed_acknowledgement_leaves_the_result_unseen() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let path = fixture.path("state/cockpit/continuity.json");
    if path.exists() {
        fs::rename(&path, path.with_extension("backup")).unwrap();
    }
    fs::create_dir_all(&path).unwrap();
    continuity_action(&mut app, "1\treviewed", &[]);
    assert!(app.message.contains("cannot save continuity"));
    assert!(
        texts_with_badges(&app, 1)
            .join("\n")
            .contains("Finished · new result")
    );
}

#[test]
fn continuity_a_successful_handoff_links_the_task_and_requests_start_acknowledgement() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "Calculate VAT");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('h'), &mut FakeScreen::default());
    assert!(
        fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(&format!("cockpit-todo start {key}")))
    );
    let saved = continuity_saved(&fixture);
    assert_eq!(saved["tasks"][&key]["assignments"][0]["session"], SESSION_A);
    assert_eq!(
        saved["tasks"][&key]["assignments"][0]["baseline_result"],
        "11111111111111111111111111111111"
    );
    assert!(saved["tasks"][&key]["assignments"][0]["result"].is_null());
}

#[test]
fn continuity_reusing_a_session_does_not_move_the_previous_tasks_result() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let first = app.todos.add("shop", "/r/shop", "First task");
    app.todos.set_status(&first, "doing", SESSION_A);
    app.build();
    fixture.snapshot(json!([continuity_result_row(
        "22222222222222222222222222222222",
        "waiting"
    )]));
    app.build();
    app.todos.set_status(&first, "done", SESSION_A);
    let second = app.todos.add("shop", "/r/shop", "Second task");
    app.todos.set_status(&second, "doing", SESSION_A);
    app.build();
    fixture.snapshot(json!([continuity_result_row(
        "33333333333333333333333333333333",
        "waiting"
    )]));
    app.build();
    let saved = continuity_saved(&fixture);
    assert_eq!(
        saved["tasks"][&first]["assignments"][0]["result"]["id"],
        "22222222222222222222222222222222"
    );
    assert_eq!(
        saved["tasks"][&second]["assignments"][0]["result"]["id"],
        "33333333333333333333333333333333"
    );
}

#[test]
fn continuity_a_simultaneous_assignment_and_result_is_not_guessed() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let first = app.todos.add("shop", "/r/shop", "First task");
    app.todos.set_status(&first, "doing", SESSION_A);
    app.build();
    app.todos.set_status(&first, "done", SESSION_A);
    let second = app.todos.add("shop", "/r/shop", "Second task");
    app.todos.set_status(&second, "doing", SESSION_A);
    fixture.snapshot(json!([continuity_result_row(
        "22222222222222222222222222222222",
        "waiting"
    )]));
    app.build();
    let saved = continuity_saved(&fixture);
    for task in [&first, &second] {
        assert!(saved["tasks"][task]["assignments"][0]["result"].is_null());
    }
    assert_eq!(
        saved["tasks"][&second]["assignments"][0]["baseline_result"],
        "22222222222222222222222222222222"
    );
}

#[test]
fn continuity_exact_branches_link_every_matching_session_not_a_different_repository() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.reviews
        .checkouts
        .insert("acme/shop".into(), "/r/shop".into());
    let key = app.todos.add("shop", "/r/shop", "VAT");
    let entry = Entry::Typed(app.todos.items[0].clone());
    app.hand_to_new_session(&entry, &mut FakeScreen::default());
    let mut first = continuity_result_row("11111111111111111111111111111111", "waiting");
    first["branch"] = json!(format!("todo-{key}"));
    let mut second = first.clone();
    second["session"] = json!(SESSION_B);
    second["cwd"] = json!("/r/clone");
    second["tmux_pane"] = json!("%2");
    let mut unrelated = first.clone();
    unrelated["session"] = json!("cccccccc-0000-4000-8000-000000000000");
    unrelated["repository"] = json!("other/shop");
    unrelated["tmux_pane"] = json!("%3");
    fixture.snapshot(json!([first, second, unrelated]));
    app.build();
    let saved = continuity_saved(&fixture);
    let linked: Vec<_> = saved["tasks"][&key]["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|assignment| assignment["session"].as_str().unwrap())
        .collect();
    assert_eq!(linked, [SESSION_A, SESSION_B]);
}

#[test]
fn continuity_background_assignment_waits_for_the_attributed_start_update() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "VAT");
    let entry = Entry::Typed(app.todos.items[0].clone());
    app.hand_to_claude(&entry, &mut FakeScreen::default());
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    app.build();
    assert_eq!(continuity_saved(&fixture)["tasks"][&key]["pending"], true);
    app.todos.set_status(&key, "doing", SESSION_A);
    app.build();
    let saved = continuity_saved(&fixture);
    assert_eq!(saved["tasks"][&key]["pending"], false);
    assert_eq!(saved["tasks"][&key]["assignments"][0]["session"], SESSION_A);
}

#[test]
fn continuity_issue_branches_require_the_full_github_repository() {
    let fixture = Fixture::new();
    let mut first = continuity_result_row("11111111111111111111111111111111", "waiting");
    first["branch"] = json!("issue-12");
    let mut other = first.clone();
    other["session"] = json!(SESSION_B);
    other["repository"] = json!("other/shop");
    other["tmux_pane"] = json!("%2");
    fixture.snapshot(json!([first, other]));
    let mut app = fixture.app();
    app.todos.set_status("acme/shop#12", "doing", "");
    app.reviews.set_issues(vec![Issue {
        number: 12,
        title: "Fix VAT".into(),
        url: "https://github.com/acme/shop/issues/12".into(),
        repository: "shop".into(),
        full_repository: "acme/shop".into(),
        updated: String::new(),
        labels: Vec::new(),
    }]);
    app.build();
    let saved = continuity_saved(&fixture);
    assert_eq!(
        saved["tasks"]["acme/shop#12"]["assignments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        saved["tasks"]["acme/shop#12"]["assignments"][0]["session"],
        SESSION_A
    );
    assert_eq!(
        saved["tasks"]["acme/shop#12"]["snapshot"]["title"],
        "Fix VAT"
    );
}

#[test]
fn continuity_task_updates_preserve_unknown_snapshot_fields() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "VAT");
    app.todos.set_status(&key, "doing", SESSION_A);
    app.build();
    let mut saved = continuity_saved(&fixture);
    saved["tasks"][&key]["snapshot"]["future"] = json!({"keep":true});
    saved["tasks"][&key]["assignments"][0]["future"] = json!({"keep":true});
    fixture.write("state/cockpit/continuity.json", &saved.to_string());
    let mut app = fixture.app();
    app.todos.edit(&key, "Updated VAT task", "Notes");
    app.build();
    let saved = continuity_saved(&fixture);
    assert_eq!(saved["tasks"][&key]["snapshot"]["future"]["keep"], true);
    assert_eq!(
        saved["tasks"][&key]["assignments"][0]["future"]["keep"],
        true
    );
}

#[test]
fn continuity_changing_branches_does_not_move_an_earlier_assignments_result() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "VAT");
    app.todos.set_status(&key, "doing", SESSION_A);
    app.build();
    fixture.snapshot(json!([continuity_result_row(
        "22222222222222222222222222222222",
        "waiting"
    )]));
    app.build();
    let mut changed = continuity_result_row("33333333333333333333333333333333", "waiting");
    changed["branch"] = json!("feat/other");
    changed["last_result"]["branch"] = json!("feat/other");
    fixture.snapshot(json!([changed]));
    app.build();
    let saved = continuity_saved(&fixture);
    let assignments = saved["tasks"][&key]["assignments"].as_array().unwrap();
    assert_eq!(assignments.len(), 2);
    assert_eq!(
        assignments[0]["result"]["id"],
        "22222222222222222222222222222222"
    );
    assert!(assignments[1]["result"].is_null());
}

#[test]
fn continuity_result_extensions_survive_persistence() {
    let fixture = Fixture::new();
    let mut row = continuity_result_row("11111111111111111111111111111111", "waiting");
    row["last_result"]["future"] = json!({"keep":true});
    fixture.snapshot(json!([row]));
    let _app = fixture.app();
    assert_eq!(
        continuity_saved(&fixture)["sessions"][SESSION_A]["result"]["future"]["keep"],
        true
    );
}

#[test]
fn continuity_trail_navigation_opens_the_live_session_and_selects_the_done_task() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "VAT");
    app.todos.set_status(&key, "doing", SESSION_A);
    app.build();
    continuity_action(&mut app, "1\ttask-trail", &[]);
    app.handle(Key::Enter, &mut FakeScreen::typing(&["1"]));
    assert!(
        fixture
            .wait_for_calls("cockpit", 1)
            .iter()
            .any(|call| call == &format!("cockpit view {SESSION_A}"))
    );
    app.todos.set_status(&key, "done", SESSION_A);
    app.build();
    continuity_action(&mut app, "1\ttask-trail", &[]);
    app.handle(Key::Enter, &mut FakeScreen::typing(&["0"]));
    assert_eq!(app.focus, 3);
    assert_eq!(app.todo_tab, 4);
    assert_eq!(app.todo_entries[app.panels[3].selected].key(), key);
    assert_eq!(app.todos.status_of(&key), "done");
}

#[test]
fn continuity_task_trails_link_every_exact_pr_without_title_or_repository_guessing() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "Calculate VAT");
    app.todos.set_status(&key, "doing", SESSION_A);
    let mut prs = reviews::parse_reviews(
        r#"[{"number":61,"title":"Calculate VAT","url":"https://github.com/acme/shop/pull/61","headRefName":"feat/vat","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","reviewDecision":"REVIEW_REQUIRED"},{"number":62,"title":"Different title","url":"https://github.com/acme/shop/pull/62","headRefName":"feat/vat","headRefOid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},{"number":63,"title":"Calculate VAT","url":"https://github.com/acme/shop/pull/63","headRefName":"Feat/vat"}]"#,
        "acme/shop",
    );
    prs.extend(reviews::parse_reviews(r#"[{"number":64,"title":"Calculate VAT","url":"https://github.com/other/shop/pull/64","headRefName":"feat/vat"}]"#, "other/shop"));
    app.reviews.set_rows(prs);
    app.build();
    app.focus_panel(3);
    continuity_action(&mut app, "3\ttask-trail", &[]);
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(
        rows.contains("shop#61") && rows.contains("shop#62"),
        "{rows}"
    );
    assert!(
        !rows.contains("shop#63") && !rows.contains("shop#64"),
        "{rows}"
    );
    app.handle(Key::Enter, &mut FakeScreen::typing(&["2"]));
    assert!(
        fixture
            .calls("hyprctl")
            .iter()
            .any(|call| call.contains("https://github.com/acme/shop/pull/61")),
        "{:?}",
        fixture.calls("hyprctl")
    );
    app.reviews.set_rows(Vec::new());
    app.build();
    app.focus_panel(3);
    continuity_action(&mut app, "3\ttask-trail", &[]);
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(
        rows.contains("shop#61") && rows.contains("last known"),
        "{rows}"
    );
}

#[test]
fn continuity_pr_metadata_preserves_the_exact_branch_and_revision() {
    let rows = reviews::parse_reviews(
        r#"[{"number":61,"headRefName":"Feat/VAT","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]"#,
        "Acme/Shop",
    );
    let row = &rows[0];
    assert_eq!(row.full_repository, "acme/shop");
    // The branch and revision must reach a trail, not be discarded by the parser.
    let fixture = Fixture::new();
    let mut snapshot = continuity_result_row("11111111111111111111111111111111", "waiting");
    snapshot["branch"] = json!("Feat/VAT");
    fixture.snapshot(json!([snapshot]));
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "VAT");
    app.todos.set_status(&key, "doing", SESSION_A);
    app.reviews.set_rows(rows);
    app.build();
    let saved = continuity_saved(&fixture);
    assert_eq!(
        saved["prs"]["acme/shop#61"]["head_oid"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[test]
fn continuity_review_search_enriches_requested_prs_with_branch_and_revision() {
    let fixture = Fixture::new();
    let updated = chrono::Utc::now().to_rfc3339();
    let search = json!([{"number":61,"title":"VAT","repository":{"nameWithOwner":"acme/shop"},"updatedAt":updated}]);
    let detail = json!({"number":61,"title":"VAT","updatedAt":updated,"headRefName":"feat/vat","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","reviewDecision":"REVIEW_REQUIRED"});
    fixture.shim(
        "gh",
        &format!(
            "case \"$1 $2\" in 'search prs') echo '{search}';; 'pr view') echo '{detail}';; esac"
        ),
    );
    let (rows, error) = reviews::fetch_reviews(&[]);
    assert!(error.is_empty(), "{error}");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].head_branch, "feat/vat");
    assert_eq!(rows[0].head_oid, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
}

#[test]
fn continuity_pruned_tasks_open_archived_details_instead_of_being_recreated() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "Archived task");
    app.todos.set_status(&key, "doing", SESSION_A);
    app.build();
    app.todos.remove(&key).unwrap();
    app.build();
    continuity_action(&mut app, "1\ttask-trail", &[]);
    app.handle(Key::Enter, &mut FakeScreen::typing(&["0"]));
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(
        rows.contains("Archived task") && rows.contains("Archived"),
        "{rows}"
    );
    assert!(app.todos.items.is_empty());
}

#[test]
fn continuity_grouping_still_delivers_queued_messages_to_quiet_sessions() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r/runway", 1),
        continuity_question_row(SESSION_B, "Retry failures?")
    ]));
    let mut app = fixture.app();
    crate::prompts::add(SESSION_A, "follow-up").unwrap();
    app.build();
    app.dispatch_prompts();
    assert_eq!(
        fixture.calls("cockpit"),
        [format!("cockpit say {SESSION_A} follow-up")]
    );
}

#[test]
fn continuity_grouping_hands_a_task_to_the_quiet_viewed_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r/runway", 1),
        continuity_question_row(SESSION_B, "Retry failures?")
    ]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.todos.add("runway", "/r/runway", "add a healthcheck");
    hand_over_first_item(&mut app);
    let calls = fixture.wait_for_calls("cockpit", 1);
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with(&format!("cockpit say {SESSION_A} add a healthcheck"))),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|call| call.contains(" background ")),
        "{calls:?}"
    );
}

#[test]
fn continuity_a_session_title_with_a_tmux_sequence_still_records_its_result() {
    let fixture = Fixture::new();
    fixture.write(
        &format!("home/.claude/projects/p/{SESSION_A}.jsonl"),
        &format!(
            "{{\"type\":\"ai-title\",\"aiTitle\":\"Fix #{{pane_title}} {}\"}}\n",
            "x".repeat(400)
        ),
    );
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    app.build();
    app.build();
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(!rows.contains("Continuity unavailable"), "{rows}");
    let saved = continuity_saved(&fixture);
    assert_eq!(
        saved["sessions"][SESSION_A]["result"]["id"],
        "11111111111111111111111111111111"
    );
}

#[test]
fn continuity_a_task_note_with_a_tmux_sequence_still_links_its_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    let key = app.todos.add("shop", "/r/shop", "Interpolate #{name}");
    app.todos.items[0]
        .0
        .insert("notes".into(), json!("use \"#{x}\" here"));
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('h'), &mut FakeScreen::default());
    app.build();
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(!rows.contains("Continuity unavailable"), "{rows}");
    assert_eq!(
        continuity_saved(&fixture)["tasks"][&key]["assignments"][0]["session"],
        SESSION_A
    );
}

#[test]
fn continuity_an_issue_of_a_mixed_case_repository_links_its_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.reviews
        .checkouts
        .insert("Acme/Shop".into(), "/r/shop".into());
    app.reviews.set_issues(vec![Issue {
        number: 12,
        title: "Broken VAT".into(),
        url: String::new(),
        repository: "Shop".into(),
        full_repository: "Acme/Shop".into(),
        updated: "2026-10-19T00:00:00Z".into(),
        labels: Vec::new(),
    }]);
    hand_over_first_item(&mut app);
    app.build();
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(!rows.contains("Continuity unavailable"), "{rows}");
    assert_eq!(app.todos.status_of("Acme/Shop#12"), "doing");
    assert_eq!(
        continuity_saved(&fixture)["tasks"]["acme/shop#12"]["assignments"][0]["session"],
        SESSION_A
    );
}

#[test]
fn continuity_deferral_keeps_questions_visible_but_out_of_attention_navigation() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        continuity_question_row(SESSION_A, "Which provider?"),
        continuity_question_row(SESSION_B, "Retry failures?")
    ]));
    let mut app = fixture.app();
    let before = util::now_secs();
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    let saved = continuity_saved(&fixture);
    let expiry = saved["deferrals"][format!("question:{SESSION_A}")]["expires_at"]
        .as_i64()
        .expect("persisted expiry");
    assert!(expiry >= before + 1200 && expiry <= util::now_secs() + 1200);
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(rows.contains("1 deferred"), "{rows}");
    assert!(app.sessions[0].needs_attention());
    assert!(
        app.sessions
            .iter()
            .any(|session| session.session == SESSION_A)
    );
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_B);
    assert_eq!(app.sessions.len(), 2, "Later keeps both sessions reachable");
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(rows.contains("1 deferred"), "{rows}");
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(" say "))
    );
}

#[test]
fn continuity_deferrals_survive_restart_and_expire_without_acknowledging_questions() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_question_row(
        SESSION_A,
        "Which provider?"
    )]));
    let mut app = fixture.app();
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    let mut reloaded = fixture.app();
    assert!(
        buffer_rows(&render(&mut reloaded, 110, 45))
            .join("\n")
            .contains("1 deferred")
    );
    let mut saved = continuity_saved(&fixture);
    saved["deferrals"][format!("question:{SESSION_A}")]["created_at"] =
        json!(util::now_secs() - 1201);
    saved["deferrals"][format!("question:{SESSION_A}")]["expires_at"] = json!(util::now_secs() - 1);
    fixture.write("state/cockpit/continuity.json", &saved.to_string());
    let mut expired = fixture.app();
    let rows = buffer_rows(&render(&mut expired, 110, 45)).join("\n");
    assert!(!rows.contains("1 deferred"), "{rows}");
    assert!(
        rows.contains("1 need you") && rows.contains("1 unseen"),
        "{rows}"
    );
}

#[test]
fn continuity_new_questions_and_command_requests_invalidate_an_old_deferral() {
    let fixture = Fixture::new();
    let question = continuity_question_row(SESSION_A, "Which provider?");
    fixture.snapshot(json!([question.clone()]));
    let mut app = fixture.app();
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    let mut command = question.clone();
    command["ask_to_run"] = json!("approval");
    fixture.snapshot(json!([command]));
    app.build();
    assert!(
        continuity_saved(&fixture)["deferrals"]
            .as_object()
            .is_some_and(|values| values.is_empty())
    );
    fixture.snapshot(json!([question.clone()]));
    app.build();
    assert!(
        !buffer_rows(&render(&mut app, 110, 45))
            .join("\n")
            .contains("1 deferred")
    );
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    fixture.snapshot(json!([continuity_question_row(
        SESSION_A,
        "Should failures be retried?"
    )]));
    app.build();
    assert!(
        continuity_saved(&fixture)["deferrals"]
            .as_object()
            .is_some_and(|values| values.is_empty())
    );
}

#[test]
fn continuity_a_deferred_question_answered_elsewhere_leaves_nothing_needing_you() {
    let fixture = Fixture::new();
    let mut row = continuity_question_row(SESSION_A, "Which provider?");
    fixture.snapshot(json!([row.clone()]));
    let mut app = fixture.app();
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    row["state"] = json!("waiting");
    row["needs_input"] = json!(false);
    fixture.snapshot(json!([row]));
    app.build();
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(rows.contains("Nothing needs you"), "{rows}");
}

#[test]
fn continuity_nondeferrable_requests_are_never_approved_or_hidden() {
    for case in [
        "approval",
        "input",
        "running",
        "permission",
        "unknown",
        "working",
        "ready",
    ] {
        let fixture = Fixture::new();
        let mut row = continuity_question_row(SESSION_A, "Which provider?");
        match case {
            "approval" | "input" | "running" => row["ask_to_run"] = json!(case),
            "permission" => {
                row["request"] = json!("Do you want to proceed?");
                row["detail"] = json!("1. Yes\n2. No");
            }
            "unknown" | "working" => row["state"] = json!(case),
            _ => {
                row["state"] = json!("waiting");
                row["needs_input"] = json!(false);
            }
        }
        fixture.snapshot(json!([row]));
        let mut app = fixture.app();
        continuity_action(&mut app, "1\tdefer-20m", &[]);
        assert!(
            app.message.contains("only explicit questions or reviews"),
            "{case}: {}",
            app.message
        );
        assert!(fixture.calls("cockpit").is_empty(), "{case}");
        let saved = if fixture.path("state/cockpit/continuity.json").exists() {
            continuity_saved(&fixture)
        } else {
            json!({})
        };
        assert!(
            saved
                .get("deferrals")
                .is_none_or(|value| value.as_object().is_some_and(|values| values.is_empty())),
            "{case}"
        );
    }
}

#[test]
fn continuity_bring_back_is_explicit_and_a_failed_deferral_remains_actionable() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_question_row(
        SESSION_A,
        "Which provider?"
    )]));
    let mut app = fixture.app();
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    continuity_action(&mut app, "1\tbring-back", &[]);
    assert!(
        !buffer_rows(&render(&mut app, 110, 45))
            .join("\n")
            .contains("1 deferred")
    );
    let path = fixture.path("state/cockpit/continuity.json");
    fs::rename(&path, path.with_extension("backup")).unwrap();
    fs::create_dir(&path).unwrap();
    continuity_action(&mut app, "1\tdefer-20m", &[]);
    assert!(app.message.contains("cannot save continuity"));
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_A);
    assert!(
        !buffer_rows(&render(&mut app, 110, 45))
            .join("\n")
            .contains("1 deferred")
    );
}

#[test]
fn continuity_pr_deferral_tracks_head_revision_and_review_decision() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let row = json!({"number":61,"title":"VAT","url":"https://github.com/acme/shop/pull/61","headRefName":"feat/vat","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","reviewDecision":"REVIEW_REQUIRED"});
    app.reviews.set_rows(reviews::parse_reviews(
        &json!([row.clone()]).to_string(),
        "acme/shop",
    ));
    app.build();
    app.focus_panel(2);
    continuity_action(&mut app, "2\tdefer-20m", &[]);
    assert!(continuity_saved(&fixture)["deferrals"]["pr:acme/shop#61"]["expires_at"].is_i64());
    for (field, value) in [
        (
            "headRefOid",
            json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ),
        ("reviewDecision", json!("APPROVED")),
    ] {
        let mut changed = row.clone();
        changed[field] = value;
        app.reviews.set_rows(reviews::parse_reviews(
            &json!([changed]).to_string(),
            "acme/shop",
        ));
        app.build();
        assert!(
            continuity_saved(&fixture)["deferrals"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        app.reviews.set_rows(reviews::parse_reviews(
            &json!([row.clone()]).to_string(),
            "acme/shop",
        ));
        app.build();
        continuity_action(&mut app, "2\tdefer-20m", &[]);
    }
    let mut missing = row;
    missing["headRefOid"] = json!("");
    app.reviews.set_rows(reviews::parse_reviews(
        &json!([missing]).to_string(),
        "acme/shop",
    ));
    app.build();
    continuity_action(&mut app, "2\tdefer-20m", &[]);
    assert!(app.message.contains("only explicit questions or reviews"));
}

#[test]
fn continuity_a_review_that_does_not_need_review_cannot_be_deferred() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (decision, head, draft) in [
        (
            "APPROVED",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            false,
        ),
        (
            "CHANGES_REQUESTED",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            false,
        ),
        ("REVIEW_REQUIRED", "", false),
        (
            "REVIEW_REQUIRED",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            true,
        ),
    ] {
        let rows = reviews::parse_reviews(
            &json!([{"number":61,"headRefOid":head,"reviewDecision":decision,"isDraft":draft}])
                .to_string(),
            "acme/shop",
        );
        app.reviews.set_rows(rows);
        app.build();
        continuity_action(&mut app, "2\tdefer-20m", &[]);
        assert!(
            app.message.contains("only explicit questions or reviews"),
            "{}",
            app.message
        );
    }
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn continuity_return_notes_persist_without_becoming_agent_messages() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", "/r/shop", 1)]));
    let mut app = fixture.app();
    write_note(&mut app, "Review the migration first.");
    assert!(
        texts(&app, 1)
            .join("\n")
            .contains("Your note: Review the migration first.")
    );
    assert!(
        texts(&fixture.app(), 1)
            .join("\n")
            .contains("Review the migration first.")
    );
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(" say "))
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(fixture.path("state/cockpit/continuity.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn continuity_queue_explanation_and_dispatch_agree_about_blocked_capacity() {
    for (case, expected, can_dispatch) in [
        ("ready", "ready for the next dispatch check", true),
        ("question", "awaiting your answer", false),
        ("approval", "awaiting your answer", false),
        ("input", "awaiting your answer", false),
        ("running", "approved command is running", false),
        ("working", "still working", false),
        ("unknown", "unknown state", false),
        ("terminal", "terminal-managed session", false),
        ("external", "no terminal target", false),
        ("disabled", "queue disabled", false),
        ("gaming", "paused while gaming", false),
        ("busy", "assigned todo still in progress", false),
        ("pending", "manual message queued", false),
        ("viewed", "viewed on the active cockpit workspace", false),
        ("fresh", "idle window", false),
    ] {
        let (fixture, mut app) = queue_fixture("waiting", if case == "fresh" { 0 } else { 600 });
        let mut row = session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            util::now_secs() - if case == "fresh" { 0 } else { 600 },
        );
        match case {
            "question" => {
                row["needs_input"] = json!(true);
                row["request"] = json!("Which provider?");
            }
            "approval" => row["ask_to_run"] = json!("approval"),
            "input" => row["ask_to_run"] = json!("input"),
            "running" => row["ask_to_run"] = json!("running"),
            "working" | "unknown" => row["state"] = json!(case),
            "terminal" => row["terminal"] = json!(true),
            "external" => row["tmux_pane"] = Value::Null,
            "disabled" => {
                fixture.write("state/cockpit/settings.json", "{\"todo-queue\":false}");
            }
            "gaming" => app.gaming = || true,
            "busy" => {
                let id = app.todos.add("runway", "/r/runway", "already working");
                app.todos.set_status(&id, "doing", SESSION_A);
            }
            "pending" => crate::prompts::add(SESSION_A, "My manual follow-up").unwrap(),
            "viewed" => {
                fixture.view(SESSION_A);
                fixture.shim("hyprctl", "echo '{\"id\":6}'");
            }
            _ => {}
        }
        fixture.snapshot(json!([row]));
        app.build();
        let key = app.todos.add("runway", "/r/runway", "next job");
        app.todos.toggle_queued(&key);
        continuity_action(&mut app, "-\tqueue", &[]);
        let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
        assert!(rows.contains(expected), "{case}: {rows}");
        assert_eq!(app.todos.status_of(&key), "todo");
        app.dispatch_queue();
        assert_eq!(app.todos.status_of(&key) == "doing", can_dispatch, "{case}");
        assert_eq!(
            fixture
                .calls("cockpit")
                .iter()
                .any(|call| call.contains(" say ")),
            can_dispatch,
            "{case}"
        );
    }
}

#[test]
fn continuity_queue_view_keeps_priority_and_designer_routes_without_network_calls() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    let normal = app.todos.add("runway", "/r/runway", "normal job");
    let urgent = app.todos.add("runway", "/r/runway", "urgent job");
    app.todos.change_priority(&urgent, 1);
    let design = design_item(&mut app, "runway", "/r/runway", "design job");
    for key in [&normal, &urgent, &design] {
        app.todos.toggle_queued(key);
    }
    continuity_action(&mut app, "-\tqueue", &[]);
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(
        rows.find("urgent job").unwrap() < rows.find("normal job").unwrap(),
        "{rows}"
    );
    assert!(rows.contains("queued behind"), "{rows}");
    assert!(
        rows.contains("design job") && rows.contains("no matching session"),
        "{rows}"
    );
    assert!(fixture.calls("gh").is_empty());
    assert!(fixture.calls("git").is_empty());
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn continuity_queue_idle_boundary_is_exactly_five_minutes() {
    let fixture = Fixture::new();
    let context = crate::queue::QueueContext {
        now: 1000,
        enabled: true,
        gaming: false,
        viewed: String::new(),
        workspace_active: false,
        qa_sessions: Default::default(),
        chief_session: None,
        host_available: true,
    };
    for (since, remaining) in [(701, Some(1)), (700, None), (699, None)] {
        let reported = sessions::Session::from_value(&session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            since,
        ))
        .unwrap();
        assert_eq!(
            crate::queue::block(&reported, &context, false, false),
            remaining.map(|remaining| crate::queue::QueueBlock::Idle { remaining })
        );
    }
    assert!(fixture.calls("cockpit").is_empty());
}
