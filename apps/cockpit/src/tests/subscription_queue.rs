use super::*;

#[test]
fn subscription_ownership_reads_refuse_corrupt_or_partially_discarded_todo_state() {
    let _fixture = Fixture::new();
    fs::create_dir_all(crate::paths::state_dir()).unwrap();
    for invalid in [
        "not JSON",
        "42",
        r#"{"items":[{"id":"1797cdd6"}]}"#,
        r#"{"issues":{"not-an-issue":{"status":"doing"}}}"#,
    ] {
        fs::write(crate::paths::todo(), invalid).unwrap();
        assert!(TodoList::load_checked().is_err(), "{invalid}");
        assert_eq!(fs::read_to_string(crate::paths::todo()).unwrap(), invalid);
    }
    fs::write(crate::paths::todo(), r#"{"items":[],"issues":{}}"#).unwrap();
    assert!(TodoList::load_checked().is_ok());
}

#[test]
fn subscription_prompt_transfer_includes_late_messages_without_replaying_moved_files() {
    let _fixture = Fixture::new();
    let replacement = "cdx_c214e8c0c9458f09ea4300974071bcad955995188f33264e341800f8a2bb5fcb";
    crate::prompts::add(SESSION_A, "First queued instruction").unwrap();
    assert_eq!(
        crate::prompts::transfer_subscription_prompts(SESSION_A, replacement).unwrap(),
        1
    );
    crate::prompts::add(SESSION_A, "Message arriving during handoff").unwrap();
    assert_eq!(
        crate::prompts::transfer_subscription_prompts(SESSION_A, replacement).unwrap(),
        1
    );
    assert_eq!(
        crate::prompts::transfer_subscription_prompts(SESSION_A, replacement).unwrap(),
        0
    );
    assert_eq!(crate::prompts::count(SESSION_A), 0);
    let prompts: Vec<_> = crate::prompts::pending(replacement)
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    assert_eq!(
        prompts,
        [
            "First queued instruction",
            "Message arriving during handoff"
        ]
    );
}

#[test]
fn subscription_prompt_transfer_recovers_a_published_link_without_copying_the_message_twice() {
    let _fixture = Fixture::new();
    crate::prompts::add(SESSION_A, "Keep this instruction exactly once").unwrap();
    let source = crate::prompts::pending(SESSION_A).remove(0);
    let target = crate::prompts::folder(SESSION_B).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::hard_link(&source, target.join(source.file_name().unwrap())).unwrap();
    assert_eq!(
        crate::prompts::transfer_subscription_prompts(SESSION_A, SESSION_B).unwrap(),
        1
    );
    assert_eq!(crate::prompts::count(SESSION_A), 0);
    assert_eq!(crate::prompts::count(SESSION_B), 1);
}

#[test]
fn subscription_prompt_transfer_never_overwrites_another_queued_message() {
    let _fixture = Fixture::new();
    crate::prompts::add(SESSION_A, "Source instruction").unwrap();
    let source = crate::prompts::pending(SESSION_A).remove(0);
    let target = crate::prompts::folder(SESSION_B).unwrap();
    fs::create_dir_all(&target).unwrap();
    let target = target.join(source.file_name().unwrap());
    fs::write(&target, "Unrelated instruction").unwrap();
    assert!(crate::prompts::transfer_subscription_prompts(SESSION_A, SESSION_B).is_err());
    assert_eq!(fs::read_to_string(&source).unwrap(), "Source instruction");
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "Unrelated instruction"
    );
}

#[test]
fn a_waiting_subscription_switch_shows_its_reason_once_in_the_message_line() {
    use crate::subscription_fallback::{Binding, RoutingStore, Selection};
    let (_fixture, mut app) = queue_fixture("waiting", 7200);
    let source = Binding {
        session: SESSION_A.into(),
        runtime: "runtime-source".into(),
        native_thread: None,
        selection: Selection {
            agent: "claude".into(),
            provider: "anthropic".into(),
            model: "claude-opus-4-7".into(),
            variant: Some("high".into()),
            account: "a".repeat(64),
        },
    };
    let mut store = RoutingStore::load(&crate::paths::subscription_routes()).unwrap();
    store.track(source, "/r/runway", "1797cdd6", 100).unwrap();
    store
        .set_waiting(SESSION_A, Some("both subscriptions are exhausted"))
        .unwrap();
    drop(store);
    app.dispatch_subscription_routes();
    assert_eq!(
        app.message,
        "runway: subscription switch waiting: both subscriptions are exhausted"
    );
    app.message.clear();
    app.dispatch_subscription_routes();
    assert!(
        app.message.is_empty(),
        "an unchanged reason must not keep reappearing"
    );
}

