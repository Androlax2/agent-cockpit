use super::*;

#[test]
fn repositories_come_from_the_sessions_git_origins_and_the_viewed_one_narrows_them() {
    let fixture = Fixture::new();
    for (name, origin) in [
        ("shop", "https://github.com/acme/shop.git"),
        ("notes", "git@github.com:acme/notes.git"),
    ] {
        let folder = fixture.path(&format!("repos/{name}"));
        fs::create_dir_all(&folder).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&folder)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(&folder)
            .args(["remote", "add", "origin", origin])
            .status()
            .unwrap();
    }
    let shop = fixture.path("repos/shop").display().to_string();
    let notes = fixture.path("repos/notes").display().to_string();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "shop", &shop, 1),
        session(SESSION_B, "waiting", "notes", &notes, 2)
    ]));
    assert_eq!(
        reviews::session_checkouts("")
            .into_keys()
            .collect::<Vec<_>>(),
        vec!["acme/notes".to_string(), "acme/shop".to_string()]
    );
    assert_eq!(
        reviews::session_checkouts(SESSION_A)
            .into_keys()
            .collect::<Vec<_>>(),
        vec!["acme/shop".to_string()]
    );
}

#[test]
fn a_turn_ending_in_the_panes_repository_fetches_its_pull_requests_again() {
    let fixture = Fixture::new();
    let mut app = reviews_fetched_while_a_shop_session_works(&fixture);
    app.reviews.age_fetch(31.0);
    end_the_shop_sessions_turn(&fixture);
    app.reviews.tick();
    assert_eq!(wait_for_fetch(&fixture, &app, 2), 2);
}

#[test]
fn a_turn_ending_right_after_a_fetch_refetches_once_the_fetch_is_30_seconds_old() {
    let fixture = Fixture::new();
    let mut app = reviews_fetched_while_a_shop_session_works(&fixture);
    end_the_shop_sessions_turn(&fixture);
    app.reviews.tick();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(wait_for_fetch(&fixture, &app, 1), 1);
    app.reviews.age_fetch(31.0);
    app.reviews.tick();
    assert_eq!(wait_for_fetch(&fixture, &app, 2), 2);
}

#[test]
fn without_a_turn_ending_the_pull_requests_wait_for_the_five_minute_refresh() {
    let fixture = Fixture::new();
    let mut app = reviews_fetched_while_a_shop_session_works(&fixture);
    app.reviews.age_fetch(31.0);
    app.reviews.tick();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(wait_for_fetch(&fixture, &app, 1), 1);
}

#[test]
fn switching_session_never_shows_the_previous_sessions_pull_requests() {
    let fixture = Fixture::new();
    shim_gh_slow_for_a(&fixture, "2");
    let mut app = fixture.app();
    app.reviews.repositories = vec!["o/b".into()];
    app.reviews.refresh();
    assert_eq!(wait_for_rows(&app)[0].repository, "b");
    app.reviews.repositories = vec!["o/a".into()];
    app.reviews.refresh();
    let (rows, _, loading) = app.reviews.current();
    assert!(rows.is_empty() && loading, "{rows:?}");
}

