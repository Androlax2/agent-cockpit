use super::*;

#[test]
fn the_column_greys_its_focused_panel_when_the_focus_leaves_it() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.build();
    assert_eq!(blue_panel_corners(&mut app), 1);
    app.handle(Key::FocusLost, &mut FakeScreen::default());
    assert_eq!(blue_panel_corners(&mut app), 0);
}

#[test]
fn the_focus_coming_back_lights_the_same_panel_again() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.build();
    let screen = &mut FakeScreen::default();
    app.handle(Key::FocusLost, screen);
    app.handle(Key::FocusGained, screen);
    assert_eq!((blue_panel_corners(&mut app), app.focus), (1, 1));
}

#[test]
fn the_whole_column_fades_while_the_focus_is_elsewhere() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("p", "/p", "one item");
    app.build();
    app.handle(Key::FocusLost, &mut FakeScreen::default());
    let buffer = render(&mut app, 120, 40);
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| cell.modifier.contains(ratatui::style::Modifier::DIM))
    );
    assert!(
        !buffer
            .content
            .iter()
            .any(|cell| cell.bg == ratatui::style::Color::Indexed(crate::app::BLUE)),
        "a chip stays blue"
    );
}

#[test]
fn the_shown_todo_tab_is_drawn_as_a_chip() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.todos.add("p", "/p", "one item");
    show_to_do_tab(&mut app);
    let buffer = render(&mut app, 120, 40);
    let chip: String = (0..120)
        .filter_map(|x| {
            (0..40)
                .find(|y| {
                    buffer[(x, *y)].symbol() == "T"
                        && buffer[(x, *y)].bg == ratatui::style::Color::Indexed(crate::app::BLUE)
                })
                .map(|_| "T")
        })
        .collect();
    assert!(!chip.is_empty(), "no tab drawn on blue");
    let rows: Vec<String> = (0..40)
        .map(|y| {
            (0..120)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    assert!(
        rows.iter()
            .any(|row| row.contains(" To do 1 ") && !row.contains("[To do")),
        "{rows:#?}"
    );
}

#[test]
fn palette_homes_sessions_queue_and_updates_work_without_a_selection() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('u'), &mut FakeScreen::default());
    assert_eq!(app.message, "no unseen updates or return notes");
    app.handle(Key::Char('Q'), &mut FakeScreen::default());
    assert!(
        buffer_rows(&render(&mut app, 100, 45))
            .join("\n")
            .contains("Queue")
    );
}

#[test]
fn palette_homes_status_enter_runs_the_selected_service_or_utility() {
    for (label, program, expected) in [
        ("Claude", "xdg-open", "xdg-open https://status.claude.com/"),
        ("Codex", "xdg-open", "xdg-open https://status.openai.com/"),
        ("Daily digest", "cockpit", "cockpit digest"),
        ("Clones review", "cockpit", "cockpit clones"),
    ] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        app.focus_panel(0);
        let services = app.panels[0]
            .lines
            .iter()
            .position(|line| line.texts() == "SERVICES")
            .unwrap();
        let target = app.panels[0]
            .lines
            .iter()
            .skip(services)
            .find(|line| line.texts().contains(label) && line.item.is_some())
            .and_then(|line| line.item)
            .expect("selectable action");
        for _ in 0..target {
            app.handle(Key::Down, &mut FakeScreen::default());
        }
        render(&mut app, 40, 18);
        app.handle(Key::Enter, &mut FakeScreen::default());
        assert_eq!(fixture.wait_for_calls(program, 1), vec![expected]);
    }
}

#[test]
fn palette_homes_sessions_updates_include_archived_return_notes() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    write_note(&mut app, "My next step");
    fixture.snapshot(json!([]));
    app.build();
    app.handle(Key::Char('u'), &mut FakeScreen::typing(&["0"]));
    let text = buffer_rows(&render(&mut app, 100, 45)).join("\n");
    assert!(
        text.contains("tests passed") && text.contains("My next step") && text.contains("Archived"),
        "{text}"
    );
}

#[test]
fn palette_homes_sessions_details_review_only_the_updates_they_displayed() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let mut app = fixture.app();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    fixture.snapshot(json!([continuity_result_row(
        "22222222222222222222222222222222",
        "waiting"
    )]));
    app.build();
    app.handle(Key::Char('v'), &mut FakeScreen::default());
    assert!(app.message.contains("marked reviewed"), "{}", app.message);
    app.build();
    assert!(
        texts_with_badges(&app, 1)
            .join("\n")
            .contains("Finished · new result")
    );
}