#[test]
fn subscription_poll_starts_one_background_sweep_without_blocking_the_sidebar() {
    let (fixture, mut app) = queue_fixture("waiting", 7200);
    app.poll_subscription_routes();
    app.poll_subscription_routes();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        fixture
            .calls("cockpit")
            .iter()
            .filter(|call| call.contains("subscription-sweep"))
            .count(),
        1
    );
}

#[test]
fn automatic_actions_leave_routing_held_sessions_and_their_work_untouched() {
    for action in ["queue", "prompt", "restart", "sleep"] {
        let (fixture, mut app) = queue_fixture("waiting", 7200);
        no_team_members(&fixture);
        app.check_team();
        prepare_subscription_hold();
        app.dispatch_subscription_routes();
        match action {
            "queue" => {
                let key = app
                    .todos
                    .add("runway", "/r/runway", "A different queued task");
                app.todos.toggle_queued(&key);
                app.dispatch_queue();
                assert_eq!(app.todos.status_of(&key), "todo");
            }
            "prompt" => {
                crate::prompts::add(SESSION_A, "A queued user message").unwrap();
                app.dispatch_prompts();
                assert_eq!(crate::prompts::count(SESSION_A), 1);
            }
            "restart" => {
                crate::prompts::want_restart(SESSION_A).unwrap();
                app.dispatch_prompts();
                assert!(crate::prompts::restart_wanted(SESSION_A));
            }
            "sleep" => {
                app.dispatch_sleep();
                std::thread::sleep(Duration::from_millis(200));
            }
            _ => unreachable!(),
        }
        assert!(fixture.calls("cockpit").is_empty(), "{action}");
    }
}