#[test]
fn a_fetch_still_running_for_another_session_does_not_hold_back_this_ones() {
    let fixture = Fixture::new();
    shim_gh_slow_for_a(&fixture, "0.5");
    let mut app = fixture.app();
    app.reviews.repositories = vec!["o/a".into()];
    app.reviews.refresh();
    app.reviews.repositories = vec!["o/b".into()];
    app.reviews.refresh();
    assert_eq!(
        wait_for_rows(&app)
            .iter()
            .map(|row| row.repository.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
}

#[test]
fn the_column_notices_another_session_in_the_pane_before_its_next_tick() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.tick();
    assert!(!app.viewed_changed());
    fixture.view(SESSION_B);
    assert!(app.viewed_changed());
}

#[test]
fn pull_requests_come_from_gh_without_bots_and_open_in_the_cockpit_window() {
    let fixture = Fixture::new();
    let recent = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let response = format!(
        "[{{\"number\": 3, \"title\": \"Fix\", \"url\": \"https://github.com/o/r/pull/3\", \"repository\": {{\"nameWithOwner\": \"o/r\"}}, \"author\": {{\"login\": \"sam\"}}, \"updatedAt\": \"{recent}\"}}, {{\"number\": 4, \"title\": \"Bump\", \"url\": \"u\", \"repository\": {{\"nameWithOwner\": \"o/r\"}}, \"author\": {{\"login\": \"app/dependabot\"}}, \"updatedAt\": \"{recent}\"}}]"
    );
    fixture.shim("gh", &format!("if [ \"$1 $2\" = 'pr view' ]; then echo '{response}' | jq -c --argjson number \"$3\" '.[] | select(.number == $number)'; else echo '{response}'; fi"));
    let (rows, error) = reviews::fetch_reviews(&[]);
    assert_eq!((rows.len(), error.as_str()), (1, ""));
    assert!(fixture.calls("gh")[0].contains("--review-requested=@me"));
    let mut app = fixture.app();
    app.reviews.refresh();
    for _ in 0..100 {
        if !app.reviews.current().0.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    app.build();
    app.focus_panel(2);
    app.handle(Key::Char('o'), &mut FakeScreen::default());
    assert_eq!(
        fixture.calls("hyprctl"),
        vec![
            "hyprctl -j activeworkspace".to_string(),
            r#"hyprctl eval OPEN_IN_SCRATCHPAD("prs", "https://github.com/o/r/pull/3")"#
                .to_string()
        ]
    );
}

#[test]
fn g_opens_pending_requests_in_the_greyproxy_window() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('G'), &mut FakeScreen::default());
    assert_eq!(
        fixture.calls("hyprctl"),
        vec![
            "hyprctl -j activeworkspace".to_string(),
            r#"hyprctl eval OPEN_IN_SCRATCHPAD("greyproxy", "http://localhost:43080/pending")"#
                .to_string()
        ]
    );
}

#[test]
fn bad_json_from_gh_gives_no_rows() {
    assert!(reviews::parse_reviews("not json", "o/r").is_empty());
    assert!(reviews::parse_issues("{}", "o/r").is_empty());
}

#[test]
fn ideas_show_proposals_from_every_project_without_changing_the_viewed_session() {
    let (_fixture, mut app) = ideas_fixture();
    open_ideas(&mut app);
    let rows = settings_screen_rows(&render(&mut app, 160, 45)).join("\n");
    for text in [
        "3  Ideas",
        "Show sandbox failures",
        "greywall",
        "Explain queued work",
        "agent-cockpit",
    ] {
        assert!(rows.contains(text), "missing {text}: {rows}");
    }
    assert!(!texts(&app, 4).join("\n").contains("Ordinary work"));
    assert_eq!(util::current_session(), SESSION_A);
    assert_eq!(app.todo_scope, "homelab");
}

#[test]
fn ideas_ignore_the_todo_kind_filter_without_changing_it() {
    let (_fixture, mut app) = ideas_fixture();
    app.focus_panel(3);
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
    open_ideas(&mut app);
    let rows = texts(&app, 4).join("\n");
    assert!(
        rows.contains("Show sandbox failures") && rows.contains("Explain queued work"),
        "{rows}"
    );
    assert!(app.subtitle(3).contains("kind: Bug"));
}

#[test]
fn ideas_show_the_source_session_and_reason_even_when_the_session_is_gone() {
    let (_fixture, mut app) = ideas_fixture();
    open_ideas(&mut app);
    let rows = texts(&app, 4).join("\n");
    for text in [
        "Check isolation",
        SESSION_B,
        OPENCODE_SESSION,
        "Failures should be visible",
        "Show what happens next",
    ] {
        assert!(rows.contains(text), "missing {text}: {rows}");
    }
    assert!(rows.contains("not running"), "{rows}");
}

#[test]
fn ideas_with_no_source_do_not_claim_to_come_from_the_viewed_session() {
    let (_fixture, mut app) = ideas_fixture();
    for item in &mut app.todos.items {
        item.0.remove("source");
    }
    app.build();
    open_ideas(&mut app);
    let rows = texts(&app, 4).join("\n");
    assert!(rows.contains("Unknown session"), "{rows}");
    assert!(
        !rows.contains(SESSION_A) && !rows.contains("Check isolation"),
        "{rows}"
    );
}