#[test]
fn palette_homes_sessions_card_review_leaves_the_question_unanswered() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_question_row(
        SESSION_A,
        "Which provider?"
    )]));
    let mut app = fixture.app();
    app.handle(Key::Char('v'), &mut FakeScreen::default());
    app.build();
    assert!(app.message.contains("marked reviewed"), "{}", app.message);
    assert!(app.sessions[0].needs_attention());
    assert!(
        !texts_with_badges(&app, 1)
            .join("\n")
            .contains("new question")
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn palette_homes_sessions_details_open_cross_session_views() {
    for key in ['u', 'Q'] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([continuity_result_row(
            "11111111111111111111111111111111",
            "waiting"
        )]));
        let mut app = fixture.app();
        app.handle(Key::Char('i'), &mut FakeScreen::default());
        app.handle(Key::Char(key), &mut FakeScreen::typing(&["0"]));
        let text = buffer_rows(&render(&mut app, 100, 45)).join("\n");
        assert!(
            text.contains(if key == 'u' { "Latest turn" } else { "Queue" }),
            "{text}"
        );
    }
}

#[test]
fn tab_digits_and_function_keys_open_the_tabs() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let screen = &mut FakeScreen::default();
    assert_eq!(app.focus, 1);
    for (key, panel) in [
        (Key::Char('2'), 3),
        (Key::Char('3'), 4),
        (Key::Char('4'), 5),
        (Key::Char('5'), 6),
        (Key::Char('1'), 1),
        (Key::F(2), 3),
        (Key::F(3), 4),
        (Key::F(4), 5),
        (Key::F(5), 6),
        (Key::F(1), 1),
    ] {
        app.handle(key.clone(), screen);
        assert_eq!(app.focus, panel, "{key:?}");
    }
    for removed in [Key::Tab, Key::BackTab, Key::Char('0'), Key::Char('6')] {
        app.handle(removed.clone(), screen);
        assert_eq!(app.focus, 1, "{removed:?} no longer moves between panels");
    }
}

#[test]
fn coming_back_to_a_panel_shows_it_from_the_top() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.panels[1].scroll = 7;
    app.focus_panel(3);
    app.focus_panel(1);
    assert_eq!(app.panels[1].scroll, 0);
    app.panels[1].scroll = 3;
    app.focus_panel(1);
    assert_eq!(app.panels[1].scroll, 3);
}

#[test]
fn right_moves_into_the_session_from_any_panel() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for panel in [0, 3] {
        app.focus_panel(panel);
        app.handle(Key::Right, &mut FakeScreen::default());
    }
    assert_eq!(fixture.calls("cockpit").len(), 2);
    assert!(
        fixture
            .calls("cockpit")
            .iter()
            .all(|call| call == "cockpit focus-viewer")
    );
}

#[test]
fn sessions_next_attention_works_after_switching_tabs() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('1'), &mut FakeScreen::default());
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    assert_eq!(app.focus, 1);
    assert_eq!(app.message, "nothing needs you");
}

#[test]
fn help_shows_the_sessions_keys_then_everywhere_then_what_the_signs_mean() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(Key::Char('?'), &mut FakeScreen::default());
    let text = buffer_rows(&render(&mut app, 90, 60)).join("\n");
    let order: Vec<usize> = [
        "Help · Sessions",
        "THIS TAB",
        "later: hide this ask for 20 min (never approves)",
        "EVERYWHERE",
        "Sessions · Todo · Ideas",
        "WHAT THE SIGNS MEAN",
        "your approved command runs",
        "esc  close",
    ]
    .iter()
    .map(|needle| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing:\n{text}"))
    })
    .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert_eq!(
        text.matches("new session: blank or Claude Design canvas")
            .count(),
        1,
        "N listed twice:\n{text}"
    );
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(
        !buffer_rows(&render(&mut app, 90, 60))
            .join("\n")
            .contains("Help · Sessions")
    );
}