#[test]
fn remote_messages_queue_while_subscription_ownership_is_pending() {
    let (fixture, _app) = queue_fixture("waiting", 7200);
    prepare_subscription_hold();
    let outcome = crate::remote::message(SESSION_A, "Stop before pushing this change").unwrap();
    assert!(outcome.contains("queued"), "{outcome}");
    assert_eq!(
        crate::prompts::oldest(SESSION_A).unwrap().0,
        "Stop before pushing this change"
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn remote_messages_addressed_to_a_retired_session_follow_its_active_replacement() {
    let (fixture, _app) = queue_fixture("waiting", 7200);
    prepare_subscription_hold();
    let replacement = activate_subscription_replacement();
    let mut row = session(replacement, "working", "runway", "/r/runway", 1);
    row["agent"] = json!("codex");
    fixture.snapshot(json!([row]));
    let outcome = crate::remote::message(SESSION_A, "A message for the logical task").unwrap();
    assert!(outcome.contains("queued"), "{outcome}");
    assert_eq!(crate::prompts::count(SESSION_A), 0);
    assert_eq!(
        crate::prompts::oldest(replacement).unwrap().0,
        "A message for the logical task"
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn late_messages_to_retired_sessions_follow_the_active_owner_without_terminal_replay() {
    let (fixture, mut app) = queue_fixture("waiting", 7200);
    prepare_subscription_hold();
    let replacement = activate_subscription_replacement();
    crate::prompts::add(SESSION_A, "Message arriving after ownership binding").unwrap();
    app.dispatch_subscription_routes();
    app.dispatch_subscription_routes();
    app.dispatch_prompts();
    assert_eq!(crate::prompts::count(SESSION_A), 0);
    assert_eq!(crate::prompts::count(replacement), 1);
    assert_eq!(
        crate::prompts::oldest(replacement).unwrap().0,
        "Message arriving after ownership binding"
    );
    assert!(
        fixture.calls("cockpit").is_empty(),
        "the retired terminal must never receive these messages"
    );
}

#[test]
fn routed_codex_messages_wait_for_the_authenticated_inbox_instead_of_unsupported_say() {
    let (fixture, mut app) = queue_fixture("waiting", 7200);
    prepare_subscription_hold();
    let replacement = activate_subscription_replacement();
    let mut row = session(
        replacement,
        "waiting",
        "runway",
        "/r/runway",
        util::now() as i64,
    );
    row["agent"] = json!("codex");
    app.running_sessions = vec![sessions::Session::from_value(&row).unwrap()];
    crate::prompts::add(replacement, "A later user instruction").unwrap();
    app.dispatch_subscription_routes();
    app.dispatch_prompts();
    assert_eq!(crate::prompts::count(replacement), 1);
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn an_unreachable_team_host_blocks_queue_sends() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    let id = app.todos.add("runway", "/r/runway", "protected queue");
    app.todos.toggle_queued(&id);
    let entry = Entry::Typed(
        app.todos
            .items
            .iter()
            .find(|item| item.id() == id)
            .unwrap()
            .clone(),
    );
    let _ = fs::remove_file(crate::paths::team_host_socket());
    let reported =
        sessions::Session::from_value(&session(SESSION_A, "waiting", "runway", "/r/runway", 1))
            .unwrap();
    let context = crate::queue::QueueContext {
        now: util::now_secs(),
        enabled: true,
        gaming: false,
        viewed: String::new(),
        workspace_active: false,
        qa_sessions: Default::default(),
        chief_session: None,
        host_available: false,
    };
    assert_eq!(
        crate::queue::block(&reported, &context, false, false),
        Some(crate::queue::QueueBlock::HostUnavailable)
    );
    app.dispatch_queue();
    assert_eq!(app.todos.status_of(&id), "todo");
    assert!(
        app.hand_to_claude(&entry, &mut FakeScreen::default())
            .contains("unavailable")
    );
    assert!(
        app.hand_to_new_session(&entry, &mut FakeScreen::default())
            .contains("unavailable")
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn an_enabled_chief_keeps_the_todo_queue_sending() {
    let fixture = Fixture::new();
    team_host(&fixture, json!({"members":[]}), vec![]);
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        util::now_secs() - 600
    )]));
    let mut first = fixture.app();
    first.gaming = || false;
    let id = first.todos.add("runway", "/r/runway", "queued");
    first.todos.toggle_queued(&id);
    let handed = first.todos.add("other", "/r/other", "handed");
    first
        .todos
        .items
        .iter_mut()
        .find(|item| item.id() == handed)
        .unwrap()
        .0
        .insert("queued_by".into(), json!("another project"));
    first.todos.toggle_queued(&handed);
    let mut second = fixture.app();
    second.gaming = || false;
    first.dispatch_queue();
    second.dispatch_queue();
    assert_eq!(first.todos.status_of(&id), "doing");
    assert_eq!(TodoList::load().status_of(&handed), "doing");
    assert_eq!(fixture.wait_for_calls("cockpit", 2).len(), 2);
}

#[test]
fn an_item_the_host_refuses_does_not_hold_back_the_next_queued_item() {
    let fixture = Fixture::new();
    let (mut app, first, second) = two_queued_items(&fixture);
    queue_host_refusing(
        &fixture,
        first.clone(),
        "work already reserved by legacy: ses_gone",
    );
    app.dispatch_queue();
    assert_eq!(app.todos.status_of(&first), "todo");
    assert_eq!(app.todos.status_of(&second), "doing");
    let log = fs::read_to_string(fixture.path("state/cockpit/log")).unwrap_or_default();
    assert!(
        log.contains(&format!("queue: {first} not given to runway (")),
        "the refusal names the item and the session: {log}"
    );
}

#[test]
fn a_session_the_host_finds_not_ready_is_offered_no_further_item() {
    let fixture = Fixture::new();
    let (mut app, first, second) = two_queued_items(&fixture);
    let requests = queue_host_refusing(
        &fixture,
        first.clone(),
        "automatic legacy admission is no longer ready: the session is not idle",
    );
    app.dispatch_queue();
    assert_eq!(app.todos.status_of(&second), "todo");
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["request"] == "reserve_task")
            .count(),
        1
    );
}

