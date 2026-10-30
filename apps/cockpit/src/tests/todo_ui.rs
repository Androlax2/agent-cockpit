use super::*;

#[test]
fn integrated_native_form_holds_the_reload_lock_until_save_or_cancel() {
    let fixture = Fixture::new();
    fixture.view(SESSION_A);
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.focus_panel(3);
    for edit in [false, true] {
        app.handle(
            Key::Char(if edit { 'e' } else { 't' }),
            &mut FakeScreen::default(),
        );
        let lock = fs::File::open(fixture.path("run/cockpit-todo-editor.lock")).unwrap();
        assert!(
            lock.try_lock().is_err(),
            "reload must wait for the entire native draft lifetime"
        );
        type_in_app(&mut app, "kept title");
        if edit {
            app.handle(Key::Esc, &mut FakeScreen::default());
        } else {
            app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
        }
        assert!(
            lock.try_lock().is_ok(),
            "finished draft leaked the reload lock"
        );
    }
}

#[test]
fn integrated_native_drafts_recover_add_and_edit_with_original_metadata_once() {
    let fixture = Fixture::new();
    fixture.view(SESSION_A);
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    type_in_app(&mut app, "Recovered 界 draft");
    for key in [Key::BackTab, Key::Right, Key::Right, Key::Tab, Key::Tab] {
        app.handle(key, &mut FakeScreen::default());
    }
    type_in_app(&mut app, "first\nsecond");
    assert!(TodoList::load().items.is_empty());
    drop(app);
    fixture.view(SESSION_B);
    let mut recovered = fixture.app();
    assert_eq!(
        recovered.todos.items.len(),
        1,
        "native typing was not checkpointed for recovery"
    );
    let item = &recovered.todos.items[0];
    let id = item.id();
    assert_eq!(
        (
            item.project(),
            item.cwd(),
            item.text(),
            item.notes(),
            item.category()
        ),
        (
            "runway".into(),
            "/r/runway".into(),
            "Recovered 界 draft".into(),
            "first\nsecond".into(),
            Some(crate::todo::Category::Bug)
        )
    );
    recovered.todos.items[0]
        .0
        .insert("category".into(), json!("future-kind"));
    recovered.todos.items[0]
        .0
        .insert("status".into(), json!("proposed"));
    recovered.todos.items[0]
        .0
        .insert("kind".into(), json!("design"));
    recovered.todos.items[0]
        .0
        .insert("source".into(), json!("session origin"));
    recovered.todos.items[0]
        .0
        .insert("why".into(), json!("keep reason"));
    recovered.todos.save();
    recovered.build();
    recovered.focus_panel(3);
    recovered.handle(Key::Char(']'), &mut FakeScreen::default());
    recovered.handle(Key::Char('e'), &mut FakeScreen::default());
    type_in_app(&mut recovered, " edited");
    drop(recovered);
    let reloaded = fixture.app();
    assert_eq!(reloaded.todos.items.len(), 1);
    let item = &reloaded.todos.items[0];
    assert_eq!(
        (
            item.id(),
            item.text(),
            item.status(),
            item.source(),
            item.why()
        ),
        (
            id,
            "Recovered 界 draft edited".into(),
            "proposed".into(),
            "session origin".into(),
            "keep reason".into()
        )
    );
    assert_eq!(item.0["category"], "future-kind");
    assert!(item.is_design());
    assert_eq!(fixture.app().todos.items.len(), 1);
}

#[test]
fn integrated_native_cancelled_drafts_are_not_recovered_after_restart() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "must never become a TODO");
    let drafts = fixture.path("state/cockpit/todo-drafts");
    assert!(
        drafts.is_dir() && fs::read_dir(&drafts).unwrap().count() > 0,
        "native draft lacks a durable checkpoint"
    );
    app.handle(Key::Esc, &mut FakeScreen::default());
    drop(app);
    assert!(fixture.app().todos.items.is_empty());
    assert_eq!(fs::read_dir(drafts).unwrap().count(), 0);
}

#[test]
fn reviewed_cancellation_cleanup_failure_cannot_reactivate_add_or_edit_after_input() {
    for edit in [true, false] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        if edit {
            app.todos.add("p", "/p", "original title");
            app.build();
        }
        app.focus_panel(3);
        app.handle(
            Key::Char(if edit { 'e' } else { 't' }),
            &mut FakeScreen::typing(&["/p"]),
        );
        type_in_app(&mut app, " cancelled text");
        let directory = fixture.path("state/cockpit/todo-drafts");
        let buffer = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "md"))
            .unwrap();
        fs::remove_file(&buffer).unwrap();
        fs::create_dir(&buffer).unwrap();
        let before = TodoList::load().items;
        app.handle(Key::Esc, &mut FakeScreen::default());
        assert!(buffer_text(&render(&mut app, 60, 30)).contains("Cannot discard draft"));
        for key in [
            Key::Tab,
            Key::BackTab,
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
        ] {
            app.handle(key, &mut FakeScreen::default());
        }
        type_in_app(&mut app, " must stay cancelled");
        app.handle(
            Key::Paste("pasted after cancel".into()),
            &mut FakeScreen::default(),
        );
        drop(app);
        let restarted = fixture.app();
        assert_eq!(
            restarted.todos.items,
            before,
            "cancelled {} was recovered after ordinary input",
            if edit { "edit" } else { "addition" }
        );
    }
}

#[test]
fn reviewed_cancellation_cleanup_allows_only_esc_retry_for_add_and_edit() {
    for edit in [true, false] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        if edit {
            app.todos.add("p", "/p", "original title");
            app.build();
        }
        app.focus_panel(3);
        app.handle(
            Key::Char(if edit { 'e' } else { 't' }),
            &mut FakeScreen::typing(&["/p"]),
        );
        type_in_app(&mut app, " cancelled text");
        let directory = fixture.path("state/cockpit/todo-drafts");
        let buffer = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "md"))
            .unwrap();
        fs::remove_file(&buffer).unwrap();
        fs::create_dir(&buffer).unwrap();
        let before = TodoList::load().items;
        app.handle(Key::Esc, &mut FakeScreen::default());
        for key in [
            Key::Tab,
            Key::Right,
            Key::Char('x'),
            Key::Ctrl('s'),
            Key::Enter,
        ] {
            app.handle(key, &mut FakeScreen::default());
        }
        assert_eq!(
            TodoList::load().items,
            before,
            "save must not consume a cancelled draft"
        );
        let lock = fs::File::open(fixture.path("run/cockpit-todo-editor.lock")).unwrap();
        assert!(
            lock.try_lock().is_err(),
            "cleanup still owns the reload guard"
        );
        fs::remove_dir(&buffer).unwrap();
        app.handle(Key::Esc, &mut FakeScreen::default());
        assert!(
            lock.try_lock().is_ok(),
            "Esc retry did not finish cleanup after removing the obstruction"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        drop(lock);
        drop(app);
        assert_eq!(fixture.app().todos.items, before);
    }
}

#[test]
fn integrated_native_checkpoint_failures_keep_the_latest_text_for_retry() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "first");
    let metadata = fs::read_dir(fixture.path("state/cockpit/todo-drafts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let backup = metadata.with_extension("backup");
    fs::rename(&metadata, &backup).unwrap();
    fs::create_dir(&metadata).unwrap();
    type_in_app(&mut app, " latest");
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert!(TodoList::load().items.is_empty());
    assert!(buffer_text(&render(&mut app, 60, 30)).contains("first latest"));
    fs::remove_dir(&metadata).unwrap();
    fs::rename(backup, metadata).unwrap();
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(TodoList::load().items[0].text(), "first latest");
}

#[test]
fn integrated_native_durable_save_closes_even_if_retired_buffer_cleanup_fails() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "durably saved");
    let buffer = fs::read_dir(fixture.path("state/cockpit/todo-drafts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "md"))
        .unwrap();
    fs::remove_file(&buffer).unwrap();
    fs::create_dir(&buffer).unwrap();
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(TodoList::load().items[0].text(), "durably saved");
    let lock = fs::File::open(fixture.path("run/cockpit-todo-editor.lock")).unwrap();
    assert!(
        lock.try_lock().is_ok(),
        "durable save left an editable draft under an already-consumed token"
    );
    drop(lock);
    assert_eq!(
        app.todo_entries[app.panels[3].selected].key(),
        TodoList::load().items[0].id()
    );
    assert_eq!(fixture.app().todos.items.len(), 1);
}

#[test]
fn integrated_failed_native_save_keeps_all_in_memory_list_state_for_retry() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.issues.insert(
        "owner/project#1".into(),
        json!({"status": "done", "updated": 0}),
    );
    app.todos.queued.insert("owner/project#1".into());
    let before = app.todos.clone();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "retry safely");
    fs::create_dir_all(crate::paths::todo()).unwrap();
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(app.todos.items, before.items);
    assert_eq!(
        app.todos.issues, before.issues,
        "failed persistence pruned live issue state"
    );
    assert_eq!(
        app.todos.queued, before.queued,
        "failed persistence pruned the queue"
    );
}

#[test]
fn integrated_recovery_keeps_stale_edits_instead_of_overwriting_a_newer_draft() {
    let fixture = Fixture::new();
    let mut first = fixture.app();
    first.todos.add("p", "/p", "original");
    first.build();
    first.focus_panel(3);
    first.handle(Key::Char('e'), &mut FakeScreen::default());
    type_in_app(&mut first, " stale");
    let mut second = fixture.app();
    second.focus_panel(3);
    second.handle(Key::Char('e'), &mut FakeScreen::default());
    type_in_app(&mut second, " accepted");
    second.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    drop(first);
    drop(second);
    let recovered = fixture.app();
    assert_eq!(recovered.todos.items[0].text(), "original accepted");
    assert!(recovered.message.contains("drafts kept"));
    assert!(
        fs::read_dir(fixture.path("state/cockpit/todo-drafts"))
            .unwrap()
            .count()
            > 0
    );
}