#[test]
fn help_on_todo_shows_todos_keys_and_no_card_signs() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(3);
    app.handle(Key::Char('?'), &mut FakeScreen::default());
    let text = buffer_rows(&render(&mut app, 90, 70)).join("\n");
    assert!(
        text.contains("Help · Todo") && text.contains("raise a typed item's priority"),
        "{text}"
    );
    assert!(
        text.contains("new session: blank or Claude Design canvas")
            && !text.contains("WHAT THE SIGNS MEAN"),
        "{text}"
    );
}

#[test]
fn help_scrolls_to_the_last_sign_and_back_in_a_short_terminal() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('?'), &mut FakeScreen::default());
    for _ in 0..200 {
        app.handle(Key::Down, &mut FakeScreen::default());
        render(&mut app, 45, 18);
    }
    let bottom = buffer_rows(&render(&mut app, 45, 18)).join("\n");
    assert!(
        app.help_open && bottom.contains("queued messages"),
        "{bottom}"
    );
    for _ in 0..200 {
        app.handle(Key::Up, &mut FakeScreen::default());
    }
    let top = buffer_rows(&render(&mut app, 45, 18)).join("\n");
    assert!(
        top.contains("Help · Sessions") && top.contains("THIS TAB"),
        "{top}"
    );
    app.handle(Key::Esc, &mut FakeScreen::default());
    app.handle(Key::Char('?'), &mut FakeScreen::default());
    assert!(
        buffer_rows(&render(&mut app, 45, 18))
            .join("\n")
            .contains("Help · Sessions")
    );
}

#[test]
fn queue_details_cannot_drop_a_sessions_prompts() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    crate::prompts::add(SESSION_A, "keep this").unwrap();
    let mut app = fixture.app();
    continuity_action(&mut app, "-\tqueue", &[]);
    app.handle(
        Key::Char('d'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(crate::prompts::count(SESSION_A), 1);
}

#[test]
fn vanished_session_details_cannot_act_on_the_replacement_selection() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    crate::prompts::add(SESSION_B, "keep this").unwrap();
    fixture.snapshot(json!([session(
        SESSION_B,
        "waiting",
        "runway",
        "/r/runway",
        2
    )]));
    app.build();
    app.handle(
        Key::Char('d'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(crate::prompts::count(SESSION_B), 1);
}

#[test]
fn l_hides_a_question_for_twenty_minutes_in_quiet_and_l_again_brings_it_back() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_question_row(
        SESSION_A,
        "Which provider?"
    )]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(Key::Char('l'), &mut FakeScreen::default());
    assert_eq!(
        app.message,
        "later: back in 20 min; nothing was answered or approved"
    );
    let rows = buffer_rows(&render(&mut app, 110, 45)).join("\n");
    assert!(
        rows.contains("‖ Later · back in 20m") && rows.contains("QUIET"),
        "{rows}"
    );
    app.handle(Key::Char('l'), &mut FakeScreen::default());
    assert_eq!(
        app.message,
        "brought back; nothing was answered or approved"
    );
    assert!(
        buffer_rows(&render(&mut app, 110, 45))
            .join("\n")
            .contains("NEEDS YOU")
    );
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(" say "))
    );
}

#[test]
fn l_never_hides_a_command_waiting_for_approval() {
    let fixture = Fixture::new();
    let mut row = continuity_question_row(SESSION_A, "Which provider?");
    row["ask_to_run"] = json!("approval");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(Key::Char('l'), &mut FakeScreen::default());
    assert!(
        app.message
            .contains("approvals and command input stay immediate"),
        "{}",
        app.message
    );
    assert!(
        buffer_rows(&render(&mut app, 110, 45))
            .join("\n")
            .contains("NEEDS YOU")
    );
}

#[test]
fn w_writes_a_private_note_and_a_dash_removes_it() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(
        Key::Char('w'),
        &mut FakeScreen::typing(&["check the gate after lunch"]),
    );
    assert_eq!(app.message, "note saved");
    assert_eq!(
        continuity_saved(&fixture)["sessions"][SESSION_A]["note"],
        "check the gate after lunch"
    );
    app.handle(Key::Char('w'), &mut FakeScreen::typing(&[""]));
    assert_eq!(app.message, "unchanged");
    app.handle(Key::Char('w'), &mut FakeScreen::typing(&["-"]));
    assert_eq!(app.message, "note removed");
    assert!(continuity_saved(&fixture)["sessions"][SESSION_A]["note"].is_null());
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(" say "))
    );
}