#[test]
fn the_todo_queue_never_offers_work_to_the_chief_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        util::now_secs() - 600
    )]));
    let requests = team_host(
        &fixture,
        json!({"members": [], "chief_session": SESSION_A}),
        vec![],
    );
    let mut app = fixture.app();
    app.gaming = || false;
    let id = app.todos.add("runway", "/r/runway", "queued");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    assert_eq!(app.todos.status_of(&id), "todo");
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request["request"] != "reserve_task"),
        "{:?}",
        requests.lock().unwrap()
    );
    let log = fs::read_to_string(fixture.path("state/cockpit/log")).unwrap_or_default();
    assert!(!log.contains("queue:"), "{log}");
    assert!(fixture.calls("cockpit").is_empty());

    let reported =
        sessions::Session::from_value(&session(SESSION_A, "waiting", "runway", "/r/runway", 1))
            .unwrap();
    let context = crate::queue::QueueContext {
        now: util::now_secs(),
        enabled: true,
        gaming: false,
        viewed: String::new(),
        workspace_active: false,
        qa_sessions: Default::default(),
        chief_session: Some(SESSION_A.into()),
        host_available: true,
    };
    let block = crate::queue::block(&reported, &context, false, false);
    assert_eq!(block, Some(crate::queue::QueueBlock::Chief));
    assert_eq!(block.unwrap().to_string(), "the Chief's own session");
}

#[test]
fn a_todo_with_a_leftover_chief_assignment_field_can_change_status() {
    let _fixture = Fixture::new();
    let mut todos = TodoList::load();
    let id = todos.add("runway", "/r/runway", "old chief work");
    todos
        .items
        .iter_mut()
        .find(|item| item.id() == id)
        .unwrap()
        .0
        .insert("chief_assignment".into(), json!("x"));
    todos.save();
    assert!(
        TodoList::load()
            .items
            .iter()
            .all(|item| !item.0.contains_key("chief_assignment")),
        "loading drops the removed field"
    );
    assert!(todos.set_status(&id, "done", SESSION_A));
    assert_eq!(TodoList::load().status_of(&id), "done");
}

#[test]
fn an_enabled_chief_allows_archived_resume() {
    let fixture = Fixture::new();
    team_host(&fixture, json!({"members":[]}), vec![]);
    fixture.write(
        "home/.claude/history.jsonl",
        &json!({"sessionId":SESSION_A,"project":"/r/runway","timestamp":1000,"display":"archived"})
            .to_string(),
    );
    let mut app = fixture.app();
    app.handle(Key::Char('H'), &mut FakeScreen::typing(&["0"]));
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit resume /r/runway {SESSION_A}")]
    );
}

#[test]
fn the_queue_waits_while_theo_games() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    app.gaming = || true;
    let id = app.todos.add("runway", "/r/runway", "job");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    assert!(fixture.calls("cockpit").is_empty());
    app.gaming = || false;
    app.queue_checked = 0.0;
    app.dispatch_queue();
    assert_eq!(fixture.wait_for_calls("cockpit", 1).len(), 1);
}

#[test]
fn an_idle_session_gets_its_projects_first_queued_item_by_priority() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    let normal = app.todos.add("runway", "/r/runway", "normal job");
    let urgent = app.todos.add("runway", "/r/runway", "urgent job");
    app.todos.change_priority(&urgent, 2);
    app.todos.toggle_queued(&normal);
    app.todos.toggle_queued(&urgent);
    app.dispatch_queue();
    let calls = fixture.wait_for_calls("cockpit", 1);
    assert_eq!(
        calls,
        [format!(
            "cockpit say {SESSION_A} urgent job (todo {urgent}: before working, run cockpit-todo start {urgent}; when this is done, run cockpit-todo done {urgent})"
        )]
    );
    assert_eq!(app.todos.status_of(&urgent), "doing");
}

#[test]
fn the_queue_gives_nothing_while_todo_queue_is_off() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    fixture.write("state/cockpit/settings.json", r#"{"todo-queue": false}"#);
    let id = app.todos.add("runway", "/r/runway", "build the list");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn the_queue_gives_design_items_to_the_designer_only() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    let design = design_item(&mut app, "runway", "/r/runway", "design the empty state");
    let code = app.todos.add("runway", "/r/runway", "build the list");
    app.todos.toggle_queued(&design);
    app.todos.toggle_queued(&code);
    app.dispatch_queue();
    assert!(fixture.wait_for_calls("cockpit", 1)[0].contains("build the list"));
    declare_role(&fixture, SESSION_A, "designer");
    app.todos.set_status(&code, "done", "");
    app.build();
    app.queue_checked = 0.0;
    app.dispatch_queue();
    assert!(fixture.wait_for_calls("cockpit", 2)[1].contains("design the empty state"));
}