#[test]
fn integrated_recovery_reads_legacy_metadata_and_ignores_cancelled_native_residue() {
    let fixture = Fixture::new();
    let draft = crate::todo_editor::Draft::create("legacy-project", "/legacy", None).unwrap();
    let directory = fixture.path("state/cockpit/todo-drafts");
    let metadata_path = directory.join(format!("{}.json", draft.token));
    let mut metadata: Value = serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata.as_object_mut().unwrap().remove("native");
    metadata.as_object_mut().unwrap().remove("cancelled");
    fs::write(&metadata_path, metadata.to_string()).unwrap();
    fs::write(
        directory.join(format!("{}.md", draft.token)),
        "Legacy title\n\nLegacy notes\n",
    )
    .unwrap();
    let mut recovered = fixture.app();
    assert_eq!(
        (
            recovered.todos.items[0].project(),
            recovered.todos.items[0].text(),
            recovered.todos.items[0].notes()
        ),
        (
            "legacy-project".into(),
            "Legacy title".into(),
            "Legacy notes".into()
        )
    );
    recovered.focus_panel(3);
    recovered.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut recovered, "cancelled residue");
    drop(recovered);
    let path = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let mut metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["cancelled"] = json!(true);
    fs::write(path, metadata.to_string()).unwrap();
    assert_eq!(fixture.app().todos.items.len(), 1);
    assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
}

#[test]
fn reviewed_github_rows_keep_identity_title_and_queue_marker_at_narrow_widths() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for repository in ["a", "example-project-with-a-long-name"] {
        let mut issue = issue(123, "Recognizable issue title", &[]);
        issue.repository = repository.into();
        issue.full_repository = format!("owner/{repository}");
        let key = issue.key();
        app.reviews.set_issues(vec![issue]);
        app.todos.queued.insert(key.clone());
        app.build();
        app.focus_panel(3);
        for width in [40, 60] {
            let buffer = render(&mut app, width, 40);
            let row = *app
                .hit_lines
                .iter()
                .find(|(_, value)| **value == (3, Some(0)))
                .unwrap()
                .0;
            let rendered: String = (0..width)
                .map(|column| buffer[(column, row)].symbol())
                .collect();
            assert!(
                rendered.contains("#123")
                    && rendered.contains("Recogniz")
                    && rendered.contains('Q'),
                "{repository} at {width}: {rendered}"
            );
        }
    }
}

#[test]
fn reviewed_cross_session_saves_preserve_viewed_preferences_and_reveal_the_original_project() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "working", "project-a", "/r/a", 1),
        session(SESSION_B, "working", "project-b", "/r/b", 2)
    ]));
    for (edit, escape) in [(false, false), (false, true), (true, false), (true, true)] {
        fixture.view(SESSION_B);
        let mut app = fixture.app();
        app.focus_panel(3);
        app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
        for _ in 0..3 {
            app.handle(Key::Char(']'), &mut FakeScreen::default());
        }
        fixture.view(SESSION_A);
        app.build();
        app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Feature"]));
        let original = app.todos.add("project-a", "/r/a", "original title");
        app.todos
            .set_category(&original, crate::todo::Category::Feature);
        app.todos.set_status(&original, "doing", "");
        app.build();
        app.handle(
            Key::Char(if edit { 'e' } else { 't' }),
            &mut FakeScreen::default(),
        );
        type_in_app(&mut app, " saved draft");
        if !edit {
            // Initial Task differs from A's Feature filter and B's Bug filter.
            assert!(app.subtitle(3).contains("kind: Feature"));
        }
        fixture.view(SESSION_B);
        app.build();
        let mut other_issue = issue(42, "unrelated B issue", &[]);
        other_issue.repository = "project-b".into();
        other_issue.full_repository = "owner/project-b".into();
        app.reviews.set_issues(vec![other_issue]);
        app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
        assert_eq!(
            util::current_session(),
            SESSION_B,
            "save must not switch the viewed session"
        );
        assert_eq!(app.todo_scope, "project-a");
        assert!(
            app.todo_entries
                .iter()
                .all(|entry| matches!(entry, Entry::Typed(item) if item.project() == "project-a")),
            "the temporary project must not borrow B's GitHub rows"
        );
        let saved = &app.todo_entries[app.panels[3].selected];
        assert!(
            matches!(saved, Entry::Typed(item) if item.project() == "project-a" && item.text().ends_with("saved draft"))
        );
        assert_eq!(app.todo_tab, if edit { 0 } else { 2 });
        assert!(
            app.subtitle(3).contains("saved: project-a"),
            "{}",
            app.subtitle(3)
        );
        let narrow = buffer_text(&render(&mut app, 40, 40));
        assert!(
            narrow.contains("saved: project-a"),
            "temporary project must remain explicit at 40 columns: {narrow}"
        );
        render(&mut app, 60, 40);
        app.build();
        assert_eq!(
            app.todo_scope, "project-a",
            "ordinary refresh must retain the saved-item scope"
        );
        // Esc explicitly returns to B without changing B's tab/filter.
        if escape {
            app.handle(Key::Esc, &mut FakeScreen::default());
            assert_eq!((app.todo_scope.as_str(), app.todo_tab), ("project-b", 3));
            assert!(app.subtitle(3).contains("kind: Bug"));
        }
        assert!(
            fixture
                .calls("tmux")
                .iter()
                .all(|call| !call.contains("select-pane")
                    && !call.contains("switch-client")
                    && !call.contains("select-window"))
        );
        // A subsequent deliberate view change must also expire any temporary scope.
        fixture.view(SESSION_A);
        app.build();
        assert_eq!(app.todo_tab, if edit { 0 } else { 2 });
        assert_eq!(app.subtitle(3).contains("kind: Feature"), edit);
        assert!(!app.subtitle(3).contains("saved:"));
        fixture.view(SESSION_B);
        app.build();
        assert_eq!(app.todo_tab, 3);
        assert!(app.subtitle(3).contains("kind: Bug"));
    }
}

#[test]
fn reviewed_saved_project_scope_does_not_change_queue_dispatch() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    fixture.snapshot(json!([
        session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            util::now() as i64 - 600
        ),
        session(SESSION_B, "working", "other-project", "/r/other", 2)
    ]));
    let queued = app.todos.add("runway", "/r/runway", "queued runway work");
    app.todos.toggle_queued(&queued);
    fixture.view(SESSION_B);
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    type_in_app(&mut app, "other project draft");
    fixture.view(SESSION_A);
    app.build();
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(app.todo_scope, "other-project");
    assert!(!app.todo_entries.iter().any(|entry| entry.key() == queued));
    app.dispatch_queue();
    assert!(fixture.wait_for_calls("cockpit", 1)[0].contains("queued runway work"));
    assert_eq!(TodoList::load().status_of(&queued), "doing");
}

#[test]
fn todo_composer_add_saves_one_complete_item_without_external_editor_or_picker() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.focus_panel(3);
    let screen = &mut FakeScreen::default();
    app.handle(Key::Char('t'), screen);
    let form = buffer_text(&render(&mut app, 60, 30));
    for expected in [
        "Add a todo · runway",
        "KIND",
        "Task",
        "Feature",
        "Bug",
        "Improvement",
        "Chore",
        "Research",
        "TITLE",
        "NOTES (optional)",
        "save",
        "next field",
        "kind",
        "image",
        "cancel",
    ] {
        assert!(form.contains(expected), "{expected}: {form}");
    }
    assert!(TodoList::load().items.is_empty());
    type_in_app(&mut app, "Fix café 界 qtdjk123?");
    app.handle(Key::BackTab, screen);
    app.handle(Key::Right, screen);
    app.handle(Key::Right, screen);
    app.handle(Key::Tab, screen);
    app.handle(Key::Tab, screen);
    type_in_app(&mut app, "first");
    app.handle(Key::Enter, screen);
    type_in_app(&mut app, "second");
    app.build();
    render(&mut app, 40, 12);
    render(&mut app, 100, 40);
    assert!(
        TodoList::load().items.is_empty(),
        "refresh must not persist a draft"
    );
    app.handle(Key::Ctrl('s'), screen);
    app.handle(Key::Ctrl('s'), screen);
    let items = TodoList::load().items;
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].text(), items[0].notes(), items[0].category()),
        (
            "Fix café 界 qtdjk123?".into(),
            "first\nsecond".into(),
            Some(crate::todo::Category::Bug)
        )
    );
    assert_eq!(
        app.todo_entries[app.panels[3].selected].key(),
        items[0].id()
    );
    assert_eq!(app.todo_tab, 2);
    assert!(
        fixture
            .calls("tmux")
            .iter()
            .all(|call| !call.contains("display-popup"))
    );
    assert!(fixture.calls("fzf").is_empty());
}

#[test]
fn todo_composer_cancel_preserves_disk_filters_queue_and_proposal_metadata() {
    let fixture = Fixture::new();
    fixture.write("state/cockpit/todo.json", &json!({"items": [
        {"id": "0123abcd", "text": "original", "project": "p", "category": "future-kind", "status": "proposed", "kind": "design", "why": "reason", "source": "session s1", "notes": "old"}
    ], "queued": ["0123abcd"]}).to_string());
    let mut app = fixture.app();
    app.focus_panel(3);
    let screen = &mut FakeScreen::default();
    app.handle(Key::Char(']'), screen);
    let before = fs::read(crate::paths::todo()).unwrap();
    app.handle(Key::Char('e'), screen);
    type_in_app(&mut app, " changed");
    app.handle(Key::Esc, screen);
    assert_eq!(fs::read(crate::paths::todo()).unwrap(), before);
    app.handle(Key::Char('e'), screen);
    type_in_app(&mut app, " edited");
    app.handle(Key::Ctrl('s'), screen);
    let item = &TodoList::load().items[0];
    assert_eq!(item.text(), "original edited");
    assert_eq!(item.0["category"], "future-kind");
    assert_eq!(
        (item.status(), item.why(), item.source(), item.notes()),
        (
            "proposed".into(),
            "reason".into(),
            "session s1".into(),
            "old".into()
        )
    );
    assert!(item.is_design() && TodoList::load().is_queued("0123abcd"));
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "unsaved");
    app.handle(Key::Esc, screen);
    assert!(app.subtitle(3).contains("kind: Bug"));
    assert_eq!(TodoList::load().items.len(), 1);
}