#[test]
fn d_on_a_card_drops_its_queued_messages_after_a_yes() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    crate::prompts::add(SESSION_A, "queued thought").unwrap();
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(
        Key::Char('d'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert_eq!(crate::prompts::count(SESSION_A), 1);
    app.handle(
        Key::Char('d'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert_eq!(crate::prompts::count(SESSION_A), 0);
}

#[test]
fn k_on_a_card_opens_its_links_and_up_still_moves() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "working", "runway", "/r/runway", 1),
        session(SESSION_B, "working", "shop", "/r/shop", 2)
    ]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app.panels[1].selected = 1;
    app.handle(Key::Char('k'), &mut FakeScreen::default());
    assert_eq!(app.panels[1].selected, 1);
    assert!(
        app.team_view.is_some() || app.message.starts_with("Team view"),
        "{}",
        app.message
    );
    app.team_view = None;
    app.handle(Key::Up, &mut FakeScreen::default());
    assert_eq!(app.panels[1].selected, 0);
}

#[test]
fn status_opens_claudes_official_status_page() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    select_status_action(&mut app, "Claude");
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(app.focus, 0);
    assert_eq!(
        fixture.wait_for_calls("xdg-open", 1),
        vec![format!("xdg-open {}", crate::status::PAGE)]
    );
}

#[test]
fn status_opens_codexs_official_status_page() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    select_status_action(&mut app, "Codex");
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(app.focus, 0);
    assert_eq!(
        fixture.wait_for_calls("xdg-open", 1),
        vec!["xdg-open https://status.openai.com/".to_string()]
    );
}

#[test]
fn the_rare_actions_are_left_without_a_key() {
    // The audit of October 2026 took these off the keyboard; the redesign key map gave R, m and N
    // a meaning again (restart, remote control, new session).
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "needs-you",
        "runway",
        "/r/runway",
        1
    )]));
    crate::prompts::add(SESSION_A, "queued thought").unwrap();
    let mut app = fixture.app();
    for (panel, key) in [
        (1, 'M'),
        (1, 'Y'),
        (1, 'f'),
        (1, 'A'),
        (1, 'V'),
        (1, 'D'),
        (1, 'K'),
        (1, '/'),
        (0, 'o'),
        (0, 'r'),
        (2, 'r'),
    ] {
        app.focus_panel(panel);
        app.handle(
            Key::Char(key),
            &mut FakeScreen::answering(&[Key::Char('y')]),
        );
        assert_eq!(app.message, "", "{key} did something in panel {panel}");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.calls("cockpit").is_empty() && fixture.calls("xdg-open").is_empty());
    assert_eq!(crate::prompts::count(SESSION_A), 1);
}

#[test]
fn a_footer_message_uses_the_foreground_not_amber() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.message = "restarting runway …".into();
    let buffer = render(&mut app, 60, 20);
    assert_eq!(buffer[(2, 19)].fg, crate::app::color(crate::app::FG));
}

#[test]
fn ctrl_c_and_escape_cancel_a_prompt_and_enter_sends_it() {
    use crate::app::{LineEdit, edit_line};
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut line = String::new();
    for typed in "hix".chars() {
        assert_eq!(
            edit_line(&mut line, KeyCode::Char(typed), KeyModifiers::NONE),
            LineEdit::Typing
        );
    }
    edit_line(&mut line, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(line, "hi");
    assert_eq!(
        edit_line(&mut line, KeyCode::Char('c'), KeyModifiers::CONTROL),
        LineEdit::Cancelled
    );
    assert_eq!(
        edit_line(&mut line, KeyCode::Esc, KeyModifiers::NONE),
        LineEdit::Cancelled
    );
    assert_eq!(
        edit_line(&mut line, KeyCode::Enter, KeyModifiers::NONE),
        LineEdit::Done
    );
}

#[test]
fn a_key_press_redraws_without_asking_tmux_or_pgrep() {
    // Performance 1: a full tick per j/k cost 15 to 40 ms.
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r/runway", 100),
        session(SESSION_B, "waiting", "homelab", "/r/homelab", 100)
    ]));
    fixture.shim("pgrep", "exit 1");
    let mut app = fixture.app();
    let reads = |fixture: &Fixture| fixture.calls("tmux").len() + fixture.calls("pgrep").len();
    let before = reads(&fixture);
    app.handle(Key::Down, &mut FakeScreen::default());
    app.redraw();
    assert_eq!(reads(&fixture), before);
    assert_eq!(app.panels[1].selected, 1);
}