#[test]
fn a_session_gets_one_queued_item_at_a_time() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    for text in ["first", "second"] {
        let id = app.todos.add("runway", "/r/runway", text);
        app.todos.toggle_queued(&id);
    }
    app.dispatch_queue();
    app.queue_checked = 0.0;
    app.dispatch_queue();
    assert_eq!(fixture.wait_for_calls("cockpit", 1).len(), 1);
}

#[test]
fn four_parallel_sessions_each_receive_a_different_queued_item() {
    let fixture = Fixture::new();
    let since = util::now() as i64 - 600;
    let session_ids = [
        SESSION_A,
        SESSION_B,
        "cccccccc-0000-4000-8000-000000000000",
        "dddddddd-0000-4000-8000-000000000000",
    ];
    let rows: Vec<Value> = session_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let checkout = format!("home/IdeaProjects/.parallel/runway/parallel-{index}");
            fixture.write(&format!("{checkout}/.git/HEAD"), "ref: refs/heads/main\n");
            let cwd = fixture.path(&format!("{checkout}/apps"));
            session(
                id,
                "waiting",
                "runway",
                &cwd.to_string_lossy(),
                since + index as i64,
            )
        })
        .collect();
    fixture.snapshot(json!(rows));
    let mut app = fixture.app();
    app.gaming = || false;
    let items: Vec<String> = (0..5)
        .map(|index| {
            let id = app
                .todos
                .add("runway", "/r/runway", &format!("job {index}"));
            app.todos.toggle_queued(&id);
            id
        })
        .collect();

    app.dispatch_queue();

    let calls = fixture.calls("cockpit");
    assert_eq!(calls.len(), 4, "{calls:?}");
    for session_id in session_ids {
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with(&format!("cockpit say {session_id} ")))
                .count(),
            1
        );
    }
    let assigned: Vec<_> = items
        .iter()
        .filter(|id| app.todos.status_of(id) == "doing")
        .collect();
    assert_eq!(assigned.len(), 4);
    for id in assigned {
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.contains(&format!("(todo {id}:")))
                .count(),
            1
        );
    }
    app.queue_checked = 0.0;
    app.dispatch_queue();
    assert_eq!(fixture.calls("cockpit").len(), 4);
}

#[test]
fn queued_items_created_in_a_parallel_clone_are_shared_with_the_project() {
    for session_path in [
        "home/IdeaProjects/runway",
        "home/IdeaProjects/.parallel/runway/second",
    ] {
        let fixture = Fixture::new();
        let item_checkout = "home/IdeaProjects/.parallel/runway/first";
        fixture.write(
            &format!("{item_checkout}/.git/HEAD"),
            "ref: refs/heads/main\n",
        );
        fixture.write(
            &format!("{session_path}/.git/HEAD"),
            "ref: refs/heads/main\n",
        );
        let cwd = fixture.path(&format!("{session_path}/apps"));
        fixture.snapshot(json!([session(
            SESSION_A,
            "waiting",
            "runway",
            &cwd.to_string_lossy(),
            util::now() as i64 - 600
        )]));
        let mut app = fixture.app();
        app.gaming = || false;
        let other_project = app.todos.add("runway", "/r/impostor", "wrong project");
        app.todos.change_priority(&other_project, 2);
        app.todos.toggle_queued(&other_project);
        let item_cwd = fixture.path(&format!("{item_checkout}/apps"));
        let id = app
            .todos
            .add("runway", &item_cwd.to_string_lossy(), "shared job");
        app.todos.toggle_queued(&id);

        app.dispatch_queue();

        assert_eq!(
            fixture.calls("cockpit"),
            [format!(
                "cockpit say {SESSION_A} shared job (todo {id}: before working, run cockpit-todo start {id}; when this is done, run cockpit-todo done {id})"
            )]
        );
        assert_eq!(app.todos.status_of(&other_project), "todo");
    }
}

#[test]
fn a_parallel_session_receives_queued_issues_from_the_projects_checkout() {
    let fixture = Fixture::new();
    let cwd = fixture.path("home/IdeaProjects/.parallel/runway/first");
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        &cwd.to_string_lossy(),
        util::now() as i64 - 600
    )]));
    let mut app = fixture.app();
    app.gaming = || false;
    app.reviews
        .set_issues(vec![issue(2, "Broken login", &["bug"])]);
    app.reviews
        .checkouts
        .insert("o/runway".into(), "/r/runway".into());
    app.todos.toggle_queued("o/runway#2");

    app.dispatch_queue();

    let calls = fixture.calls("cockpit");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with(&format!("cockpit say {SESSION_A} ")));
    assert!(calls[0].contains("(todo o/runway#2:"));
    assert_eq!(app.todos.status_of("o/runway#2"), "doing");
    app.queue_checked = 0.0;
    app.dispatch_queue();
    assert_eq!(fixture.calls("cockpit").len(), 1);
}