#[test]
fn todo_composer_validates_title_and_edits_unicode_at_the_cursor() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    let screen = &mut FakeScreen::default();
    app.handle(Key::Ctrl('s'), screen);
    let form = buffer_text(&render(&mut app, 60, 30));
    assert!(form.contains("Title is required"), "{form}");
    assert!(TodoList::load().items.is_empty());
    type_in_app(&mut app, "a界b");
    app.handle(Key::Left, screen);
    app.handle(Key::Backspace, screen);
    type_in_app(&mut app, "é");
    app.handle(Key::Tab, screen);
    type_in_app(&mut app, "one\ntwo");
    app.handle(Key::Ctrl('s'), screen);
    let item = &TodoList::load().items[0];
    assert_eq!(
        (item.text(), item.notes()),
        ("aéb".into(), "one\ntwo".into())
    );
}

#[test]
fn todo_composer_paste_is_text_even_when_it_contains_shortcuts_and_controls() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    app.handle(
        Key::Paste("qtdjk123\n界\tfin".into()),
        &mut FakeScreen::default(),
    );
    app.handle(Key::Tab, &mut FakeScreen::default());
    app.handle(
        Key::Paste("first\nsecond\tlast\u{1b}\u{13}".into()),
        &mut FakeScreen::default(),
    );
    assert!(TodoList::load().items.is_empty());
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    let item = &TodoList::load().items[0];
    assert_eq!(
        (item.text(), item.notes()),
        ("qtdjk123 界 fin".into(), "first\nsecond last".into())
    );
}

#[test]
fn todo_composer_pastes_a_clipboard_image_as_a_marker_in_the_notes() {
    let fixture = Fixture::new();
    fixture.shim("wl-paste", CLIPBOARD_PNG);
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "Fix the header");
    app.handle(Key::Tab, &mut FakeScreen::default());
    type_in_app(&mut app, "Looks like this: ");
    app.handle(Key::Ctrl('v'), &mut FakeScreen::default());
    type_in_app(&mut app, " and then ");
    app.handle(Key::Ctrl('v'), &mut FakeScreen::default());
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    let item = &TodoList::load().items[0];
    assert_eq!(
        item.notes(),
        "Looks like this: [Image 1] and then [Image 2]"
    );
    let images = item.images();
    assert_eq!(images.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
    for file in images.values() {
        assert!(
            fs::read(crate::paths::todo_images().join(file))
                .unwrap()
                .starts_with(b"\x89PNG\r\n\x1a\n")
        );
    }
}

#[test]
fn todo_composer_drops_an_image_whose_marker_was_deleted() {
    let fixture = Fixture::new();
    fixture.shim("wl-paste", CLIPBOARD_PNG);
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "Fix the header");
    app.handle(Key::Ctrl('v'), &mut FakeScreen::default());
    for _ in 0.."[Image 1]".len() {
        app.handle(Key::Backspace, &mut FakeScreen::default());
    }
    type_in_app(&mut app, "no image after all");
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    let item = &TodoList::load().items[0];
    assert_eq!(item.notes(), "no image after all");
    assert!(item.images().is_empty() && !item.0.contains_key("images"));
}

#[test]
fn todo_composer_reports_a_clipboard_without_an_image() {
    let fixture = Fixture::new();
    fixture.shim(
        "wl-paste",
        "case $1 in --list-types) echo text/plain ;; *) echo secret ;; esac",
    );
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    app.handle(Key::Ctrl('v'), &mut FakeScreen::default());
    let form = buffer_text(&render(&mut app, 60, 30));
    assert!(form.contains("No image in the clipboard"), "{form}");
    assert!(
        fixture
            .calls("wl-paste")
            .iter()
            .all(|call| call.contains("--list-types")),
        "text is never read"
    );
    assert!(!crate::paths::todo_images().exists());
}

#[test]
fn a_todo_with_images_tells_the_session_where_to_read_them() {
    let item = crate::todo::Item(json!({"id": "0123abcd", "text": "Fix the header", "images": {"1": "0123456789abcdef.png"}}).as_object().unwrap().clone());
    let prompt = crate::todo::prompt_of(&Entry::Typed(item));
    assert!(
        prompt.starts_with("Fix the header") && prompt.contains("cockpit-todo show 0123abcd"),
        "{prompt}"
    );
}

#[test]
fn saving_the_list_removes_old_images_nothing_refers_to() {
    let _fixture = Fixture::new();
    let folder = crate::paths::todo_images();
    fs::create_dir_all(&folder).unwrap();
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    for name in [
        "0000000000000001.png",
        "0000000000000002.png",
        "0000000000000003.png",
        "keep-me.txt",
    ] {
        fs::File::create(folder.join(name))
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
    }
    fs::write(folder.join("0000000000000004.png"), "pasted just now").unwrap();
    let mut todos = TodoList::load();
    todos.items.push(crate::todo::Item(
        json!({"id": "0123abcd", "text": "t", "images": {"1": "0000000000000001.png"}})
            .as_object()
            .unwrap()
            .clone(),
    ));
    todos.try_save().unwrap();
    let mut left: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "0000000000000001.png",
            "0000000000000004.png",
            "keep-me.txt"
        ]
    );
}

#[test]
fn todo_composer_keeps_focus_and_draft_at_all_terminal_sizes() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, &"Long 界 title ".repeat(30));
    app.handle(Key::Tab, &mut FakeScreen::default());
    type_in_app(&mut app, &"long note 界\n".repeat(80));
    for (width, height) in [
        (40, 14),
        (60, 30),
        (100, 40),
        (130, 50),
        (20, 8),
        (1, 1),
        (0, 0),
    ] {
        let buffer = render(&mut app, width, height);
        if width >= 32 {
            // The selected kind chip is blue too: the cursor is the one blue cell that is not a chip's.
            let chips = ["Task", "Feature", "Bug", "Improvement", "Chore", "Research"];
            let cursor_cells: Vec<_> = (0..height)
                .flat_map(|row| (0..width).map(move |column| (column, row)))
                .filter(|(column, row)| {
                    buffer[(*column, *row)].bg == crate::app::color(crate::app::BLUE)
                })
                .filter(|(_, row)| {
                    let line: String = (0..width)
                        .map(|column| buffer[(column, *row)].symbol())
                        .collect();
                    !chips
                        .iter()
                        .filter(|chip| line.contains(*chip))
                        .nth(1)
                        .is_some()
                })
                .collect();
            assert_eq!(
                cursor_cells.len(),
                1,
                "cursor missing or outside field at {width}x{height}"
            );
        } else if width == 20 {
            assert!(buffer_text(&buffer).contains("Resize"));
        }
        app.build();
    }
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    let item = &TodoList::load().items[0];
    assert_eq!(item.text().chars().count(), 300);
    assert!(item.notes().contains("long note 界\nlong note 界"));
}

#[test]
fn compact_todo_keeps_core_actions_visible_and_long_project_names_off_titles() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos
        .add("a-very-long-project-name", "/p", "Readable title");
    app.build();
    app.focus_panel(3);
    let buffer = render(&mut app, 60, 40);
    let footer: String = (0..60)
        .map(|column| buffer[(column, 39)].symbol())
        .collect();
    for action in [" h  to a session", " Enter  Details"] {
        assert!(footer.contains(action), "{footer}");
    }
    let item_row = *app
        .hit_lines
        .iter()
        .find(|(_, value)| **value == (3, Some(0)))
        .unwrap()
        .0;
    let row: String = (0..60)
        .map(|column| buffer[(column, item_row)].symbol())
        .collect();
    assert!(row.contains("Readable title"), "{row}");
}

#[test]
fn compact_todo_retains_queue_and_routing_markers_in_a_forty_column_row() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    let id = design_item(&mut app, "runway", "/r/runway", "Readable title");
    app.todos
        .set_category(&id, crate::todo::Category::Improvement);
    app.todos.change_priority(&id, 4);
    app.todos.toggle_queued(&id);
    app.build();
    app.focus_panel(3);
    let buffer = render(&mut app, 40, 40);
    let item_row = *app
        .hit_lines
        .iter()
        .find(|(_, value)| **value == (3, Some(0)))
        .unwrap()
        .0;
    let row: String = (0..40)
        .map(|column| buffer[(column, item_row)].symbol())
        .collect();
    assert!(
        row.contains("✎Q") && row.contains("!!!!") && row.contains("Readable"),
        "{row}"
    );
    let footer: String = (0..40)
        .map(|column| buffer[(column, 39)].symbol())
        .collect();
    assert!(footer.contains(" Enter  Details"), "{footer}");
}

#[test]
fn compact_todo_summary_keeps_design_routing_visible_ahead_of_long_source_ids() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = design_item(&mut app, "p", "/p", "design title");
    app.todos
        .items
        .iter_mut()
        .find(|item| item.id() == id)
        .unwrap()
        .0
        .insert("source".into(), json!(format!("session {SESSION_A}")));
    app.todos.save();
    app.build();
    app.focus_panel(3);
    let buffer = render(&mut app, 40, 40);
    assert!(
        buffer_text(&buffer)
            .lines()
            .any(|line| line.contains("Task")
                && line.contains("Claude")
                && line.contains("Design"))
    );
    assert!(crate::app::todo_details(&app.todo_entries[0]).contains(SESSION_A));
}