#[test]
fn palette_homes_canvas_validates_the_link_before_launching_in_the_project() {
    for link in ["not-a-link", "https://claude.ai/artifacts/123"] {
        let fixture = Fixture::new();
        let project = fixture.root.join("home/IdeaProjects/runway");
        fs::create_dir_all(&project).unwrap();
        let mut app = fixture.app();
        app.handle(Key::Char('N'), &mut FakeScreen::default());
        for key in [Key::BackTab, Key::Right, Key::Tab, Key::Tab] {
            app.handle(key, &mut FakeScreen::default());
        }
        type_text(&mut app, link);
        app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
        assert!(app.message.contains("not a canvas link"), "{}", app.message);
        assert!(
            buffer_rows(&render(&mut app, 100, 45))
                .join("\n")
                .contains("Canvas link")
        );
        app.handle(Key::Esc, &mut FakeScreen::default());
        assert!(fixture.calls("cockpit").is_empty());
    }
}

#[test]
fn palette_homes_canvas_uses_the_existing_canvas_launcher() {
    let fixture = Fixture::new();
    let project = fixture.root.join("home/IdeaProjects/runway");
    fs::create_dir_all(&project).unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    for key in [Key::BackTab, Key::Right, Key::Tab, Key::Tab] {
        app.handle(key, &mut FakeScreen::default());
    }
    let link = "https://claude.ai/artifact/12345678-1234-1234-1234-123456789abc";
    type_text(&mut app, link);
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    let call = fixture.wait_for_calls("cockpit", 1).remove(0);
    assert!(
        call.starts_with(&format!("cockpit new {} ", project.display())) && call.contains(link),
        "{call}"
    );
}

#[test]
fn palette_homes_canvas_round_trip_keeps_blank_session_choices() {
    for (steps, agent) in [(0, "claude"), (1, "opencode"), (2, "codex")] {
        let fixture = Fixture::new();
        let project = fixture.root.join("home/IdeaProjects/runway");
        fs::create_dir_all(&project).unwrap();
        let mut app = fixture.app();
        app.handle(Key::Char('N'), &mut FakeScreen::default());
        type_text(&mut app, "runway");
        app.handle(Key::Tab, &mut FakeScreen::default());
        for _ in 0..steps {
            app.handle(Key::Right, &mut FakeScreen::default());
        }
        app.handle(Key::Tab, &mut FakeScreen::default());
        type_text(&mut app, "Keep this message");
        for key in [
            Key::Tab,
            Key::Char(' '),
            Key::Tab,
            Key::Right,
            Key::Tab,
            Key::Tab,
        ] {
            app.handle(key, &mut FakeScreen::default());
        }
        type_text(&mut app, "unfinished link");
        for key in [Key::Tab, Key::Left, Key::Ctrl('s')] {
            app.handle(key, &mut FakeScreen::default());
        }
        assert_eq!(
            fixture.wait_for_calls("cockpit", 1),
            vec![format!(
                "cockpit --agent {agent} new {} Keep this message",
                project.display()
            )]
        );
    }
}

#[test]
fn retired_palette_and_search_keys_never_open_a_picker() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for panel in 0..5 {
        app.focus_panel(panel);
        let mut screen = FakeScreen::typing(&["unused"]);
        app.handle(Key::Char(':'), &mut screen);
        app.handle(Key::Char('/'), &mut screen);
        assert_eq!(screen.typed.len(), 1);
    }
}

#[test]
fn new_session_keeps_the_focused_field_visible_in_a_short_column() {
    let fixture = Fixture::new();
    for index in 0..8 {
        fs::create_dir_all(
            fixture
                .root
                .join(format!("home/IdeaProjects/project-{index}")),
        )
        .unwrap();
    }
    let mut app = fixture.app();
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    for key in [Key::BackTab, Key::Right, Key::Tab, Key::Tab] {
        app.handle(key, &mut FakeScreen::default());
    }
    let canvas = buffer_rows(&render(&mut app, 40, 18)).join("\n");
    assert!(canvas.contains("Canvas link"), "{canvas}");
    assert!(
        canvas.contains("default agent") && canvas.contains("without a new clone"),
        "{canvas}"
    );
    for key in [Key::Tab, Key::Left, Key::Tab, Key::Tab, Key::Tab] {
        app.handle(key, &mut FakeScreen::default());
    }
    type_text(&mut app, "Visible message");
    let blank = buffer_rows(&render(&mut app, 40, 18)).join("\n");
    assert!(
        blank.contains("FIRST MESSAGE") && blank.contains("Visible message"),
        "{blank}"
    );
}