#[test]
fn a_session_idle_for_less_than_five_minutes_gets_nothing() {
    let (fixture, mut app) = queue_fixture("waiting", 60);
    let id = app.todos.add("runway", "/r/runway", "job");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn a_working_session_gets_nothing() {
    let (fixture, mut app) = queue_fixture("working", 600);
    let id = app.todos.add("runway", "/r/runway", "job");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn unqueued_items_and_other_projects_stay_put() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    app.todos.add("runway", "/r/runway", "not queued");
    let elsewhere = app.todos.add("homelab", "/r/homelab", "other project");
    app.todos.toggle_queued(&elsewhere);
    app.dispatch_queue();
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn q_queues_an_item_and_the_panel_says_so() {
    let (_fixture, mut app) = queue_fixture("working", 0);
    let id = app.todos.add("runway", "/r/runway", "job");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('Q'), &mut FakeScreen::default());
    assert!(TodoList::load().is_queued(&id));
    app.build();
    assert!(app.subtitle(3).ends_with("1 queued"), "{}", app.subtitle(3));
}

#[test]
fn a_done_item_leaves_the_queue() {
    let (_fixture, mut app) = queue_fixture("waiting", 600);
    let id = app.todos.add("runway", "/r/runway", "job");
    app.todos.toggle_queued(&id);
    app.todos.set_status(&id, "done", "");
    assert!(!TodoList::load().is_queued(&id));
}

#[test]
fn a_session_borrowing_a_projects_name_gets_none_of_its_queue() {
    let fixture = Fixture::new();
    let since = util::now() as i64 - 600;
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/impostor",
        since
    )]));
    let mut app = fixture.app();
    app.gaming = || false;
    let id = app.todos.add("runway", "/r/runway", "job");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn handed_work_starts_a_session_when_none_runs_in_its_project() {
    let fixture = Fixture::new();
    let (greywall, greyproxy) = greywall_and_greyproxy(&fixture);
    file_in_greyproxy(&greywall, "Accept a per-client secret", json!({}));
    let mut app = fixture.app();
    app.gaming = || false;
    app.todos.apply_updates();
    app.dispatch_queue();
    let calls = fixture.wait_for_calls("cockpit", 1);
    assert!(
        calls[0].starts_with(&format!(
            "cockpit --agent claude background {} A greywall session filed this",
            greyproxy.display()
        )),
        "{calls:?}"
    );
    assert_eq!(
        app.todos
            .status_of(&item_titled(&app.todos, "Accept a per-client secret").id()),
        "doing"
    );
}

#[test]
fn handed_work_starts_a_fresh_session_even_when_the_project_has_sessions() {
    for state in ["working", "waiting"] {
        let fixture = Fixture::new();
        let (greywall, greyproxy) = greywall_and_greyproxy(&fixture);
        fixture.snapshot(json!([session(
            SESSION_B,
            state,
            "greyproxy",
            &greyproxy.display().to_string(),
            util::now() as i64 - 600
        )]));
        file_in_greyproxy(&greywall, "Unblock the reporter", json!({}));
        let mut app = fixture.app();
        app.gaming = || false;
        app.todos.apply_updates();
        app.dispatch_queue();
        let calls = fixture.wait_for_calls("cockpit", 1);
        assert_eq!(calls.len(), 1, "{state}: {calls:?}");
        assert!(
            calls[0].starts_with(&format!(
                "cockpit --agent claude background {} ",
                greyproxy.display()
            )),
            "{state}: {calls:?}"
        );
        assert_eq!(
            app.todos
                .status_of(&item_titled(&app.todos, "Unblock the reporter").id()),
            "doing"
        );
    }
}