#[test]
fn compact_todo_scrolls_selection_past_headings_and_filters_without_index_drift() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let doing = app.todos.add("p", "/p", "working");
    app.todos.set_status(&doing, "doing", "");
    let mut ids = Vec::new();
    for index in 0..15 {
        let id = app.todos.add("p", "/p", &format!("task {index:02}"));
        app.todos.set_category(&id, crate::todo::Category::Bug);
        ids.push(id);
    }
    app.build();
    app.focus_panel(3);
    let screen = &mut FakeScreen::default();
    for _ in 0..15 {
        app.handle(Key::Down, screen);
    }
    render(&mut app, 60, 16);
    assert_eq!(app.todo_entries[app.panels[3].selected].key(), ids[14]);
    assert!(app.hit_lines.values().any(|value| *value == (3, Some(15))));
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
    for _ in 0..14 {
        app.handle(Key::Down, screen);
    }
    render(&mut app, 60, 16);
    let selected = app.todo_entries[app.panels[3].selected].key();
    assert_eq!(selected, ids[14]);
    app.handle(Key::Char('x'), screen);
    assert!(
        !TodoList::load()
            .items
            .iter()
            .any(|item| item.id() == selected)
    );
    assert_eq!(TodoList::load().status_of(&doing), "doing");
}

#[test]
fn todo_composer_can_retry_a_failed_write_without_mutating_the_list() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "retry title");
    fs::create_dir_all(crate::paths::todo()).unwrap();
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert!(app.todos.items.is_empty());
    assert!(buffer_text(&render(&mut app, 60, 30)).contains("retry title"));
    fs::remove_dir(crate::paths::todo()).unwrap();
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(TodoList::load().items.len(), 1);
    assert_eq!(TodoList::load().items[0].text(), "retry title");
}

#[test]
fn todo_composer_cycles_all_six_kinds_and_reveals_saved_items_after_filtering() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    for (steps, expected) in ["task", "feature", "bug", "improvement", "chore", "research"]
        .into_iter()
        .enumerate()
    {
        app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
        app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
        type_in_app(&mut app, expected);
        let screen = &mut FakeScreen::default();
        app.handle(Key::BackTab, screen);
        for _ in 0..steps {
            app.handle(Key::Right, screen);
        }
        app.handle(Key::Ctrl('s'), screen);
        let items = TodoList::load().items;
        let item = items.iter().find(|item| item.text() == expected).unwrap();
        assert_eq!(item.0["category"], expected);
        assert_eq!(app.todo_entries[app.panels[3].selected].key(), item.id());
        assert_eq!(app.subtitle(3).contains("kind: Bug"), expected == "bug");
    }
}

#[test]
fn todo_composer_multiline_arrows_keep_the_edit_on_the_intended_line() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    type_in_app(&mut app, "title");
    let screen = &mut FakeScreen::default();
    app.handle(Key::Tab, screen);
    app.handle(Key::Paste("a界b\nxéz".into()), screen);
    app.handle(Key::Left, screen);
    app.handle(Key::Up, screen);
    app.handle(Key::Backspace, screen);
    type_in_app(&mut app, "c");
    app.handle(Key::Down, screen);
    app.handle(Key::Right, screen);
    type_in_app(&mut app, "!");
    app.handle(Key::Ctrl('s'), screen);
    assert_eq!(TodoList::load().items[0].notes(), "acb\nxéz!");
}

#[test]
fn compact_todo_groups_keep_actions_and_clicks_on_the_displayed_item() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    let waiting = app.todos.add("runway", "/r", "waiting title");
    app.todos.change_priority(&waiting, 4);
    let doing = app.todos.add("runway", "/r", "working title");
    app.todos.set_status(&doing, "doing", "");
    app.build();
    app.focus_panel(3);
    render(&mut app, 60, 40);
    assert_eq!(
        app.todo_entries.iter().map(Entry::key).collect::<Vec<_>>(),
        [doing.clone(), waiting.clone()]
    );
    let headings: Vec<_> = app.panels[3]
        .lines
        .iter()
        .filter(|line| line.item.is_none())
        .map(|line| line.texts())
        .collect();
    assert_eq!(headings, ["In progress", "To do"]);
    let rows: Vec<_> = app.panels[3]
        .lines
        .iter()
        .filter(|line| line.item.is_some())
        .collect();
    assert!(
        rows.iter()
            .all(|line| !line.texts().contains("runway") && !line.texts().contains("Task"))
    );
    assert_eq!(
        rows[0].segments.last().unwrap().0,
        rows[1].segments.last().unwrap().0
    );
    let click_row = *app
        .hit_lines
        .iter()
        .find(|(_, value)| **value == (3, Some(1)))
        .unwrap()
        .0;
    app.handle(
        Key::Click {
            x: 5,
            y: click_row,
            double: false,
        },
        &mut FakeScreen::default(),
    );
    app.handle(Key::Char('Q'), &mut FakeScreen::default());
    assert!(TodoList::load().is_queued(&waiting));
    assert!(!TodoList::load().is_queued(&doing));
    app.handle(Key::Up, &mut FakeScreen::default());
    app.handle(Key::Char('d'), &mut FakeScreen::default());
    assert_eq!(TodoList::load().status_of(&doing), "done");
}

#[test]
fn compact_todo_summary_follows_selection_and_yields_to_short_heights() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = app.todos.add("p", "/p", "selected full title");
    app.todos
        .edit(&id, "selected full title", "\nfirst note\nsecond note");
    app.build();
    app.focus_panel(3);
    render(&mut app, 60, 40);
    assert_eq!(app.panels[3].fixed_bottom.len(), 3);
    assert!(
        app.panels[3].fixed_bottom[0]
            .texts()
            .contains("selected full title")
    );
    assert_eq!(app.panels[3].fixed_bottom[2].texts(), "first note");
    assert!(!app.panels[3].fixed_top.is_empty());
    app.focus_panel(1);
    render(&mut app, 60, 40);
    assert!(app.panels[3].fixed_bottom.is_empty());
    app.focus_panel(3);
    render(&mut app, 40, 12);
    assert!(app.panels[3].fixed_bottom.is_empty());
}

#[test]
fn todo_drafts_recover_an_interrupted_addition_in_its_original_project_once() {
    let fixture = Fixture::new();
    fixture.view(SESSION_A);
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        100
    )]));
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    type_in_app(&mut app, "Original draft");
    app.handle(Key::Tab, &mut FakeScreen::default());
    type_in_app(&mut app, "Keep these notes");
    assert!(
        TodoList::load().items.is_empty(),
        "an editor failure must retain a draft, not claim a save"
    );
    drop(app);
    fixture.view(SESSION_B);
    fixture.snapshot(json!([session(
        SESSION_B,
        "waiting",
        "opusline",
        "/r/opusline",
        100
    )]));
    let recovered = fixture.app();
    assert_eq!(recovered.todos.items.len(), 1);
    let item = &recovered.todos.items[0];
    assert_eq!(
        (item.text(), item.notes(), item.project(), item.cwd()),
        (
            "Original draft".into(),
            "Keep these notes".into(),
            "runway".into(),
            "/r/runway".into()
        )
    );
    assert!(
        recovered.message.contains("recovered"),
        "{}",
        recovered.message
    );
    assert_eq!(
        fixture.app().todos.items.len(),
        1,
        "recovery duplicated the draft"
    );
}

#[test]
fn todo_drafts_recover_edits_without_changing_identity_or_status() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = app.todos.add("runway", "/r/runway", "Original");
    app.todos.set_status(&id, "doing", SESSION_A);
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('e'), &mut FakeScreen::default());
    for _ in 0..8 {
        app.handle(Key::Backspace, &mut FakeScreen::default());
    }
    type_in_app(&mut app, "Recovered edit");
    app.handle(Key::Tab, &mut FakeScreen::default());
    type_in_app(&mut app, "New notes");
    assert_eq!(TodoList::load().items[0].text(), "Original");
    drop(app);
    let recovered = fixture.app();
    assert_eq!(recovered.todos.items.len(), 1);
    let item = &recovered.todos.items[0];
    assert_eq!(
        (item.id(), item.text(), item.notes(), item.status()),
        (
            id,
            "Recovered edit".into(),
            "New notes".into(),
            "doing".into()
        )
    );
}

#[test]
fn todo_drafts_survive_a_failed_list_save_and_recover_after_it_is_fixed() {
    let fixture = Fixture::new();
    fixture.view(SESSION_A);
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        100
    )]));
    let mut app = fixture.app();
    fs::create_dir_all(fixture.path("state/cockpit/todo.json")).unwrap();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    type_in_app(&mut app, "Do not lose me");
    for key in [Key::BackTab, Key::Right, Key::Right] {
        app.handle(key, &mut FakeScreen::default());
    }
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert!(buffer_text(&render(&mut app, 60, 30)).contains("draft kept"));
    drop(app);
    let app = fixture.app();
    assert!(
        app.message.contains("draft") && !app.message.starts_with("added"),
        "{}",
        app.message
    );
    assert!(
        fixture.app().message.contains("draft"),
        "startup should report the failed recovery"
    );
    fs::remove_dir(fixture.path("state/cockpit/todo.json")).unwrap();
    let recovered = fixture.app();
    assert_eq!(recovered.todos.items[0].text(), "Do not lose me");
    assert_eq!(
        recovered.todos.items[0].category(),
        Some(crate::todo::Category::Bug)
    );
}

#[test]
fn todo_drafts_are_not_recovered_while_an_editor_is_active() {
    let fixture = Fixture::new();
    fixture.view(SESSION_A);
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        100
    )]));
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    type_in_app(&mut app, "Still editing");
    let lock = fs::File::open(fixture.path("run/cockpit-todo-editor.lock")).unwrap();
    lock.lock_shared().unwrap();
    let mut restarted = fixture.app();
    assert!(restarted.todos.items.is_empty());
    drop(app);
    drop(lock);
    restarted.tick();
    assert_eq!(restarted.todos.items[0].text(), "Still editing");
}