#[test]
fn ideas_details_do_not_attribute_a_missing_source_to_the_user() {
    let (_fixture, mut app) = ideas_fixture();
    for item in &mut app.todos.items {
        item.0.remove("source");
    }
    app.build();
    open_ideas(&mut app);
    app.handle(Key::Enter, &mut FakeScreen::default());
    let rows = settings_screen_rows(&render(&mut app, 160, 45)).join("\n");
    assert!(rows.contains("Unknown session"), "{rows}");
    assert!(
        rows.contains("Show sandbox failures") && rows.contains("Failures should be visible"),
        "{rows}"
    );
    assert!(!rows.contains("filed by you"), "{rows}");
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert_eq!(app.focus, 4);
}

#[test]
fn ideas_recognize_sources_running_outside_the_managed_session_list() {
    let (fixture, mut app) = ideas_fixture();
    let greywall = project_checkout(&fixture, "greywall");
    let mut reporter = external_session(
        SESSION_B,
        "waiting",
        "greywall",
        greywall.to_str().unwrap(),
        10,
    );
    reporter["label"] = json!("External reporter");
    fixture.snapshot(json!([reporter]));
    app.build();
    open_ideas(&mut app);
    let rows = texts(&app, 4).join("\n");
    assert!(rows.contains("External reporter"), "{rows}");
    assert!(
        !rows.contains(&format!("session {SESSION_B} · not running")),
        "{rows}"
    );
}

#[test]
fn ideas_approve_only_the_selected_proposal_without_dispatching_work() {
    let (fixture, mut app) = ideas_fixture();
    open_ideas(&mut app);
    app.handle(Key::Down, &mut FakeScreen::default());
    app.handle(Key::Char('y'), &mut FakeScreen::default());
    let stored = TodoList::load();
    assert_eq!(item_titled(&stored, "Explain queued work").status(), "todo");
    assert_eq!(
        item_titled(&stored, "Show sandbox failures").status(),
        "proposed"
    );
    app.build();
    assert!(!texts(&app, 4).join("\n").contains("Explain queued work"));
    assert!(
        fixture.calls("cockpit say").is_empty() && fixture.calls("cockpit background").is_empty()
    );
    assert!(app.todos.queued.is_empty());
}

#[test]
fn ideas_decline_only_the_selected_proposal_and_remember_the_decline() {
    let (fixture, mut app) = ideas_fixture();
    open_ideas(&mut app);
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    let stored = TodoList::load();
    assert!(
        stored
            .items
            .iter()
            .all(|item| item.text() != "Show sandbox failures")
    );
    assert_eq!(
        item_titled(&stored, "Explain queued work").status(),
        "proposed"
    );
    queue_update(
        json!({"action": "propose", "text": "Show sandbox failures", "why": "Again", "session": SESSION_B}),
        &project_checkout(&fixture, "greywall"),
    );
    app.todos.apply_updates();
    assert!(
        app.todos
            .items
            .iter()
            .all(|item| item.text() != "Show sandbox failures")
    );
    assert!(app.todos.queued.is_empty());
}

#[test]
fn ideas_show_an_empty_state_and_clear_the_counter_after_the_last_decision() {
    let (_fixture, mut app) = ideas_fixture();
    open_ideas(&mut app);
    app.handle(Key::Down, &mut FakeScreen::default());
    app.handle(Key::Char('y'), &mut FakeScreen::default());
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    app.build();
    let rows = settings_screen_rows(&render(&mut app, 160, 45));
    assert!(
        rows.iter().any(|row| row.contains("No ideas waiting")),
        "{rows:?}"
    );
    assert!(
        !rows[0].contains("? 1 idea") && !rows[0].contains("? 2 ideas"),
        "{}",
        rows[0]
    );
}

#[test]
fn ideas_layout_stays_within_short_and_narrow_terminal_buffers() {
    let (_fixture, mut app) = ideas_fixture();
    open_ideas(&mut app);
    for panel in 0..=4 {
        app.focus_panel(panel);
        for (width, height) in [(20, 8), (40, 12), (60, 24), (160, 45)] {
            let _ = render(&mut app, width, height);
            assert!(app.layout(height).iter().sum::<u16>() <= height.saturating_sub(2));
        }
    }
}