#[test]
fn handed_work_does_not_wait_for_the_ordinary_queue_check() {
    let fixture = Fixture::new();
    let (greywall, _) = greywall_and_greyproxy(&fixture);
    file_in_greyproxy(&greywall, "Unblock now", json!({}));
    let mut app = fixture.app();
    app.gaming = || false;
    app.todos.apply_updates();
    app.queue_checked = util::now();
    app.dispatch_queue();
    assert_eq!(fixture.wait_for_calls("cockpit", 1).len(), 1);
    assert_eq!(
        app.todos
            .status_of(&item_titled(&app.todos, "Unblock now").id()),
        "doing"
    );
}

#[test]
fn handed_work_starts_each_item_once_after_the_previous_launch_has_a_known_worker() {
    let fixture = Fixture::new();
    let (greywall, _) = greywall_and_greyproxy(&fixture);
    for title in ["First handoff", "Second handoff"] {
        file_in_greyproxy(&greywall, title, json!({}));
    }
    let mut app = fixture.app();
    app.gaming = || false;
    app.todos.apply_updates();
    app.dispatch_queue();
    let first = fixture.wait_for_calls("cockpit", 1);
    assert_eq!(
        first.len(),
        1,
        "unknown launch ownership must block another launch in this project"
    );
    let id = item_titled(&app.todos, "First handoff").id();
    assert!(app.todos.set_status(&id, "doing", SESSION_A));
    app.queue_checked = 0.0;
    app.dispatch_queue();
    let calls = fixture.wait_for_calls("cockpit", 2);
    assert_eq!(calls.len(), 2, "{calls:?}");
    for title in ["First handoff", "Second handoff"] {
        let id = item_titled(&app.todos, title).id();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.contains(&format!("(todo {id}:")))
                .count(),
            1
        );
        assert_eq!(app.todos.status_of(&id), "doing");
    }
    app.queue_checked = 0.0;
    app.dispatch_queue();
    assert_eq!(fixture.calls("cockpit").len(), 2);
}

#[test]
fn a_refused_handed_item_is_not_asked_for_again_on_every_tick() {
    let fixture = Fixture::new();
    let (greywall, _) = greywall_and_greyproxy(&fixture);
    file_in_greyproxy(&greywall, "Refused handoff", json!({}));
    let requests = refusing_scheduler(&fixture);
    let mut app = fixture.app();
    app.gaming = || false;
    app.todos.apply_updates();
    for _ in 0..5 {
        app.dispatch_queue();
    }
    assert_eq!(reserve_requests(&requests), 1);
    assert_eq!(
        app.todos
            .status_of(&item_titled(&app.todos, "Refused handoff").id()),
        "todo"
    );
}

#[test]
fn ticks_with_nothing_to_start_ask_the_daemon_for_no_snapshot() {
    let fixture = Fixture::new();
    let (greywall, _) = greywall_and_greyproxy(&fixture);
    file_in_greyproxy(&greywall, "Refused handoff", json!({}));
    let requests = refusing_scheduler(&fixture);
    let mut app = fixture.app();
    app.gaming = || false;
    app.todos.apply_updates();
    app.dispatch_queue();
    let snapshots = || {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["request"] == "snapshot")
            .count()
    };
    let before = snapshots();
    for _ in 0..5 {
        app.dispatch_queue();
    }
    assert_eq!(snapshots(), before);
}

#[test]
fn a_refused_handed_item_is_asked_for_again_at_the_next_queue_check() {
    let fixture = Fixture::new();
    let (greywall, _) = greywall_and_greyproxy(&fixture);
    file_in_greyproxy(&greywall, "Refused handoff", json!({}));
    let requests = refusing_scheduler(&fixture);
    let mut app = fixture.app();
    app.gaming = || false;
    app.todos.apply_updates();
    app.dispatch_queue();
    app.queue_checked = 0.0;
    app.dispatch_queue();
    assert_eq!(reserve_requests(&requests), 2);
}

#[test]
fn handed_work_preserves_queue_off_and_gaming_pauses() {
    for (enabled, gaming) in [(false, false), (true, true)] {
        let fixture = Fixture::new();
        let (greywall, _) = greywall_and_greyproxy(&fixture);
        fixture.write(
            "state/cockpit/settings.json",
            &json!({"todo-queue": enabled}).to_string(),
        );
        file_in_greyproxy(&greywall, "Paused handoff", json!({}));
        let mut app = fixture.app();
        app.gaming = if gaming { || true } else { || false };
        app.todos.apply_updates();
        app.dispatch_queue();
        assert_eq!(
            app.todos
                .status_of(&item_titled(&app.todos, "Paused handoff").id()),
            "todo"
        );
        assert!(fixture.calls("cockpit").is_empty());
    }
}