#[test]
fn todo_drafts_cancel_a_blank_first_line_without_promoting_notes_to_the_title() {
    for is_interrupted in [false, true] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        let id = app.todos.add("runway", "/r/runway", "Original title");
        app.todos.edit(&id, "Original title", "Existing notes");
        app.build();
        app.focus_panel(3);
        app.handle(Key::Char('e'), &mut FakeScreen::default());
        for _ in 0..14 {
            app.handle(Key::Backspace, &mut FakeScreen::default());
        }
        if !is_interrupted {
            app.handle(Key::Esc, &mut FakeScreen::default());
        }
        drop(app);
        let recovered = fixture.app();
        assert_eq!(
            (
                recovered.todos.items[0].text(),
                recovered.todos.items[0].notes()
            ),
            ("Original title".into(), "Existing notes".into())
        );
        assert_eq!(
            fs::read_dir(fixture.path("state/cockpit/todo-drafts"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn todo_drafts_left_after_a_successful_save_do_not_create_duplicates() {
    for is_edit in [false, true] {
        let fixture = Fixture::new();
        fixture.view(SESSION_A);
        fixture.snapshot(json!([session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            100
        )]));
        let mut app = fixture.app();
        if is_edit {
            app.todos.add("runway", "/r/runway", "Old title");
            app.build();
        }
        app.focus_panel(3);
        app.handle(
            Key::Char(if is_edit { 'e' } else { 't' }),
            &mut FakeScreen::default(),
        );
        if is_edit {
            for _ in 0..9 {
                app.handle(Key::Backspace, &mut FakeScreen::default());
            }
        }
        type_in_app(&mut app, "Saved before crash");
        app.handle(Key::Tab, &mut FakeScreen::default());
        type_in_app(&mut app, "Important notes");
        let drafts = fixture.path("state/cockpit/todo-drafts");
        let backup: Vec<_> = fs::read_dir(&drafts)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect();
        app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
        let saved_id = TodoList::load().items[0].id();
        for (name, contents) in backup {
            fs::write(drafts.join(name), contents).unwrap();
        }
        let recovered = fixture.app();
        assert_eq!(recovered.todos.items.len(), 1);
        assert_eq!(recovered.todos.items[0].id(), saved_id);
        assert_eq!(recovered.todos.items[0].notes(), "Important notes");
    }
}

#[test]
fn todo_drafts_report_native_startup_failure_without_losing_existing_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos
        .add("runway", "/r/runway", "Keep my existing text");
    app.build();
    app.focus_panel(3);
    fixture.write(
        "state/cockpit/todo-drafts",
        "not a writable draft directory",
    );
    app.handle(Key::Char('e'), &mut FakeScreen::default());
    assert!(
        app.message.contains("cannot protect the TODO draft"),
        "{}",
        app.message
    );
    assert_eq!(TodoList::load().items[0].text(), "Keep my existing text");
    assert_eq!(fixture.app().todos.items[0].text(), "Keep my existing text");
}

#[test]
fn todo_drafts_cancel_empty_titles_and_leave_untracked_legacy_text_alone() {
    let fixture = Fixture::new();
    fixture.view(SESSION_A);
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        100
    )]));
    fixture.write("run/claude-todo-old.md", "Legacy draft without a project\n");
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(fixture.app().todos.items.is_empty());
    assert!(fixture.path("run/claude-todo-old.md").exists());
    let drafts = fixture.path("state/cockpit/todo-drafts");
    assert!(drafts.is_dir(), "editor did not create a persistent draft");
    assert_eq!(fs::read_dir(drafts).unwrap().count(), 0);
}

#[test]
fn todo_editor_holds_the_reload_lock_for_additions_and_edits() {
    for is_edit in [false, true] {
        let fixture = Fixture::new();
        fixture.view(SESSION_A);
        fixture.snapshot(json!([session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            100
        )]));
        let mut app = fixture.app();
        if is_edit {
            app.todos.add("runway", "/r/runway", "Original title");
            app.build();
        }
        app.focus_panel(3);
        app.handle(
            Key::Char(if is_edit { 'e' } else { 't' }),
            &mut FakeScreen::default(),
        );
        let lock = fs::File::open(fixture.path("run/cockpit-todo-editor.lock")).unwrap();
        assert!(
            lock.try_lock().is_err(),
            "native form released the reload lock before saving"
        );
        if is_edit {
            for _ in 0..14 {
                app.handle(Key::Backspace, &mut FakeScreen::default());
            }
        }
        type_in_app(&mut app, "Saved title");
        app.handle(Key::Tab, &mut FakeScreen::default());
        type_in_app(&mut app, "Saved notes");
        app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
        assert_eq!(TodoList::load().items[0].text(), "Saved title");
        assert!(lock.try_lock().is_ok(), "lock leaked after saving");
    }
}

#[test]
fn the_editor_file_gives_back_a_title_and_notes() {
    let text = crate::app::editor_text("Old title", "old notes", "because");
    assert!(text.contains("# Why Claude proposed it:\n# because"));
    let edited = text.replacen("Old title", "New title", 1).replacen(
        "old notes",
        "## Steps\n1. this\n2. that",
        1,
    );
    assert_eq!(
        crate::app::parse_editor_text(&edited),
        Some(("New title".into(), "## Steps\n1. this\n2. that".into()))
    );
    assert_eq!(crate::app::parse_editor_text("\n\n"), None);
}

#[test]
fn the_form_saves_multiline_notes_and_sanitizes_control_characters() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::typing(&["/p"]));
    app.handle(
        Key::Paste(" New\n title ".into()),
        &mut FakeScreen::default(),
    );
    app.handle(Key::Tab, &mut FakeScreen::default());
    app.handle(
        Key::Paste(format!("## Steps\n1. this\n2. that\n{}", "界".repeat(4100))),
        &mut FakeScreen::default(),
    );
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    let item = &TodoList::load().items[0];
    assert_eq!(item.text(), "New title");
    assert!(item.notes().starts_with("## Steps\n1. this\n2. that\n"));
    assert_eq!(item.notes().chars().count(), 4000);
}

#[test]
fn e_saves_what_was_written_in_the_native_form() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = app.todos.add("p", "/p", "draft");
    app.build();
    app.focus_panel(3);
    let screen = &mut FakeScreen::default();
    app.handle(Key::Char('e'), screen);
    for _ in 0..5 {
        app.handle(Key::Backspace, screen);
    }
    type_in_app(&mut app, "Better title");
    app.handle(Key::Tab, screen);
    type_in_app(&mut app, "first note");
    app.handle(Key::Ctrl('s'), screen);
    let item = TodoList::load()
        .items
        .into_iter()
        .find(|item| item.id() == id)
        .unwrap();
    assert_eq!(
        (item.text().as_str(), item.notes().as_str()),
        ("Better title", "first note")
    );
}

#[test]
fn multiline_queued_prompts_preserve_paragraphs_through_dispatch() {
    for id in [SESSION_A, OPENCODE_SESSION] {
        let fixture = Fixture::new();
        let row = if id == OPENCODE_SESSION {
            opencode_session("waiting", "runway", 200)
        } else {
            session(SESSION_A, "waiting", "runway", "/r/runway", 200)
        };
        fixture.snapshot(json!([row]));
        fixture.shim(
            "cockpit",
            r#"printf '%s' "$3" > "$XDG_RUNTIME_DIR/delivered-prompt""#,
        );
        let mut app = fixture.app();
        crate::prompts::add(id, "First line\r\n\r\n  - second line").unwrap();
        assert_eq!(
            crate::prompts::oldest(id).unwrap().0,
            "First line\n\n  - second line"
        );
        app.dispatch_prompts();
        assert_eq!(
            fs::read_to_string(fixture.path("run/delivered-prompt")).unwrap(),
            "First line\n\n  - second line"
        );
        assert_eq!(crate::prompts::count(id), 0);
    }
}

#[test]
fn multiline_queued_prompts_reject_other_controls_and_keep_the_character_limit() {
    let _fixture = Fixture::new();
    for text in [
        "".to_string(),
        "\n \n".into(),
        "first\n\u{1b}[201~second".into(),
        "first\n\u{3}second".into(),
        "first\rsecond".into(),
        "first\tsecond".into(),
        "é".repeat(4001),
    ] {
        assert!(crate::prompts::add(SESSION_A, &text).is_err());
    }
    assert_eq!(crate::prompts::count(SESSION_A), 0);
    let text = format!("{}\né", "é".repeat(3998));
    crate::prompts::add(SESSION_A, &text).unwrap();
    assert_eq!(crate::prompts::oldest(SESSION_A).unwrap().0, text);
}

#[test]
fn a_queued_prompt_waits_for_the_end_of_the_turn_one_per_turn() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        100
    )]));
    let mut app = fixture.app();
    crate::prompts::add(SESSION_A, "first thought").unwrap();
    crate::prompts::add(SESSION_A, "second thought").unwrap();
    app.dispatch_prompts();
    assert!(fixture.calls("cockpit").is_empty(), "sent while it works");
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        200
    )]));
    app.build();
    app.dispatch_prompts();
    app.dispatch_prompts();
    assert_eq!(
        fixture.calls("cockpit"),
        [format!("cockpit say {SESSION_A} first thought")]
    );
    assert_eq!(crate::prompts::count(SESSION_A), 1);
}

#[test]
fn enter_shows_todo_details_in_the_column_until_dismissed() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("p", "/p", "short item");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert!(
        buffer_rows(&render(&mut app, 80, 40))
            .join("\n")
            .contains("short item")
    );
    assert!(
        !fixture
            .calls("tmux")
            .iter()
            .any(|call| call.contains("display-popup"))
    );
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(
        !buffer_rows(&render(&mut app, 80, 40))
            .join("\n")
            .contains("Todo details")
    );
}

#[test]
fn palette_homes_trails_todo_details_keep_the_inspected_task() {
    for details in [false, true] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        let first = app.todos.add("shop", "/r/shop", "Original task");
        let second = app.todos.add("shop", "/r/shop", "Other task");
        app.build();
        app.focus_panel(3);
        app.panels[3].selected = app
            .todo_entries
            .iter()
            .position(|entry| entry.key() == first)
            .unwrap();
        if details {
            app.handle(Key::Char('i'), &mut FakeScreen::default());
            app.panels[3].selected = app
                .todo_entries
                .iter()
                .position(|entry| entry.key() == second)
                .unwrap();
        }
        app.handle(Key::Char('T'), &mut FakeScreen::default());
        let text = buffer_rows(&render(&mut app, 100, 45)).join("\n");
        assert!(
            text.contains("Task trail") && text.contains("Original task"),
            "{text}"
        );
    }
}