#[test]
fn n_starts_a_blank_session_in_its_own_clone_with_the_first_message() {
    let fixture = Fixture::new();
    for project in ["runway", "shop"] {
        fs::create_dir_all(fixture.root.join("home/IdeaProjects").join(project)).unwrap();
    }
    let mut app = fixture.app();
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    let form = buffer_rows(&render(&mut app, 90, 50)).join("\n");
    assert!(
        form.contains("New session")
            && form.contains("PROJECT")
            && form.contains("[x] in its own clone (recommended)"),
        "{form}"
    );
    type_text(&mut app, "sho");
    app.handle(Key::Tab, &mut FakeScreen::default());
    app.handle(Key::Tab, &mut FakeScreen::default());
    type_text(&mut app, "Look at the proxy");
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    let call = fixture.wait_for_calls("cockpit", 1).remove(0);
    let shop = fixture.root.join("home/IdeaProjects/shop");
    assert!(
        call.starts_with(&format!(
            "cockpit --agent claude parallel {} parallel-",
            shop.display()
        )) && call.ends_with(" Look at the proxy"),
        "{call}"
    );
    assert!(
        !buffer_rows(&render(&mut app, 90, 50))
            .join("\n")
            .contains("New session")
    );
}

#[test]
fn n_without_its_own_clone_starts_the_picked_agent_in_the_project() {
    let fixture = Fixture::new();
    let project = fixture.root.join("home/IdeaProjects/runway");
    fs::create_dir_all(&project).unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    app.handle(Key::Tab, &mut FakeScreen::default());
    app.handle(Key::Right, &mut FakeScreen::default());
    for _ in 0..2 {
        app.handle(Key::Tab, &mut FakeScreen::default());
    }
    app.handle(Key::Char(' '), &mut FakeScreen::default());
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!(
            "cockpit --agent {} new {}",
            agents::AGENTS[1].name,
            project.display()
        )]
    );
}

#[test]
fn n_preselects_the_saved_default_agent() {
    for agent in agents::names() {
        let fixture = Fixture::new();
        let project = fixture.root.join("home/IdeaProjects/runway");
        fs::create_dir_all(&project).unwrap();
        fixture.write(
            "state/cockpit/settings.json",
            &json!({"default-agent":agent}).to_string(),
        );
        let mut app = fixture.app();
        app.handle(Key::Char('N'), &mut FakeScreen::default());
        app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
        assert!(
            fixture.wait_for_calls("cockpit", 1)[0].starts_with(&format!(
                "cockpit --agent {agent} parallel {} ",
                project.display()
            ))
        );
    }
}

#[test]
fn escape_closes_the_new_session_form_without_starting_anything() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("home/IdeaProjects/runway")).unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    type_text(&mut app, "zzz");
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(app.message, "no project matches: change the filter");
    app.handle(Key::Esc, &mut FakeScreen::default());
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
    assert!(
        !buffer_rows(&render(&mut app, 90, 50))
            .join("\n")
            .contains("New session")
    );
}

#[test]
fn help_on_sessions_lists_capital_i() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(Key::Char('?'), &mut FakeScreen::default());
    let text = buffer_rows(&render(&mut app, 120, 80)).join("\n");
    assert!(
        text.contains("open the images from its latest reply"),
        "{text}"
    );
}

#[test]
fn the_new_session_form_has_one_background_behind_its_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    let buffer = render(&mut app, 80, 30);
    assert!(buffer_rows(&buffer).join("\n").contains("START FROM"));
    assert_one_background(&buffer, 2..28, 2..78, &[DESIGNED_HIGHLIGHT]);
}

#[test]
fn help_has_one_background_behind_its_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('?'), &mut FakeScreen::default());
    assert!(app.help_open);
    assert_one_background(
        &render(&mut app, 80, 30),
        2..28,
        0..79,
        &[DESIGNED_HIGHLIGHT],
    );
}