#[test]
fn handed_work_waits_for_its_dependencies_before_starting() {
    let fixture = Fixture::new();
    let (greywall, _) = greywall_and_greyproxy(&fixture);
    let mut app = fixture.app();
    app.gaming = || false;
    let dependency = app
        .todos
        .add("greywall", &greywall.display().to_string(), "Prerequisite");
    file_in_greyproxy(
        &greywall,
        "Dependent handoff",
        json!({"after": [dependency]}),
    );
    app.todos.apply_updates();
    app.dispatch_queue();
    assert!(fixture.calls("cockpit").is_empty());
    assert_eq!(
        app.todos
            .status_of(&item_titled(&app.todos, "Dependent handoff").id()),
        "todo"
    );
    app.todos.set_status(&dependency, "done", "");
    app.dispatch_queue();
    assert_eq!(fixture.wait_for_calls("cockpit", 1).len(), 1);
    assert_eq!(
        app.todos
            .status_of(&item_titled(&app.todos, "Dependent handoff").id()),
        "doing"
    );
}

#[test]
fn handed_blocker_report_starts_a_session_without_an_existing_one() {
    let fixture = Fixture::new();
    fs::create_dir_all(crate::paths::cockpit_work()).unwrap();
    report_blocked(&fixture, "opusline", "Run the tests");
    let mut app = fixture.app();
    app.gaming = || false;
    app.todos.apply_updates();
    app.queue_checked = util::now();
    app.dispatch_queue();
    let calls = fixture.wait_for_calls("cockpit", 1);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        calls[0].starts_with(&format!(
            "cockpit --agent claude background {} ",
            crate::paths::cockpit_work().display()
        )),
        "{calls:?}"
    );
    assert_eq!(
        app.todos
            .items
            .iter()
            .find(|item| item.is_blocked_report())
            .unwrap()
            .status(),
        "doing"
    );
}

#[test]
fn theos_own_queued_items_never_start_a_session() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    let id = app.todos.add("homelab", "/r/homelab", "bump postgres");
    app.todos.toggle_queued(&id);
    app.dispatch_queue();
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn a_todo_the_chief_asked_for_starts_a_session_even_when_nobody_queued_it() {
    let fixture = Fixture::new();
    let (_, greyproxy) = greywall_and_greyproxy(&fixture);
    let mut app = fixture.app();
    app.gaming = || false;
    let id = app.todos.add(
        "greyproxy",
        &greyproxy.display().to_string(),
        "Theo's own item",
    );
    chief_asks_for_a_session(&id, chrono::Utc::now().timestamp_millis());
    app.dispatch_queue();
    let calls = fixture.wait_for_calls("cockpit", 1);
    assert!(
        calls[0].starts_with(&format!(
            "cockpit --agent claude background {} ",
            greyproxy.display()
        )),
        "{calls:?}"
    );
    assert_eq!(app.todos.status_of(&id), "doing");
}

#[test]
fn a_lapsed_chief_request_starts_no_session() {
    let fixture = Fixture::new();
    let (_, greyproxy) = greywall_and_greyproxy(&fixture);
    let mut app = fixture.app();
    app.gaming = || false;
    let id = app.todos.add(
        "greyproxy",
        &greyproxy.display().to_string(),
        "Theo's own item",
    );
    chief_asks_for_a_session(
        &id,
        chrono::Utc::now().timestamp_millis() - crate::chief::SESSION_START_OPEN_MS - 1000,
    );
    app.dispatch_queue();
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.calls("cockpit").is_empty());
    assert_eq!(app.todos.status_of(&id), "todo");
}

#[test]
fn theos_q_marks_the_item_as_queued_by_him_and_taking_it_out_clears_the_mark() {
    let _fixture = Fixture::new();
    let mut todos = TodoList::load();
    let id = todos.add("homelab", "/r/homelab", "bump postgres");
    let is_marked = |todos: &TodoList| {
        todos
            .items
            .iter()
            .find(|item| item.id() == id)
            .unwrap()
            .is_queued_by_theo()
    };
    assert!(!is_marked(&todos));
    todos.toggle_queued(&id);
    assert!(is_marked(&todos));
    todos.toggle_queued(&id);
    assert!(!is_marked(&todos));
}