#[test]
fn qa_sessions_do_not_receive_queued_todos() {
    for is_linked in [true, false] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([session(
            SESSION_B,
            "waiting",
            "project-a",
            "/p",
            util::now_secs() - 600
        )]));
        let mut snapshot = team_snapshot();
        if !is_linked {
            snapshot["members"] = json!([]);
            snapshot["spawns"] =
                json!([{"role": "qa", "session_id": SESSION_B, "status": "superseded"}]);
        }
        team_host(&fixture, snapshot, vec![]);
        let mut app = fixture.app();
        app.gaming = || false;
        app.check_team();
        let id = app.todos.add("project-a", "/p", "Unrelated work");
        app.todos.toggle_queued(&id);
        app.dispatch_queue();
        assert_eq!(app.todos.status_of(&id), "todo", "linked: {is_linked}");
        assert!(fixture.calls("cockpit").is_empty());
    }
}

#[test]
fn todo_handoffs_start_a_worker_instead_of_reusing_qa() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_B, "waiting", "project-a", "/p", 1)]));
    team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    app.check_team();
    let id = app.todos.add("project-a", "/p", "Unrelated work");
    app.build();
    let entry = app
        .todo_entries
        .iter()
        .find(|entry| entry.key() == id)
        .unwrap()
        .clone();
    assert_eq!(
        app.hand_to_claude(&entry, &mut FakeScreen::default()),
        "starting a session in p"
    );
    let calls = fixture.wait_for_calls("cockpit", 1);
    assert!(calls[0].contains("background /p "), "{calls:?}");
}

#[test]
fn viewed_qa_does_not_override_the_worker_chosen_for_a_todo() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_B, "waiting", "project-a", "/qa-copy", 1),
        session(SESSION_A, "waiting", "project-a", "/p", 1)
    ]));
    fixture.view(SESSION_B);
    team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    app.check_team();
    let id = app.todos.add("project-a", "/p", "Unrelated work");
    app.build();
    let entry = app
        .todo_entries
        .iter()
        .find(|entry| entry.key() == id)
        .unwrap()
        .clone();
    assert_eq!(
        app.hand_to_claude(&entry, &mut FakeScreen::default()),
        "sent to project-a"
    );
    assert!(fixture.calls("cockpit")[0].starts_with(&format!("cockpit say {SESSION_A} ")));
}

#[test]
fn typed_items_persist_and_issues_are_listed_after_them() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("runway", "/r/runway", "add  a   healthcheck");
    app.reviews
        .set_issues(vec![issue(2, "Broken login", &["bug"])]);
    app.build();
    assert!(
        matches!(&app.todo_entries[0], Entry::Typed(item) if item.text() == "add a healthcheck")
    );
    assert!(matches!(&app.todo_entries[1], Entry::Issue { .. }));
    assert_eq!(TodoList::load().items[0].project(), "runway");
}

#[test]
fn the_list_is_ordered_by_priority_across_typed_items_and_issues() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("runway", "/r/runway", "normal typed");
    app.todos.add("runway", "/r/runway", "becomes urgent");
    app.reviews.set_issues(vec![
        issue(1, "minor", &["priority: low"]),
        issue(2, "outage", &["P1"]),
        issue(3, "plain", &[]),
    ]);
    app.build();
    app.focus_panel(3);
    app.panels[3].selected = 2;
    app.handle(Key::Char('+'), &mut FakeScreen::default());
    let order: Vec<String> = app
        .todo_entries
        .iter()
        .map(|entry| match entry {
            Entry::Typed(item) => item.text(),
            Entry::Issue { issue, .. } => issue.title.clone(),
        })
        .collect();
    assert_eq!(
        order,
        vec!["becomes urgent", "outage", "normal typed", "plain", "minor"]
    );
    assert_eq!(app.panels[3].selected, 0);
}

#[test]
fn h_sends_to_the_running_session_or_starts_one_and_marks_it_in_progress() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    let healthcheck = app.todos.add("runway", "/r/runway", "add a healthcheck");
    let postgres = app.todos.add("homelab", "/r/homelab", "bump postgres");
    app.build();
    show_to_do_tab(&mut app);
    // An item handed over moves to In progress: the next one is the first row of To do.
    for _ in 0..2 {
        app.build();
        app.panels[3].selected = 0;
        app.handle(Key::Char('h'), &mut FakeScreen::typing(&["claude"]));
    }
    let mut calls = fixture.wait_for_calls("cockpit", 2);
    calls.sort();
    assert_eq!(
        calls,
        [
            format!(
                "cockpit --agent claude background /r/homelab bump postgres (todo {postgres}: before working, run cockpit-todo start {postgres}; when this is done, run cockpit-todo done {postgres})"
            ),
            format!(
                "cockpit say {SESSION_A} add a healthcheck (todo {healthcheck}: before working, run cockpit-todo start {healthcheck}; when this is done, run cockpit-todo done {healthcheck})"
            )
        ]
    );
    assert!(
        TodoList::load()
            .items
            .iter()
            .all(|item| item.status() == "doing")
    );
}

#[test]
fn a_design_item_goes_to_the_projects_designer_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "opusline", "/r/opusline", 1),
        session(SESSION_B, "waiting", "opusline", "/r/opusline", 2)
    ]));
    declare_role(&fixture, SESSION_A, "developer");
    declare_role(&fixture, SESSION_B, "designer");
    let mut app = fixture.app();
    design_item(
        &mut app,
        "opusline",
        "/r/opusline",
        "empty state of the order list",
    );
    hand_over_first_item(&mut app);
    assert!(
        fixture.wait_for_calls("cockpit", 1)[0]
            .starts_with(&format!("cockpit say {SESSION_B} empty state"))
    );
}

#[test]
fn an_ordinary_item_skips_the_designer_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "opusline", "/r/opusline", 1),
        session(SESSION_B, "waiting", "opusline", "/r/opusline", 2)
    ]));
    declare_role(&fixture, SESSION_A, "designer");
    let mut app = fixture.app();
    app.todos
        .add("opusline", "/r/opusline", "fix the login redirect");
    hand_over_first_item(&mut app);
    assert!(
        fixture.wait_for_calls("cockpit", 1)[0]
            .starts_with(&format!("cockpit say {SESSION_B} fix the login"))
    );
}

#[test]
fn an_item_goes_to_the_viewed_parallel_session_rather_than_the_main_checkout() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "opusline", "/r/opusline", 1),
        session(
            SESSION_B,
            "waiting",
            "opusline",
            "/r/.parallel/opusline/fix-login",
            2
        )
    ]));
    fixture.view(SESSION_B);
    let mut app = fixture.app();
    app.todos
        .add("opusline", "/r/opusline", "fix the login redirect");
    hand_over_first_item(&mut app);
    assert!(
        fixture.wait_for_calls("cockpit", 1)[0]
            .starts_with(&format!("cockpit say {SESSION_B} fix the login"))
    );
}

#[test]
fn an_item_of_another_project_skips_the_viewed_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "opusline", "/r/opusline", 1),
        session(SESSION_B, "waiting", "runway", "/r/runway", 2)
    ]));
    fixture.view(SESSION_B);
    let mut app = fixture.app();
    let id = app
        .todos
        .add("opusline", "/r/opusline", "fix the login redirect");
    let entry = Entry::Typed(
        app.todos
            .items
            .iter()
            .find(|item| item.id() == id)
            .unwrap()
            .clone(),
    );
    app.hand_to_claude(&entry, &mut FakeScreen::default());
    assert!(
        fixture.wait_for_calls("cockpit", 1)[0]
            .starts_with(&format!("cockpit say {SESSION_A} fix the login"))
    );
}

#[test]
fn a_design_item_without_a_designer_starts_one_with_the_design_skill() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    design_item(&mut app, "opusline", "/r/opusline", "empty state");
    hand_over_first_item(&mut app);
    assert!(fixture.wait_for_calls("cockpit", 1)[0].starts_with("cockpit --agent claude background /r/opusline Load the figcraft-design skill first. empty state"));
}

#[test]
fn a_card_shows_its_role_and_a_design_item_its_mark() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        1
    )]));
    declare_role(&fixture, SESSION_A, "designer");
    let mut app = fixture.app();
    design_item(&mut app, "opusline", "/r/opusline", "empty state");
    app.build();
    let parts = |panel: usize| {
        app.panels[panel]
            .lines
            .iter()
            .chain(&app.panels[panel].fixed_bottom)
            .flat_map(|line| line.right.iter().map(|part| part.text.clone()))
            .collect::<Vec<_>>()
    };
    assert!(
        parts(1).contains(&"✎ designer".to_string()),
        "{:?}",
        parts(1)
    );
    assert!(parts(3).contains(&"✎".to_string()), "{:?}", parts(3));
}

#[test]
fn the_list_shows_only_the_viewed_sessions_project() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        1
    )]));
    let mut app = fixture.app();
    app.todos.add("opusline", "/r/opusline", "net amounts");
    app.todos.add("runway", "/r/runway", "healthcheck");
    app.build();
    assert_eq!(app.todo_entries.len(), 2);
    fixture.view(SESSION_A);
    app.build();
    assert_eq!(app.todo_entries.len(), 1);
    assert!(app.subtitle(3).starts_with("opusline"));
}

#[test]
fn i_and_d_set_in_progress_and_done_and_toggle_back() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("runway", "/r", "deploy");
    app.build();
    app.focus_panel(3);
    let mut statuses = Vec::new();
    for key in ['i', 'i', 'd', 'd'] {
        if key == 'i' {
            app.handle(Key::Char('i'), &mut FakeScreen::default());
        }
        app.handle(Key::Char(key), &mut FakeScreen::default());
        statuses.push(TodoList::load().items[0].status());
    }
    assert_eq!(statuses, vec!["doing", "todo", "done", "todo"]);
    assert!(
        crate::queue::reserve_task("/r", &app.todos.items[0].id(), None, false).is_ok(),
        "manual undo must release its exact status-only reservation"
    );
}