#[test]
fn palette_homes_pr_later_keeps_the_row_reachable_and_toggles_back() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.reviews.set_rows(reviews::parse_reviews(&json!([{"number":61,"title":"VAT","url":"https://github.com/acme/shop/pull/61","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","reviewDecision":"REVIEW_REQUIRED"}]).to_string(), "acme/shop"));
    app.build();
    app.focus_panel(2);
    let before = util::now_secs();
    app.handle(Key::Char('l'), &mut FakeScreen::default());
    app.redraw();
    let expiry = continuity_saved(&fixture)["deferrals"]["pr:acme/shop#61"]["expires_at"]
        .as_i64()
        .expect("saved deferral");
    assert!((before + 1200..=util::now_secs() + 1200).contains(&expiry));
    assert_eq!(app.panels[2].items, 1);
    assert!(texts_with_badges(&app, 2).join("\n").contains("Later"));
    app.handle(Key::Char('l'), &mut FakeScreen::default());
    assert!(
        continuity_saved(&fixture)["deferrals"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    assert!(
        buffer_rows(&render(&mut app, 100, 45))
            .join("\n")
            .contains("Task trail")
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn the_open_tab_lists_to_do_and_in_progress_items_but_not_proposed_or_done_ones() {
    let fixture = Fixture::new();
    propose(&fixture, "proposed item", "why");
    let mut app = fixture.app();
    app.todos.apply_updates();
    app.todos.add("p", "/p", "to do item");
    let doing = app.todos.add("p", "/p", "doing item");
    app.todos.set_status(&doing, "doing", "");
    let done = app.todos.add("p", "/p", "done item");
    app.todos.set_status(&done, "done", "");
    app.build();
    let mut shown: Vec<String> = app.todo_entries.iter().map(Entry::status).collect();
    shown.sort();
    assert_eq!(
        shown,
        ["doing", "todo"],
        "proposals wait in Ideas, not in Todo's Open tab"
    );
}

#[test]
fn the_proposed_tab_hints_lead_with_approve() {
    let fixture = Fixture::new();
    propose(&fixture, "proposed item", "why");
    let mut app = fixture.app();
    app.todos.apply_updates();
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char(']'), &mut FakeScreen::default());
    assert_eq!(
        app.todo_hints()[0].0,
        "y",
        "the Proposed tab leads with approve"
    );
}

#[test]
fn a_proposal_waits_in_its_own_tab_until_approved() {
    let fixture = Fixture::new();
    propose(&fixture, "cache the usage fetch", "it runs every minute");
    let mut app = fixture.app();
    app.todos.apply_updates();
    app.build();
    show_to_do_tab(&mut app);
    assert!(
        texts(&app, 3)
            .iter()
            .all(|line| !line.contains("cache the usage fetch")),
        "shown in To do before approval"
    );
    app.handle(Key::Char('['), &mut FakeScreen::default());
    assert!(
        texts(&app, 3)
            .iter()
            .any(|line| line.contains("cache the usage fetch"))
    );
    app.handle(Key::Char('y'), &mut FakeScreen::default());
    let item = TodoList::load()
        .items
        .into_iter()
        .find(|item| item.text() == "cache the usage fetch")
        .unwrap();
    assert_eq!(
        (
            item.status().as_str(),
            item.why().as_str(),
            item.notes().as_str()
        ),
        ("todo", "it runs every minute", "line one\nline two")
    );
}

#[test]
fn a_waiting_proposal_is_counted_on_the_ideas_tab_not_announced_in_todo() {
    let fixture = Fixture::new();
    propose(&fixture, "cache the usage fetch", "it runs every minute");
    let mut app = fixture.app();
    app.todos.apply_updates();
    app.build();
    show_to_do_tab(&mut app);
    assert!(
        texts(&app, 3)
            .iter()
            .all(|line| !line.contains("from Claude wait"))
    );
    let rows = settings_screen_rows(&render(&mut app, 100, 40));
    assert!(rows[1].contains("Ideas  1 "), "{}", rows[1]);
}

#[test]
fn the_proposed_tab_does_not_announce_its_own_items() {
    let fixture = Fixture::new();
    propose(&fixture, "cache the usage fetch", "it runs every minute");
    let mut app = fixture.app();
    app.todos.apply_updates();
    app.focus_panel(3);
    app.handle(Key::Char(']'), &mut FakeScreen::default());
    assert_eq!(app.todo_tab, 1);
    assert!(
        texts(&app, 3)
            .iter()
            .all(|line| !line.contains("from Claude wait"))
    );
}

#[test]
fn the_ideas_tab_badge_counts_waiting_proposals_and_status_still_opens_from_the_header() {
    let (_fixture, mut app) = ideas_fixture();
    let rows = settings_screen_rows(&render(&mut app, 160, 45));
    assert!(rows[1].contains("Ideas  2 "), "{}", rows[1]);
    render(&mut app, 160, 45);
    app.focus_panel(4);
    let (start, _, action) = app.hit_actions[&0];
    assert_eq!(action, "status");
    app.handle(
        Key::Click {
            x: start,
            y: 0,
            double: false,
        },
        &mut FakeScreen::default(),
    );
    assert_eq!(app.focus, 0);
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert_eq!(app.focus, 4);
}

#[test]
fn a_waiting_proposal_draws_its_tab_as_a_magenta_chip() {
    let fixture = Fixture::new();
    propose(&fixture, "cache the usage fetch", "it runs every minute");
    let mut app = fixture.app();
    app.todos.apply_updates();
    app.build();
    app.focus = 3;
    let buffer = render(&mut app, 160, 40);
    let on_magenta: String = (2..40)
        .flat_map(|y| (0..160).map(move |x| (x, y)))
        .filter(|position| {
            buffer[*position].bg == ratatui::style::Color::Indexed(crate::app::MAGENTA)
        })
        .map(|position| buffer[position].symbol().to_string())
        .collect();
    assert_eq!(
        on_magenta, " Proposed 1 ",
        "below the tabs row, whose Ideas badge is magenta too"
    );
}

#[test]
fn a_declined_proposal_is_not_filed_again() {
    let fixture = Fixture::new();
    propose(&fixture, "Rewrite in Go", "faster");
    let mut app = fixture.app();
    app.todos.apply_updates();
    app.build();
    show_to_do_tab(&mut app);
    app.handle(Key::Char('['), &mut FakeScreen::default());
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    propose(&fixture, "rewrite  in go", "still faster");
    app.todos.apply_updates();
    assert!(TodoList::load().items.is_empty());
}

#[test]
fn a_session_cannot_turn_a_proposal_into_work() {
    let fixture = Fixture::new();
    propose(&fixture, "add telemetry", "insight");
    let mut todos = TodoList::load();
    todos.apply_updates();
    let id = todos.items[0].id();
    assert!(!todos.set_status(&id, "doing", "s1"));
}

#[test]
fn enter_reviews_a_pull_request_with_the_saved_default() {
    let fixture = Fixture::new();
    fixture.write(
        "state/cockpit/settings.json",
        r#"{"default-agent":"opencode"}"#,
    );
    let mut app = fixture.app();
    app.reviews
        .checkouts
        .insert("acme/shop".into(), "/r/shop".into());
    app.review_rows = vec![Review {
        number: 42,
        repository: "shop".into(),
        full_repository: "acme/shop".into(),
        ..Default::default()
    }];
    app.focus_panel(2);
    app.panels[2].selected = 0;
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec!["cockpit --agent opencode review /r/shop 42".to_string()]
    );
}

#[test]
fn enter_reviews_a_pull_request_of_a_mixed_case_repository_in_its_checkout() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.reviews
        .checkouts
        .insert("Acme/Shop".into(), "/r/shop".into());
    app.review_rows = reviews::parse_reviews(
        r#"[{"number": 61, "title": "VAT", "url": "https://github.com/Acme/Shop/pull/61"}]"#,
        "Acme/Shop",
    );
    app.focus_panel(2);
    app.panels[2].selected = 0;
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec!["cockpit --agent claude review /r/shop 61".to_string()]
    );
}

#[test]
fn i_opens_an_ideas_details_like_enter() {
    let (_fixture, mut app) = ideas_fixture();
    open_ideas(&mut app);
    app.handle(Key::Enter, &mut FakeScreen::default());
    let by_enter = settings_screen_rows(&render(&mut app, 160, 45)).join("\n");
    app.handle(Key::Esc, &mut FakeScreen::default());
    let list = settings_screen_rows(&render(&mut app, 160, 45)).join("\n");
    assert_ne!(list, by_enter, "Enter opens details");
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    assert_eq!(
        settings_screen_rows(&render(&mut app, 160, 45)).join("\n"),
        by_enter
    );
}