#[test]
fn deleting_manual_status_work_releases_the_project_for_another_task() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("runway", "/r", "manual work");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    assert_eq!(TodoList::load().items[0].status(), "doing");
    app.handle(Key::Char('x'), &mut FakeScreen::default());
    assert!(crate::queue::reserve_task("/r", "bbbbbbbb", None, false).is_ok());
}

#[test]
fn conclusive_spawn_failure_releases_but_nonzero_execution_retains_the_exact_launch() {
    let fixture = Fixture::new();
    fixture.scheduler();
    let token = crate::queue::reserve_task("/r", "aaaaaaaa", None, false).unwrap();
    let shim = fixture.root.join("bin/cockpit");
    let previous = fs::read(&shim).unwrap();
    fs::remove_file(&shim).unwrap();
    let result = crate::queue::launch_reserved(&token, &["say", SESSION_A, "message"], false);
    fs::write(&shim, previous).unwrap();
    fs::set_permissions(&shim, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert!(result.unwrap_err().contains("not submitted"));
    let next = crate::queue::reserve_task("/r", "aaaaaaaa", None, false).unwrap();
    fs::write(&shim, "#!/bin/sh\nexit 1\n").unwrap();
    assert!(
        crate::queue::launch_reserved(&next, &["say", SESSION_A, "message"], false)
            .unwrap_err()
            .contains("uncertain")
    );
    assert!(crate::queue::reserve_task("/r", "bbbbbbbb", None, false).is_err());
}

#[test]
fn plus_raises_up_to_four_marks_and_minus_goes_down_to_low() {
    let fixture = Fixture::new();
    let mut todos = TodoList::load();
    let id = todos.add("runway", "/r", "deploy");
    let mut marks = Vec::new();
    for _ in 0..5 {
        todos.change_priority(&id, 1);
        marks.push(crate::todo::priority_mark(todos.items[0].priority()));
    }
    assert_eq!(marks, vec!["!", "!!", "!!!", "!!!!", "!!!!"]);
    for _ in 0..6 {
        todos.change_priority(&id, -1);
    }
    assert_eq!(crate::todo::priority_mark(todos.items[0].priority()), "↓");
    drop(fixture);
}

#[test]
fn todo_rows_and_details_show_the_kind_alongside_design_routing() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = design_item(&mut app, "runway", "/r/runway", "empty state");
    app.todos.set_category(&id, crate::todo::Category::Feature);
    app.todos.add("runway", "/r/runway", "old task");
    app.build();
    let badges: Vec<_> = app.panels[3]
        .lines
        .iter()
        .flat_map(|line| &line.right)
        .map(|part| part.text.as_str())
        .collect();
    assert!(
        badges.contains(&"✎") && !badges.contains(&"Task"),
        "{badges:?}"
    );
    assert!(
        app.panels[3]
            .lines
            .iter()
            .any(|line| line.texts().contains("Feature"))
    );
    let details = crate::app::todo_details(
        app.todo_entries
            .iter()
            .find(|entry| entry.key() == id)
            .unwrap(),
    );
    assert!(details.contains("Kind: Feature"), "{details}");
}

#[test]
fn kind_filter_limits_status_counts_and_keeps_github_issues_under_all() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let bug = app.todos.add("runway", "/r", "crash");
    app.todos.set_category(&bug, crate::todo::Category::Bug);
    let doing = app.todos.add("runway", "/r", "feature");
    app.todos
        .set_category(&doing, crate::todo::Category::Feature);
    app.todos.set_status(&doing, "doing", "");
    app.reviews
        .set_issues(vec![issue(3, "github bug", &["bug"])]);
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
    assert_eq!(
        app.todo_entries.iter().map(Entry::key).collect::<Vec<_>>(),
        [bug]
    );
    assert_eq!(app.todo_counts, [1, 0, 1, 0, 0]);
    assert!(app.subtitle(3).contains("kind: Bug"), "{}", app.subtitle(3));
    for _ in 0..3 {
        app.handle(Key::Char(']'), &mut FakeScreen::default());
    }
    assert!(app.todo_entries.is_empty());
    assert!(
        texts(&app, 3)
            .iter()
            .any(|line| line.contains("kind filter"))
    );
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["All"]));
    app.handle(Key::Char('['), &mut FakeScreen::default());
    assert_eq!(app.todo_entries.len(), 2); // Task tab: typed bug and GitHub issue.
}

#[test]
fn each_viewed_session_keeps_its_own_kind_filter() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "working", "runway", "/r/runway", 1),
        session(SESSION_B, "working", "runway", "/r/runway", 2)
    ]));
    let mut app = fixture.app();
    app.todos.add("runway", "/r", "task");
    fixture.view(SESSION_A);
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
    assert!(app.todo_entries.is_empty());
    fixture.view(SESSION_B);
    app.build();
    assert_eq!(app.todo_entries.len(), 1);
    fixture.view(SESSION_A);
    app.build();
    assert!(app.todo_entries.is_empty());
    assert!(app.subtitle(3).contains("kind: Bug"));
}

#[test]
fn changing_kind_preserves_notes_and_clears_a_filter_that_would_hide_it() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = design_item(&mut app, "runway", "/r", "keep title");
    app.todos.edit(&id, "keep title", "keep notes");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Task"]));
    app.handle(Key::Char('K'), &mut FakeScreen::typing(&["Bug"]));
    let item = &TodoList::load().items[0];
    assert_eq!(
        (item.text(), item.notes(), item.category()),
        (
            "keep title".into(),
            "keep notes".into(),
            Some(crate::todo::Category::Bug)
        )
    );
    assert!(item.is_design());
    assert_eq!(app.todo_entries.len(), 1);
    assert!(!app.subtitle(3).contains("kind: Task"));
}

#[test]
fn moving_an_item_gives_it_the_picked_project_and_folder() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = app.todos.add("runway", "/r", "filed in the wrong project");
    app.build();
    app.focus_panel(3);
    app.handle(
        Key::Char('m'),
        &mut FakeScreen::typing(&["/home/theo/IdeaProjects/opusline"]),
    );
    let item = TodoList::load()
        .items
        .into_iter()
        .find(|item| item.id() == id)
        .unwrap();
    assert_eq!(
        (item.project(), item.cwd()),
        ("opusline".into(), "/home/theo/IdeaProjects/opusline".into())
    );
    assert_eq!(app.message, "moved to opusline");
}

#[test]
fn an_item_in_progress_is_not_moved() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = app.todos.add("runway", "/r", "being worked on");
    app.todos.set_status(&id, "doing", "session-a");
    app.build();
    app.focus_panel(3);
    app.handle(
        Key::Char('m'),
        &mut FakeScreen::typing(&["/home/theo/IdeaProjects/opusline"]),
    );
    assert_eq!(TodoList::load().items[0].project(), "runway");
    assert!(app.message.starts_with("not moved"), "{}", app.message);
}

#[test]
fn cancelling_kind_and_filter_pickers_changes_nothing() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("runway", "/r", "task");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Task"]));
    let before = fs::read(crate::paths::todo()).unwrap();
    for key in ['K', 'm', 'f'] {
        app.handle(Key::Char(key), &mut FakeScreen::default());
    }
    assert_eq!(fs::read(crate::paths::todo()).unwrap(), before);
    assert!(app.subtitle(3).contains("kind: Task"));
}

#[test]
fn unknown_persisted_kinds_stay_visible_and_are_described_without_rewriting() {
    let fixture = Fixture::new();
    fixture.write(
        "state/cockpit/todo.json",
        &json!({"items": [
            {"id": "0123abcd", "text": "future", "category": "future-kind"}
        ]})
        .to_string(),
    );
    let mut app = fixture.app();
    assert_eq!(app.todo_entries.len(), 1);
    assert!(crate::app::todo_details(&app.todo_entries[0]).contains("future-kind"));
    app.focus_panel(3);
    app.handle(Key::Char('K'), &mut FakeScreen::default());
    assert_eq!(TodoList::load().items[0].0["category"], "future-kind");
}

#[test]
fn the_kind_filter_does_not_change_queue_dispatch() {
    let (fixture, mut app) = queue_fixture("waiting", 600);
    let id = app
        .todos
        .add("runway", "/r/runway", "task still dispatches");
    app.todos.toggle_queued(&id);
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
    assert!(app.todo_entries.is_empty());
    app.dispatch_queue();
    assert!(fixture.wait_for_calls("cockpit", 1)[0].contains("task still dispatches"));
}

#[test]
fn add_and_edit_choose_a_kind_before_saving() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.focus_panel(3);
    let screen = &mut FakeScreen::default();
    app.handle(Key::Char('t'), screen);
    type_in_app(&mut app, "New title");
    app.handle(Key::BackTab, screen);
    app.handle(Key::Right, screen);
    app.handle(Key::Tab, screen);
    app.handle(Key::Tab, screen);
    type_in_app(&mut app, "New notes");
    app.handle(Key::Ctrl('s'), screen);
    let items = TodoList::load().items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].category(), Some(crate::todo::Category::Feature));
    app.handle(Key::Char('e'), screen);
    app.handle(Key::BackTab, screen);
    app.handle(Key::Left, screen);
    app.handle(Key::Left, screen);
    app.handle(Key::Ctrl('s'), screen);
    let item = &TodoList::load().items[0];
    assert_eq!(
        (item.text(), item.notes(), item.category()),
        (
            "New title".into(),
            "New notes".into(),
            Some(crate::todo::Category::Research)
        )
    );
}

#[test]
fn cancelling_add_or_edit_form_discards_all_draft_changes() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    type_in_app(&mut app, "Changed title");
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(TodoList::load().items.is_empty());
    app.todos.add("runway", "/r", "original");
    app.build();
    app.handle(Key::Char('e'), &mut FakeScreen::default());
    type_in_app(&mut app, "Changed title");
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert_eq!(TodoList::load().items[0].text(), "original");
}

#[test]
fn the_kind_picker_starts_on_the_items_current_kind() {
    let fixture = Fixture::new();
    fixture.shim("fzf", "head -n 1");
    let mut app = fixture.app();
    let id = app.todos.add("runway", "/r", "keep kind");
    app.todos.set_category(&id, crate::todo::Category::Research);
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('K'), &mut FakeScreen::default());
    assert!(
        fixture
            .calls("fzf")
            .iter()
            .any(|call| call.contains("kind>"))
    );
    assert_eq!(
        TodoList::load().items[0].category(),
        Some(crate::todo::Category::Research)
    );
}

#[test]
fn long_kind_badges_leave_the_title_readable_in_a_narrow_column() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let id = design_item(&mut app, "runway", "/r/runway", "Repair the order list");
    app.todos
        .set_category(&id, crate::todo::Category::Improvement);
    app.todos.toggle_queued(&id);
    app.build();
    app.focus_panel(3);
    let buffer = render(&mut app, 40, 40);
    let rows: Vec<String> = (0..40)
        .map(|row| {
            (0..40)
                .map(|column| buffer[(column, row)].symbol())
                .collect()
        })
        .collect();
    assert!(
        rows.iter().any(|row| row.contains("Repair the")),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn the_active_kind_filter_is_visible_even_with_a_long_project_name() {
    let fixture = Fixture::new();
    let project = "a-very-long-project-name";
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        project,
        "/r/project",
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.todos.add(project, "/r/project", "a task");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('f'), &mut FakeScreen::typing(&["Bug"]));
    let buffer = render(&mut app, 40, 40);
    let rows: Vec<String> = (0..40)
        .map(|row| {
            (0..40)
                .map(|column| buffer[(column, row)].symbol())
                .collect()
        })
        .collect();
    assert!(
        rows.iter().any(|row| row.contains("kind: Bug")),
        "{rows:#?}"
    );
}

#[test]
fn each_todo_tab_starts_with_the_key_that_moves_its_items_along() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (tab, first) in [(1, "y"), (2, "r"), (3, "d"), (4, "⏎")] {
        app.todo_tab = tab;
        assert_eq!(app.todo_hints()[0].0, first, "tab {tab}");
    }
}

#[test]
fn the_todo_panel_opens_on_the_open_tab() {
    let fixture = Fixture::new();
    assert_eq!(fixture.app().todo_tab, 0);
}

#[test]
fn each_session_keeps_the_todo_tab_it_last_showed() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "waiting", "dotfiles", "/d", 2)
    ]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char(']'), &mut FakeScreen::default());
    fixture.view(SESSION_B);
    app.build();
    assert_eq!(app.todo_tab, 0, "a session not switched yet shows Open");
    fixture.view(SESSION_A);
    app.build();
    assert_eq!(app.todo_tab, 1);
}

#[test]
fn n_hands_the_item_to_a_new_session_in_a_clone_even_with_one_running() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.write("run/cockpit.current", SESSION_A);
    let mut app = fixture.app();
    let healthcheck = app.todos.add("runway", "/r/runway", "add a healthcheck");
    app.build();
    app.focus_panel(3);
    app.panels[3].selected = 0;
    app.handle(Key::Char('r'), &mut FakeScreen::typing(&["claude"]));
    let call = fixture.wait_for_calls("cockpit", 1).remove(0);
    assert!(
        call.starts_with(&format!(
            "cockpit --agent claude parallel /r/runway todo-{healthcheck} "
        )) && call.contains("add a healthcheck"),
        "{call}"
    );
    assert!(
        !call.contains(" say "),
        "sent to the running session: {call}"
    );
    assert_eq!(app.todos.status_of(&healthcheck), "doing");
}

#[test]
fn todo_detail_trail_reports_unavailable_continuity_without_crashing() {
    let fixture = Fixture::new();
    fixture.write("state/cockpit/continuity.json", "not json");
    let mut app = fixture.app();
    app.todos.add("runway", "/r/runway", "Task to inspect");
    app.build();
    app.focus_panel(3);
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    app.handle(Key::Char('T'), &mut FakeScreen::default());
    assert!(
        app.message.starts_with("Continuity unavailable:"),
        "{}",
        app.message
    );
}

#[test]
fn todo_detail_status_action_keeps_its_target_after_reselection_or_removal() {
    for removed in [false, true] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        let original = app.todos.add("runway", "/r/runway", "Original task");
        let other = app.todos.add("runway", "/r/runway", "Other task");
        app.build();
        app.focus_panel(3);
        app.panels[3].selected = app
            .todo_entries
            .iter()
            .position(|entry| entry.key() == original)
            .unwrap();
        app.handle(Key::Char('i'), &mut FakeScreen::default());
        if removed {
            app.todos.remove(&original).unwrap();
            app.build();
        }
        app.panels[3].selected = app
            .todo_entries
            .iter()
            .position(|entry| entry.key() == other)
            .unwrap();
        app.handle(Key::Char('i'), &mut FakeScreen::default());
        assert_eq!(app.todos.status_of(&other), "todo");
        if removed {
            assert_eq!(app.message, "This todo is no longer available");
        } else {
            assert_eq!(app.todos.status_of(&original), "doing");
        }
    }
}

#[test]
fn h_sends_to_the_running_opencode_session_like_to_claude() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([opencode_session("waiting", "runway", 1)]));
    let mut app = fixture.app();
    let healthcheck = app.todos.add("runway", "/r/runway", "add a healthcheck");
    hand_over_first_item(&mut app);
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1)[0],
        format!(
            "cockpit say {OPENCODE_SESSION} add a healthcheck (todo {healthcheck}: before working, run cockpit-todo start {healthcheck}; when this is done, run cockpit-todo done {healthcheck})"
        )
    );
}

#[test]
fn h_starts_opencode_when_it_is_the_saved_default() {
    let fixture = Fixture::new();
    fixture.write(
        "state/cockpit/settings.json",
        r#"{"default-agent":"opencode"}"#,
    );
    let mut app = fixture.app();
    app.todos.add("runway", "/r/runway", "add a healthcheck");
    app.build();
    app.focus_panel(3);
    app.panels[3].selected = 0;
    app.handle(Key::Char('h'), &mut FakeScreen::default());
    assert!(
        fixture.wait_for_calls("cockpit", 1)[0]
            .starts_with("cockpit --agent opencode background /r/runway add a healthcheck")
    );
}

#[test]
fn h_with_invalid_defaults_leaves_the_item_to_do() {
    let fixture = Fixture::new();
    fixture.write(
        "state/cockpit/settings.json",
        r#"{"default-agent":"unknown"}"#,
    );
    let mut app = fixture.app();
    let healthcheck = app.todos.add("runway", "/r/runway", "add a healthcheck");
    hand_over_first_item_with(&mut app, &[]);
    assert!(
        app.message.contains("invalid default-agent"),
        "{}",
        app.message
    );
    assert_eq!(app.todos.status_of(&healthcheck), "todo");
}

#[test]
fn r_starts_opencode_in_a_clone_when_it_is_the_saved_default() {
    let fixture = Fixture::new();
    fixture.write(
        "state/cockpit/settings.json",
        r#"{"default-agent":"opencode"}"#,
    );
    let mut app = fixture.app();
    let healthcheck = app.todos.add("runway", "/r/runway", "add a healthcheck");
    app.build();
    app.focus_panel(3);
    app.panels[3].selected = 0;
    app.handle(Key::Char('r'), &mut FakeScreen::default());
    assert!(
        fixture.wait_for_calls("cockpit", 1)[0].starts_with(&format!(
            "cockpit --agent opencode parallel /r/runway todo-{healthcheck} "
        ))
    );
}

#[test]
fn an_item_from_a_deleted_clone_starts_from_its_repository_checkout() {
    for (key, command) in [('r', "parallel"), ('h', "background")] {
        let fixture = Fixture::new();
        let checkout = fixture.path("home/IdeaProjects/runway");
        fs::create_dir_all(&checkout).unwrap();
        let (app, healthcheck) = hand_off_from_a_deleted_clone(&fixture, key);
        let call = fixture.wait_for_calls("cockpit", 1).remove(0);
        assert!(
            call.starts_with(&format!(
                "cockpit --agent opencode {command} {} ",
                checkout.display()
            )),
            "{key}: {call}"
        );
        assert_eq!(app.todos.status_of(&healthcheck), "doing", "{key}");
    }
}

#[test]
fn an_item_from_a_deleted_clone_without_a_checkout_stays_to_do() {
    for key in ['r', 'h'] {
        let fixture = Fixture::new();
        let (app, healthcheck) = hand_off_from_a_deleted_clone(&fixture, key);
        assert!(fixture.calls("cockpit").is_empty(), "{key}");
        assert_eq!(app.todos.status_of(&healthcheck), "todo", "{key}");
    }
}

#[test]
fn the_todo_form_is_a_card_over_the_column_with_kind_chips() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('t'), &mut FakeScreen::default());
    let buffer = render(&mut app, 60, 30);
    let rows = buffer_rows(&buffer);
    assert!(
        rows.iter().any(|row| row.contains('╭')) && rows.iter().any(|row| row.contains('╰')),
        "a card with a border: {rows:#?}"
    );
    let chips = rows
        .iter()
        .position(|row| row.contains("Feature"))
        .expect("kind chips");
    let task = rows[chips].find("Task").unwrap();
    let task_column = rows[chips][..task].chars().count() as u16;
    assert_eq!(
        buffer[(task_column, chips as u16)].bg,
        crate::app::color(crate::app::BLUE),
        "the selected kind is a blue chip"
    );
    assert!(
        !rows.iter().any(|row| row.contains("[ Add")),
        "no button row: ⌘S saves"
    );
}
