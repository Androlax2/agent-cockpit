use super::*;

#[test]
fn activity_labels_follow_reported_state_without_guessing() {
    for (state, phase, expected) in [
        ("working", "approval", "Approval needed"),
        ("working", "input", "Input needed"),
        ("needs-you", "running", "Needs you"),
        ("waiting", "running", "Command running"),
        ("working", "", "Working"),
        ("waiting", "", "Ready"),
        ("unknown", "", "Status unknown"),
        ("recent", "", "Status unknown"),
        ("working", "unrecognized", "Working"),
    ] {
        let mut row = session(SESSION_A, state, "dotfiles", "/r/dotfiles", 1);
        row["ask_to_run"] = json!(phase);
        let reported = sessions::Session::from_value(&row).unwrap();
        assert_eq!(sessions::activity_label(&reported), expected);
    }
}

#[test]
fn background_work_keeps_a_waiting_session_working() {
    for (agents, background) in [(1, 0), (0, 1), (2, 3)] {
        let mut row = session(SESSION_A, "waiting", "shop", "/r/shop", 1);
        row["agents"] = json!(agents);
        row["background"] = json!(background);
        let reported = sessions::Session::from_value(&row).unwrap();
        assert_eq!(sessions::activity_label(&reported), "Working");
        assert!(!sessions::is_idle(&reported, sessions::IDLE_AFTER_S + 2));
    }
}

#[test]
fn background_work_does_not_hide_requests_for_attention() {
    for (state, phase, expected) in [
        ("waiting", "approval", "Approval needed"),
        ("waiting", "input", "Input needed"),
        ("needs-you", "", "Needs you"),
        ("waiting", "running", "Command running"),
    ] {
        let reported = sessions::Session {
            state: state.into(),
            ask_to_run: phase.into(),
            background: 1,
            ..Default::default()
        };
        assert_eq!(sessions::activity_label(&reported), expected);
    }
}

#[test]
fn activity_text_does_not_reuse_stale_requests_or_completed_outcomes() {
    for state in ["working", "waiting", "unknown"] {
        let mut row = session(SESSION_A, state, "dotfiles", "/r/dotfiles", 1);
        row["detail"] = json!("old permission question");
        row["outcome"] = json!("tests passed");
        let reported = sessions::Session::from_value(&row).unwrap();
        let activity = sessions::activity_text(&reported);
        assert!(
            !activity.contains("old permission question") && !activity.contains("tests passed")
        );
    }
    let mut row = session(SESSION_A, "needs-you", "dotfiles", "/r/dotfiles", 1);
    row["detail"] = json!("Choose\n  a wireframe");
    assert_eq!(
        sessions::activity_text(&sessions::Session::from_value(&row).unwrap()),
        "Choose a wireframe"
    );
}

#[test]
fn attention_navigation_walks_the_asks_without_reordering_sessions() {
    let fixture = Fixture::new();
    let mut approval = session(SESSION_B, "working", "dotfiles", "/r/dotfiles", 2);
    approval["ask_to_run"] = json!("approval");
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1),
        approval,
        session(
            "cccccccc-0000-4000-8000-000000000000",
            "working",
            "dotfiles",
            "/r/dotfiles",
            3
        ),
        session(OPENCODE_SESSION, "needs-you", "dotfiles", "/r/dotfiles", 4),
        external_session(
            "dddddddd-0000-4000-8000-000000000000",
            "needs-you",
            "dotfiles",
            "/r/dotfiles",
            5
        ),
    ]));
    let mut app = fixture.app();
    let order: Vec<_> = app
        .sessions
        .iter()
        .map(|session| session.session.clone())
        .collect();
    // NEEDS YOU lists the oldest ask first, and the selection starts on it.
    for (count, expected) in [OPENCODE_SESSION, SESSION_B, OPENCODE_SESSION]
        .into_iter()
        .enumerate()
    {
        app.handle(Key::Char('n'), &mut FakeScreen::default());
        assert_eq!(app.sessions[app.panels[1].selected].session, expected);
        assert_eq!(
            fixture.wait_for_calls("cockpit", count + 1)[count],
            format!("cockpit attend {expected}")
        );
    }
    assert_eq!(
        app.sessions
            .iter()
            .map(|session| session.session.clone())
            .collect::<Vec<_>>(),
        order
    );
}

#[test]
fn attention_navigation_does_nothing_when_sessions_are_only_ready_or_unknown() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1),
        session(SESSION_B, "unknown", "dotfiles", "/r/dotfiles", 2),
    ]));
    let mut app = fixture.app();
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    assert_eq!(app.message, "nothing needs you");
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn reported_and_unreported_tmux_sessions_are_listed() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.shim("tmux", "printf 'claude-retry-7\\t%%5\\t/r/opusline\\t✳ Fix login\\t100\\nother\\t%%6\\t/x\\tsh\\t1\\n'");
    let app = fixture.app();
    assert_eq!(
        app.sessions
            .iter()
            .map(|s| (s.project.as_str(), s.state.as_str()))
            .collect::<Vec<_>>(),
        vec![("runway", "waiting"), ("opusline", "unknown")]
    );
    assert_eq!(app.sessions[1].task, "Fix login");
}

#[test]
fn claudes_internal_messages_are_not_shown_as_tasks() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "working", "runway", "/r", 1);
    row["task"] = json!("<task-notification> done");
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    assert!(
        !texts(&app, 1)
            .iter()
            .any(|line| line.contains("task-notification"))
    );
}

#[test]
fn cards_keep_their_place_when_states_change() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "working", "runway", "/r", 1),
        session(SESSION_B, "needs-you", "homelab", "/h", 2)
    ]));
    let first = fixture
        .app()
        .sessions
        .iter()
        .map(|s| s.session.clone())
        .collect::<Vec<_>>();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r", 1),
        session(SESSION_B, "working", "homelab", "/h", 2)
    ]));
    let second = fixture
        .app()
        .sessions
        .iter()
        .map(|s| s.session.clone())
        .collect::<Vec<_>>();
    assert_eq!(first, second);
}

#[test]
fn an_updated_session_shows_its_badge_and_the_restart_hint() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "runway", "/r", 1);
    row["update"] = json!(true);
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    assert!(
        app.panels[1]
            .fixed_bottom
            .iter()
            .flat_map(|line| &line.right)
            .any(|part| part.text == "⬆ update")
    );
    assert!(
        app.sessions_hints()
            .contains(&("R", "restart to apply the update"))
    );
}

#[test]
fn a_full_context_suggests_compact_on_the_card() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    fixture.write(
        &format!("home/.cache/claude/statusline-{SESSION_A}.json"),
        r#"{"model": "Opus 5.5", "effort": "high", "context": 93}"#,
    );
    let app = fixture.app();
    assert!(
        app.panels[1]
            .fixed_bottom
            .iter()
            .flat_map(|line| &line.right)
            .any(|part| part.text == "/compact")
    );
    assert!(app.sessions_hints().contains(&("C", "/compact")));
}

#[test]
fn a_card_needing_you_says_what_it_asks() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "needs-you", "runway", "/r", 1);
    row["request"] = json!("Claude needs your permission to use Bash");
    row["detail"] = json!("Bash command · npm test");
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    assert!(
        texts(&app, 1)
            .iter()
            .any(|line| line.contains("Bash command · npm test"))
    );
}

#[test]
fn sessions_needing_attention_keep_amber_markers_even_when_selected() {
    use crate::app::{YELLOW, color};
    use ratatui::style::Modifier;
    let fixture = Fixture::new();
    for (state, phase) in [
        ("needs-you", ""),
        ("working", "approval"),
        ("working", "input"),
    ] {
        for grouped in [false, true] {
            for width in [60, 140] {
                let mut row = session(SESSION_A, state, "runway", "/r/runway", 1);
                row["ask_to_run"] = json!(phase);
                let other_project = if grouped { "runway" } else { "other" };
                fixture.snapshot(json!([
                    row,
                    session(SESSION_B, "working", other_project, "/r/other", 2)
                ]));
                let mut app = fixture.app();
                app.focus = 1;
                app.has_pane_focus = true;
                app.panels[1].selected = 0;
                let buffer = render(&mut app, width, 45);
                let tabs: String = (0..width)
                    .map(|column| buffer[(column, 1)].symbol())
                    .collect();
                assert!(tabs.contains("Sessions  1 "), "{state}/{phase}: {tabs}");
                let card_rows: Vec<_> = app
                    .hit_lines
                    .iter()
                    .filter(|(_, (panel, item))| *panel == 1 && *item == Some(0))
                    .map(|(row, _)| *row)
                    .collect();
                let first = *card_rows.iter().min().unwrap();
                for row in card_rows {
                    assert_eq!(
                        buffer[(1, row)].symbol(),
                        "▌",
                        "{state}/{phase}: missing attention rail"
                    );
                    assert_eq!(buffer[(1, row)].fg, color(YELLOW));
                }
                assert_eq!(buffer[(2, first)].symbol(), "!");
                assert_eq!(buffer[(2, first)].fg, color(YELLOW));
                let bar = *app
                    .hit_lines
                    .iter()
                    .filter(|(_, (panel, item))| *panel == 1 && *item == Some(0))
                    .map(|(row, _)| row)
                    .max()
                    .unwrap();
                let text: String = (0..width)
                    .map(|column| buffer[(column, bar)].symbol())
                    .collect();
                let expected_label = match phase {
                    "approval" => "Approve a command",
                    "input" => "A running command needs your input",
                    _ => "An answer or permission is needed",
                };
                assert!(text.contains(expected_label), "{state}/{phase}: {text}");
                let label_start = (0..width)
                    .find(|column| buffer[(*column, bar)].symbol() == &expected_label[..1])
                    .unwrap();
                assert_eq!(
                    buffer[(label_start, bar)].bg,
                    color(YELLOW),
                    "the ask keeps its amber bar when selected"
                );
                assert!(buffer[(label_start, bar)].modifier.contains(Modifier::BOLD));
                assert_eq!(
                    app.sessions[0].state, state,
                    "attention must not replace the agent's state"
                );
            }
        }
    }
}

#[test]
fn running_commands_do_not_ask_for_attention() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "working", "runway", "/r/runway", 1);
    row["ask_to_run"] = json!("running");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    let buffer = render(&mut app, 100, 45);
    let text: String = (0..45)
        .flat_map(|row| (0..100).map(move |column| (column, row)))
        .map(|position| buffer[position].symbol())
        .collect();
    assert!(!text.contains("NEEDS YOU"));
    let marker = (0..45)
        .find(|row| buffer[(2, *row)].symbol() == "▶")
        .unwrap();
    assert_eq!(buffer[(2, marker)].fg, crate::app::color(crate::app::BLUE));
}

#[test]
fn running_commands_use_a_working_marker_even_when_the_agent_is_ready() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1);
    row["ask_to_run"] = json!("running");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    let buffer = render(&mut app, 100, 40);
    let row = app
        .hit_lines
        .iter()
        .filter(|(_, (panel, item))| *panel == 1 && *item == Some(0))
        .map(|(row, _)| *row)
        .min()
        .unwrap();
    assert_eq!(buffer[(2, row)].symbol(), "▶");
    assert_eq!(buffer[(2, row)].fg, crate::app::color(crate::app::BLUE));
}

#[test]
fn sessions_are_grouped_by_attention_across_projects_with_counts() {
    let fixture = Fixture::new();
    let mut asking = session(SESSION_A, "needs-you", "opusline", "/r/opusline", 1);
    asking["request"] = json!("Which layout?");
    let mut finished = opencode_session("waiting", "runway", 0);
    finished["since"] = json!(util::now_secs());
    fixture.snapshot(json!([
        finished,
        asking,
        session(SESSION_B, "working", "notes", "/r/notes", 2)
    ]));
    let app = fixture.app();
    let headings: Vec<String> = app.panels[1]
        .lines
        .iter()
        .filter(|line| line.item.is_none() && !line.texts().is_empty())
        .map(|line| line.texts())
        .collect();
    assert_eq!(
        &headings[..3],
        ["NEEDS YOU · 1", "WORKING · 1", "QUIET · 1"]
    );
    let order: Vec<String> = app
        .sidebar_rows
        .iter()
        .map(|row| match row {
            crate::teams::sidebar::SidebarRow::Session { session_id } => session_id.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(order, [SESSION_A, SESSION_B, OPENCODE_SESSION]);
}

#[test]
fn a_card_reads_doing_now_then_project_and_task_then_the_ask() {
    let fixture = Fixture::new();
    let mut asking = session(SESSION_A, "needs-you", "opusline", "/r/opusline", 1);
    asking["request"] = json!("Which layout?");
    asking["label"] = json!("Improve the cockpit UI");
    asking["doing"] = json!("Asking which layout to sketch");
    asking["doing_at"] = json!(util::now_secs());
    fixture.snapshot(json!([asking]));
    let app = fixture.app();
    let lines: Vec<&crate::app::Line> = app.panels[1]
        .lines
        .iter()
        .filter(|line| line.item == Some(0))
        .collect();
    assert_eq!(lines[0].texts(), "!Asking which layout to sketch");
    assert_eq!(lines[1].texts(), "opusline › Improve the cockpit UI");
    assert_eq!(lines[2].texts(), " Which layout?");
    assert_eq!(lines[2].right[0].text, "⏎ answer ");
    assert_eq!(
        lines[2].fill.unwrap().1.bg,
        crate::app::style(crate::app::BLACK)
            .bg(ratatui::style::Color::Indexed(crate::app::YELLOW))
            .bg
    );
}

#[test]
fn chief_question_grace_hides_only_the_cards_attention_not_its_blocked_state() {
    let fixture = Fixture::new();
    let now = util::now_secs();
    let mut row = opencode_session("needs-you", "runway", 0);
    row["since"] = json!(now - 1);
    row["needs_input"] = json!(true);
    row["request"] = json!("Which layout?");
    row["chief_question_until"] = json!(now + 59);
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    assert!(card_text(&app, 0).contains("Chief is reviewing"));
    assert!(!app.panels[1].lines.iter().any(|line| line.fill.is_some()));
    assert_eq!(app.sessions[0].state, "needs-you");
    assert!(app.sessions[0].needs_input);
}

#[test]
fn expired_or_invalid_chief_grace_and_host_approvals_still_show_the_ask() {
    for case in ["expired", "too long", "approval", "input"] {
        let fixture = Fixture::new();
        let now = util::now_secs();
        let mut row = opencode_session("needs-you", "runway", 0);
        row["since"] = json!(now - 1);
        row["needs_input"] = json!(true);
        row["request"] = json!("Which layout?");
        row["chief_question_until"] = json!(match case {
            "expired" => now - 1,
            "too long" => now + 3600,
            _ => now + 59,
        });
        if matches!(case, "approval" | "input") {
            row["ask_to_run"] = json!(case);
        }
        fixture.snapshot(json!([row]));
        let app = fixture.app();
        assert!(
            app.panels[1].lines.iter().any(|line| line.fill.is_some()),
            "{case}"
        );
        assert!(!card_text(&app, 0).contains("Chief is reviewing"), "{case}");
    }
}

#[test]
fn needs_you_lists_the_oldest_ask_first() {
    let fixture = Fixture::new();
    let mut newer = session(SESSION_A, "needs-you", "opusline", "/r/opusline", 1);
    newer["since"] = json!(util::now_secs() - 60);
    let mut older = session(SESSION_B, "needs-you", "runway", "/r/runway", 2);
    older["since"] = json!(util::now_secs() - 600);
    fixture.snapshot(json!([newer, older]));
    let app = fixture.app();
    assert!(
        matches!(&app.sidebar_rows[0], crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_B)
    );
}

#[test]
fn a_pinned_card_comes_first_inside_its_group_only() {
    let fixture = Fixture::new();
    let mut older = session(SESSION_A, "needs-you", "opusline", "/r/opusline", 1);
    older["since"] = json!(util::now_secs() - 600);
    let mut pinned = session(SESSION_B, "needs-you", "runway", "/r/runway", 2);
    pinned["since"] = json!(util::now_secs() - 60);
    fixture.snapshot(json!([
        older,
        pinned,
        session(OPENCODE_SESSION, "working", "notes", "/r/notes", 3)
    ]));
    fixture.view(OPENCODE_SESSION);
    let mut app = fixture.app();
    app.view.toggle_pin(SESSION_B);
    app.view.toggle_pin(OPENCODE_SESSION);
    app.redraw();
    let order: Vec<&str> = app
        .sessions
        .iter()
        .map(|session| session.session.as_str())
        .collect();
    assert_eq!(
        order,
        [SESSION_B, SESSION_A, OPENCODE_SESSION],
        "a pin never lifts a card above NEEDS YOU"
    );
    assert!(card_text(&app, 0).contains("⚲"));
}

#[test]
fn two_asks_on_one_card_show_one_bar_with_a_count() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "needs-you", "opusline", "/r/opusline", 1);
    row["request"] = json!("Which layout?");
    row["ask_to_run"] = json!("approval");
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    let bars: Vec<&crate::app::Line> = app.panels[1]
        .lines
        .iter()
        .filter(|line| line.fill.is_some())
        .collect();
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].texts(), " Approve a command   1 of 2");
    assert_eq!(bars[0].right[0].text, "⏎ review ");
}

#[test]
fn a_stale_doing_now_shows_the_last_tool_and_how_long_since_an_update() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "working", "opusline", "/r/opusline", 1);
    row["doing"] = json!("Running the Rust tests");
    row["doing_at"] = json!(util::now_secs() - 18 * 60);
    row["last_tool"] = json!("Bash  cargo nextest run");
    row["last_tool_at"] = json!(util::now_secs() - 60);
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    assert!(
        card_text(&app, 0).starts_with("◐Bash  cargo nextest run"),
        "{}",
        card_text(&app, 0)
    );
    assert_eq!(card_age(&app, 0), "no update 18m");
}

#[test]
fn a_long_task_is_cut_before_the_project_name() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "working", "stray-coop", "/r/stray-coop", 1);
    row["label"] = json!("Investigate true online co-op on Steam");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    let screen = buffer_text(&render(&mut app, 30, 20));
    assert!(screen.contains("stray-coop › Investig"), "{screen}");
}

#[test]
fn project_names_are_never_amber() {
    for project in ["a", "b", "c", "d", "e", "opusline", "runway", "dotfiles"] {
        assert_ne!(
            crate::app::project_color(project),
            crate::app::YELLOW,
            "{project}"
        );
    }
}

#[test]
fn grouped_todo_cards_keep_the_task_title_after_short_replies() {
    let fixture = Fixture::new();
    fixture.write("repos/clone/.git/HEAD", "ref: refs/heads/todo-1234abcd\n");
    let clone = fixture.path("repos/clone").display().to_string();
    fixture.write("state/cockpit/todo.json", &json!({"items": [{
        "id": "1234abcd", "project": "opusline", "text": "Fix FigCraft reconnect", "status": "done"
    }]}).to_string());
    let mut row = session(SESSION_A, "waiting", "opusline", &clone, 1);
    row["task"] = json!("Yes");
    fixture.snapshot(json!([
        row,
        session(SESSION_B, "working", "opusline", "/r/opusline", 2)
    ]));
    let app = fixture.app();
    let lines = texts(&app, 1);
    assert!(
        lines.contains(&"opusline › Fix FigCraft reconnect".to_string()),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|line| line.contains("Yes")), "{lines:?}");
}

#[test]
fn native_titles_identify_tasks_before_long_todo_prompts() {
    let fixture = Fixture::new();
    fixture.write("repos/clone/.git/HEAD", "ref: refs/heads/todo-1234abcd\n");
    let clone = fixture.path("repos/clone").display().to_string();
    fixture.write("state/cockpit/todo.json", &json!({"items": [{
        "id": "1234abcd", "project": "dotfiles", "text": "Find out which of the many disk caches are safe to delete", "status": "doing"
    }]}).to_string());
    opencode_database_with(
        &fixture,
        &[(
            OPENCODE_SESSION,
            &clone,
            "Free disk space",
            None,
            2_000_000,
            None,
        )],
    );
    let mut row = opencode_session("working", "dotfiles", 1);
    row["cwd"] = json!(clone);
    row["task"] = json!("Yes");
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    assert!(
        card_text(&app, 0).contains("dotfiles › Free disk space"),
        "{}",
        card_text(&app, 0)
    );
    assert!(!texts(&app, 1).iter().any(|line| line.contains("Yes")));
}

#[test]
fn short_labels_keep_the_original_task_title_in_details() {
    let fixture = Fixture::new();
    let original = "Finding disk space hogs and freeing storage without removing project data";
    opencode_database_with(
        &fixture,
        &[(
            OPENCODE_SESSION,
            "/r/dotfiles",
            original,
            None,
            2_000_000,
            None,
        )],
    );
    let mut row = opencode_session("working", "dotfiles", 1);
    row["label"] = json!("Free disk space");
    row["task"] = json!("Yes");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    assert!(card_text(&app, 0).contains("Free disk space"));
    assert!(!card_text(&app, 0).contains("Finding disk"));
    assert_eq!(app.session_task_titles[OPENCODE_SESSION], original);
    assert!(
        app.panels[1]
            .fixed_bottom
            .iter()
            .any(|line| line.texts().contains(original))
    );
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    let details = buffer_rows(&render(&mut app, 140, 60)).join("\n");
    assert!(details.contains("Free disk space") && details.contains(original));
}

#[test]
fn generic_activity_filler_is_absent_from_cards() {
    let fixture = Fixture::new();
    for state in ["working", "waiting", "needs-you", "working"] {
        let mut row = session(SESSION_A, state, "dotfiles", "/r/dotfiles", 1);
        row["label"] = json!("Fix session navigation");
        row["request"] = json!("Which layout do you prefer?");
        fixture.snapshot(json!([
            row,
            session(SESSION_B, "working", "other", "/r/other", 2)
        ]));
        let app = fixture.app();
        let content = texts(&app, 1).join("\n");
        assert!(
            !content.contains("The agent is working")
                && !content.contains("Ready for your next instruction")
        );
        let item = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_A)).unwrap();
        assert!(card_text(&app, item).contains("Fix session navigation"));
        if state == "needs-you" {
            assert!(content.contains("Which layout do you prefer?"));
        }
    }
}

#[test]
fn compact_session_cards_separate_task_identity_from_reported_activity() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "working", "dotfiles", "/r/dotfiles", 1);
    row["task"] = json!("Proceed");
    row["agents"] = json!(3);
    let mut ready = session(SESSION_B, "waiting", "other", "/r/other", 2);
    ready["since"] = json!(util::now_secs());
    fixture.snapshot(json!([row, ready]));
    let app = fixture.app();
    let rows: Vec<_> = app.panels[1]
        .lines
        .iter()
        .filter(|line| line.item == Some(1))
        .collect();
    assert_eq!(rows.len(), 2, "a quiet card has no ask line");
    assert_eq!(rows[0].texts(), "✓Finished");
    assert_eq!(rows[1].texts(), "other");
    assert!(!texts(&app, 1).iter().any(|line| line.contains("Proceed")));
}

#[test]
fn todo_dependencies_replace_ready_and_idle_on_session_cards() {
    let fixture = Fixture::new();
    fixture.write("repos/clone/.git/HEAD", "ref: refs/heads/todo-1234abcd\n");
    let clone = fixture.path("repos/clone").display().to_string();
    for (blocker_status, expected_badge, expected_detail) in [
        ("doing", "queued", "↳Waiting on: Fix profile drift"),
        ("todo", "", "↳Blocked: its todo isn't running · r run it"),
        ("done", "", "✓Finished"),
    ] {
        for (assigned, since) in [(true, util::now_secs()), (false, 1)] {
            fixture.write("state/cockpit/todo.json", &json!({"items": [
                {"id": "1234abcd", "project": "opusline", "text": "Resume deployment", "status": "todo",
                 "session": if assigned { SESSION_A } else { "" }, "after": ["abcd1234"]},
                {"id": "abcd1234", "project": "other", "text": "Fix  profile\ndrift", "status": blocker_status}
            ]}).to_string());
            let cwd = if assigned { "/r/opusline" } else { &clone };
            let mut row = session(SESSION_A, "waiting", "opusline", cwd, 1);
            row["since"] = json!(if blocker_status == "done" {
                util::now_secs()
            } else {
                since
            });
            fixture.snapshot(json!([
                row,
                session(SESSION_B, "working", "opusline", "/r/opusline", 2)
            ]));
            let app = fixture.app();
            let item = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_A)).unwrap();
            let rows: Vec<_> = app.panels[1]
                .lines
                .iter()
                .filter(|line| line.item == Some(item))
                .collect();
            assert_eq!(rows.len(), 2);
            if !expected_badge.is_empty() {
                assert_eq!(
                    rows[0].right[0].text, expected_badge,
                    "{blocker_status}, assigned={assigned}"
                );
            }
            assert_eq!(rows[0].texts(), expected_detail);
            if blocker_status != "done" {
                assert!(
                    rows[0]
                        .right
                        .iter()
                        .all(|part| part.style.fg != crate::app::style(crate::app::GREEN).fg)
                );
                let summary = app.panels[1]
                    .fixed_top
                    .iter()
                    .map(|line| line.texts())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(summary.contains("0 ready"), "{summary}");
            }
        }
    }
}

#[test]
fn todo_dependencies_prioritize_stopped_blockers_and_ignore_completed_ones() {
    let fixture = Fixture::new();
    fixture.write(
        "state/cockpit/todo.json",
        &json!({"items": [
            {"id": "1234abcd", "project": "opusline", "text": "Resume deployment", "status": "todo",
             "session": SESSION_A, "after": ["aaaa1234", "bbbb1234", "cccc1234"]},
            {"id": "aaaa1234", "text": "Running fix", "status": "doing"},
            {"id": "bbbb1234", "text": "Stopped fix", "status": "todo"},
            {"id": "cccc1234", "text": "Completed fix", "status": "done"}
        ]})
        .to_string(),
    );
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        1
    )]));
    let app = fixture.app();
    let rows: Vec<_> = app.panels[1]
        .lines
        .iter()
        .filter(|line| line.item == Some(0))
        .collect();
    assert_eq!(
        rows[0].texts(),
        "↳Blocked: its todo isn't running · r run it"
    );
}

#[test]
fn todo_dependencies_do_not_override_active_or_attention_statuses() {
    let fixture = Fixture::new();
    fixture.write(
        "state/cockpit/todo.json",
        &json!({"items": [
            {"id": "1234abcd", "project": "opusline", "text": "Resume deployment", "status": "todo",
             "session": SESSION_A, "after": ["abcd1234"]},
            {"id": "abcd1234", "text": "Fix profile drift", "status": "todo"}
        ]})
        .to_string(),
    );
    for (state, command, first_line) in [
        ("working", "", "◐Working"),
        ("waiting", "approval", "!Waiting for you"),
        ("waiting", "input", "!Waiting for you"),
        ("needs-you", "", "!Waiting for you"),
        ("waiting", "running", "▶Running your approved command"),
    ] {
        let mut row = session(SESSION_A, state, "opusline", "/r/opusline", 1);
        row["ask_to_run"] = json!(command);
        fixture.snapshot(json!([row]));
        let app = fixture.app();
        let card = app.panels[1]
            .lines
            .iter()
            .find(|line| line.item == Some(0))
            .unwrap();
        assert_eq!(card.texts(), first_line);
        assert!(
            !texts(&app, 1)
                .iter()
                .any(|text| text.contains("Blocked by TODO"))
        );
    }
}

#[test]
fn ready_sessions_left_alone_past_the_idle_threshold_show_as_idle_not_ready() {
    let fixture = Fixture::new();
    let mut idle = session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1);
    idle["since"] = json!(util::now_secs() - sessions::IDLE_AFTER_S - 3600);
    fixture.snapshot(json!([idle]));
    let app = fixture.app();
    let card = app.panels[1]
        .lines
        .iter()
        .find(|line| line.item == Some(0))
        .unwrap();
    assert_eq!(card.texts(), "○Idle 1h · waiting for your next message");
    assert!(
        card.segments
            .iter()
            .all(|segment| segment.2.fg != crate::app::style(crate::app::GREEN).fg),
        "an idle card is not green"
    );
}

#[test]
fn session_summary_counts_idle_sessions_apart_from_ready_ones() {
    let fixture = Fixture::new();
    let mut idle = session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1);
    idle["since"] = json!(util::now_secs() - sessions::IDLE_AFTER_S);
    let mut ready = session(SESSION_B, "waiting", "other", "/r/other", 2);
    ready["since"] = json!(util::now_secs());
    fixture.snapshot(json!([idle, ready]));
    let app = fixture.app();
    let summary = app.panels[1]
        .fixed_top
        .iter()
        .map(|line| line.texts())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        summary.contains("1 ready · 1 idle"),
        "summary was {summary:?}"
    );
    assert!(!summary.contains("unknown"));
}

#[test]
fn attention_summary_stays_visible_while_session_cards_scroll() {
    let fixture = Fixture::new();
    let rows: Vec<_> = (0..20)
        .map(|index| {
            session(
                &format!("session-{index}"),
                if index < 2 { "needs-you" } else { "working" },
                "dotfiles",
                "/r/dotfiles",
                index + 1,
            )
        })
        .collect();
    fixture.snapshot(json!(rows));
    for (width, height) in [(120, 40), (80, 24), (40, 12)] {
        let mut app = fixture.app();
        let before = buffer_rows(&render(&mut app, width, height));
        let summary_row = before
            .iter()
            .position(|row| row.contains("2 need you"))
            .expect("fixed attention summary");
        app.panels[1].selected = 19;
        let after = buffer_rows(&render(&mut app, width, height));
        assert!(app.panels[1].scroll > 0);
        assert!(after[summary_row].contains("2 need you"));
        assert!(
            app.hit_lines
                .values()
                .any(|(panel, item)| *panel == 1 && *item == Some(19)),
            "selected session must remain visible at {width}x{height}"
        );
    }
}

#[test]
fn short_terminal_layout_keeps_the_focused_status_content_visible() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.focus_panel(0);
    let heights = app.layout(12);
    assert!(
        heights[0] >= 5,
        "focused status needs its content rows: {heights:?}"
    );
    assert!(heights.iter().sum::<u16>() <= 10);
    let rendered = buffer_rows(&render(&mut app, 80, 12)).join("\n");
    assert!(
        rendered.contains("Claude") && rendered.contains("Codex"),
        "{rendered}"
    );
}

#[test]
fn narrow_attention_cards_keep_the_marker_and_distinct_task_names() {
    let fixture = Fixture::new();
    let mut rows = Vec::new();
    for (index, (id, branch)) in [(SESSION_A, "Cache-cleanup"), (SESSION_B, "Model-defaults")]
        .into_iter()
        .enumerate()
    {
        fixture.write(
            &format!("repos/{index}/.git/HEAD"),
            &format!("ref: refs/heads/{branch}\n"),
        );
        let cwd = fixture
            .path(&format!("repos/{index}"))
            .display()
            .to_string();
        let mut row = session(id, "working", "dotfiles", &cwd, index as i64 + 1);
        row["ask_to_run"] = json!("approval");
        rows.push(row);
    }
    fixture.snapshot(json!(rows));
    for width in [24, 26, 30, 40] {
        let mut app = fixture.app();
        for (selected, expected) in [(0, "Cach"), (1, "Mode")] {
            app.panels[1].selected = selected;
            let buffer = render(&mut app, width, 24);
            let rows: Vec<u16> = app
                .hit_lines
                .iter()
                .filter(|(_, (panel, item))| *panel == 1 && *item == Some(selected))
                .map(|(row, _)| *row)
                .collect();
            let line = |row: u16| {
                (0..width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            };
            let first = line(*rows.iter().min().unwrap());
            assert!(first.contains('!'), "{width} columns: {first}");
            assert!(
                rows.iter().any(|row| line(*row).contains(expected)),
                "{width} columns: {:?}",
                rows.iter().map(|row| line(*row)).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn attention_summary_counts_sessions_once_by_their_current_activity() {
    let fixture = Fixture::new();
    let mut rows: Vec<_> = (0..9)
        .map(|index| {
            session(
                &format!("session-{index}"),
                if index < 2 {
                    "needs-you"
                } else if index < 6 {
                    "working"
                } else {
                    "waiting"
                },
                "dotfiles",
                "/r/dotfiles",
                index + 1,
            )
        })
        .collect();
    rows[0]["ask_to_run"] = json!("approval");
    for row in &mut rows {
        row["since"] = json!(util::now_secs());
    }
    fixture.snapshot(json!(rows));
    let mut app = fixture.app();
    let rendered = buffer_rows(&render(&mut app, 120, 40)).join("\n");
    assert!(rendered.contains("2 need you"));
    assert!(rendered.contains("4 working"));
    assert!(rendered.contains("3 ready"));
    assert!(!rendered.contains("question"));
}

#[test]
fn fixed_next_needing_me_click_focuses_the_next_request() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "working", "dotfiles", "/r/dotfiles", 1),
        session(SESSION_B, "needs-you", "dotfiles", "/r/dotfiles", 2)
    ]));
    let mut app = fixture.app();
    render(&mut app, 100, 40);
    let (&row, &(start, _, _)) = app
        .hit_actions
        .iter()
        .find(|(_, (_, _, action))| *action == "next-attention")
        .expect("clickable attention shortcut");
    app.handle(
        Key::Click {
            x: start,
            y: row,
            double: true,
        },
        &mut FakeScreen::default(),
    );
    assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_B);
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        [format!("cockpit attend {SESSION_B}")]
    );
}

#[test]
fn routine_session_details_belong_only_to_the_selected_task() {
    let fixture = Fixture::new();
    let mut first = session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1);
    first["unpushed"] = json!(8);
    let mut second = session(SESSION_B, "working", "dotfiles", "/r/dotfiles", 2);
    second["unpushed"] = json!(3);
    fixture.snapshot(json!([first, second]));
    let mut app = fixture.app();
    // The working session lists first.
    let first_render = buffer_rows(&render(&mut app, 140, 40)).join("\n");
    assert!(first_render.contains("↑3 unpushed"));
    assert!(!first_render.contains("↑8 unpushed"));
    app.panels[1].selected = 1;
    let second_render = buffer_rows(&render(&mut app, 140, 40)).join("\n");
    assert!(second_render.contains("↑8 unpushed"));
    assert!(!second_render.contains("↑3 unpushed"));
    assert!(app.panels[1].lines.iter().all(|line| {
        line.right
            .iter()
            .all(|part| !part.text.contains("unpushed"))
    }));
}

#[test]
fn arrow_navigation_selects_a_session_without_opening_it() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "dotfiles", "/r/first", 1),
        session(SESSION_B, "waiting", "dotfiles", "/r/second", 2)
    ]));
    let mut app = fixture.app();
    let order: Vec<_> = app
        .sessions
        .iter()
        .map(|session| session.session.clone())
        .collect();
    app.handle(Key::Down, &mut FakeScreen::default());
    assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_B);
    assert!(fixture.calls("cockpit").is_empty());
    assert_eq!(
        app.sessions
            .iter()
            .map(|session| session.session.clone())
            .collect::<Vec<_>>(),
        order
    );
}

#[test]
fn retired_search_keeps_the_selection_and_list_order() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "dotfiles", "/r/first", 1),
        session(SESSION_B, "waiting", "dotfiles", "/r/second", 2)
    ]));
    let mut app = fixture.app();
    app.panels[1].selected = 1;
    app.handle(Key::Char('/'), &mut FakeScreen::default());
    assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_B);
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn selected_session_identity_survives_new_sessions_pins_and_state_changes() {
    let fixture = Fixture::new();
    let first = session(SESSION_A, "waiting", "dotfiles", "/r/first", 2);
    let second = session(SESSION_B, "working", "dotfiles", "/r/second", 3);
    fixture.snapshot(json!([first.clone(), second.clone()]));
    let mut app = fixture.app();
    app.panels[1].selected = app
        .sessions
        .iter()
        .position(|session| session.session == SESSION_B)
        .unwrap();
    let mut updated = second;
    updated["state"] = json!("needs-you");
    fixture.snapshot(json!([
        session(OPENCODE_SESSION, "working", "dotfiles", "/r/third", 1),
        first.clone(),
        updated.clone()
    ]));
    app.build();
    assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_B);
    app.view.toggle_pin(SESSION_A);
    app.redraw();
    assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_B);
    fixture.snapshot(json!([first]));
    app.build();
    let text = buffer_rows(&render(&mut app, 120, 40)).join("\n");
    assert_eq!(app.panels[1].selected, 0);
    assert!(!text.contains("An answer or permission is needed"));
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit view {SESSION_A}")]
    );
}

#[test]
fn unknown_and_empty_session_summaries_do_not_claim_readiness() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "unknown",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    let mut app = fixture.app();
    let unknown = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(
        unknown.contains("1 unknown") && unknown.contains("State unknown (no report for"),
        "{unknown}"
    );
    assert!(unknown.contains("0 ready") && unknown.contains("Nothing needs you"));
    fixture.snapshot(json!([]));
    app.build();
    let empty = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(empty.contains("No running session."));
    assert!(app.panels[1].fixed_bottom.is_empty());
    assert!(
        !app.hit_actions
            .values()
            .any(|(_, _, action)| *action == "next-attention")
    );
}

#[test]
fn session_summary_counts_each_session_in_exactly_one_bucket() {
    let fixture = Fixture::new();
    let mut needing_while_working = session(SESSION_A, "working", "dotfiles", "/r/a", 1);
    needing_while_working["needs_input"] = json!(true);
    let mut fresh_ready = session(
        "cccccccc-0000-4000-8000-000000000000",
        "waiting",
        "dotfiles",
        "/r/c",
        3,
    );
    fresh_ready["since"] = json!(util::now_secs());
    fixture.snapshot(json!([
        needing_while_working,
        session(SESSION_B, "working", "dotfiles", "/r/b", 2),
        fresh_ready,
        session(
            "dddddddd-0000-4000-8000-000000000000",
            "waiting",
            "dotfiles",
            "/r/d",
            4
        ),
        session(
            "eeeeeeee-0000-4000-8000-000000000000",
            "unknown",
            "dotfiles",
            "/r/e",
            5
        ),
    ]));
    let mut app = fixture.app();
    let rows = buffer_rows(&render(&mut app, 120, 40)).join("\n");
    assert!(rows.contains("! 1 need you"), "{rows}");
    assert!(
        rows.contains("1 working · 1 ready · 1 idle · 1 unknown"),
        "{rows}"
    );
}

#[test]
fn long_unicode_task_titles_stay_complete_in_selected_detail() {
    let fixture = Fixture::new();
    let title =
        "清理磁盘缓存 e\u{301} — Preserve all project data while removing unused download caches";
    opencode_database_with(
        &fixture,
        &[(
            OPENCODE_SESSION,
            "/r/dotfiles",
            title,
            None,
            2_000_000,
            None,
        )],
    );
    fixture.snapshot(json!([opencode_session("working", "dotfiles", 1)]));
    let mut app = fixture.app();
    for width in [40, 80, 140] {
        let rendered = buffer_rows(&render(&mut app, width, 40)).join("\n");
        assert!(!rendered.contains('\u{fffd}'));
        assert!(rendered.contains('清') && rendered.contains('盘'));
        assert_eq!(app.session_task_titles[OPENCODE_SESSION], title);
        assert!(app.panels[1].fixed_bottom[0].texts().contains(title));
    }
}

#[test]
fn keypress_redraw_uses_cached_session_branches() {
    let fixture = Fixture::new();
    fixture.write("repos/clone/.git/HEAD", "ref: refs/heads/initial-branch\n");
    let clone = fixture.path("repos/clone").display().to_string();
    fixture.snapshot(json!([session(
        SESSION_A, "working", "dotfiles", &clone, 1
    )]));
    let mut app = fixture.app();
    fixture.write("repos/clone/.git/HEAD", "ref: refs/heads/changed-branch\n");
    app.handle(Key::Down, &mut FakeScreen::default());
    app.redraw();
    assert_eq!(app.session_task_titles[SESSION_A], "initial-branch");
    app.build();
    assert_eq!(app.session_task_titles[SESSION_A], "changed-branch");
}

#[test]
fn request_detail_popup_reveals_the_complete_request_without_session_actions() {
    let fixture = Fixture::new();
    let request = format!(
        "Choose the final layout: {} FINAL REQUEST MARKER",
        (0..90)
            .map(|index| format!("requirement{index}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut row = session(SESSION_A, "needs-you", "dotfiles", "/r/dotfiles", 1);
    row["request"] = json!(request);
    fixture.snapshot(json!([
        row,
        session(SESSION_B, "working", "other", "/r/other", 2)
    ]));
    for width in [40, 80] {
        let mut app = fixture.app();
        app.handle(Key::Char('i'), &mut FakeScreen::default());
        let beginning = buffer_rows(&render(&mut app, width, 16)).join("\n");
        assert!(
            beginning.contains("WHERE & WHO"),
            "details did not open: {beginning}"
        );
        let mut seen = String::new();
        for _ in 0..160 {
            app.handle(Key::Down, &mut FakeScreen::default());
            seen += &buffer_rows(&render(&mut app, width, 16)).join("\n");
        }
        for word in ["FINAL", "REQUEST", "MARKER"] {
            assert!(seen.contains(word), "request tail is missing {word}");
        }
        assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_A);
        app.handle(Key::Enter, &mut FakeScreen::default());
        assert!(
            !buffer_rows(&render(&mut app, width, 16))
                .join("\n")
                .contains("WHERE & WHO")
        );
    }
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn request_detail_has_wrapped_space_instead_of_sharing_the_badge_row() {
    let fixture = Fixture::new();
    let request = "Should I prepare the design handoff and export the tokens without changing any product code?";
    let mut row = session(SESSION_A, "needs-you", "dotfiles", "/r/dotfiles", 1);
    row["request"] = json!(request);
    row["unpushed"] = json!(12);
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    render(&mut app, 60, 40);
    let request_lines: Vec<_> = app.panels[1]
        .fixed_bottom
        .iter()
        .filter(|line| line.texts().contains("Should I") || line.texts().contains("product code"))
        .collect();
    assert!(
        request_lines.len() >= 2,
        "request should wrap into a reserved area"
    );
    assert!(
        request_lines.iter().all(|line| line.right.is_empty()),
        "badges must not crowd request text"
    );
}

#[test]
fn request_detail_popup_ignores_underlying_clicks_and_retains_its_original_text() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "needs-you", "dotfiles", "/r/dotfiles", 1);
    row["request"] = json!("Which design should I use?");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    render(&mut app, 100, 40);
    let (&row, &(column, _, _)) = app
        .hit_actions
        .iter()
        .find(|(_, (_, _, action))| *action == "effort")
        .unwrap();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    render(&mut app, 100, 40);
    assert!(app.hit_actions.is_empty() && app.hit_lines.is_empty());
    app.handle(
        Key::Click {
            x: column,
            y: row,
            double: true,
        },
        &mut FakeScreen::default(),
    );
    assert!(app.menu.is_none());
    fixture.snapshot(json!([]));
    app.build();
    assert!(
        buffer_rows(&render(&mut app, 100, 40))
            .join("\n")
            .contains("Which design should I use?")
    );
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(app.sessions.is_empty());
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn request_detail_popup_wraps_long_unicode_tokens_and_stays_readable_when_short() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "needs-you", "dotfiles", "/r/dotfiles", 1);
    row["request"] = json!(format!("{} ENDOFREQUEST", "设计确认".repeat(30)));
    fixture.snapshot(json!([row]));
    for (width, height) in [(24, 12), (40, 16), (80, 24)] {
        let mut app = fixture.app();
        app.handle(Key::Char('i'), &mut FakeScreen::default());
        let mut seen = String::new();
        for _ in 0..100 {
            let rows = buffer_rows(&render(&mut app, width, height));
            seen += &rows.join("\n");
            app.handle(Key::Down, &mut FakeScreen::default());
        }
        assert!(seen.contains('设') && seen.contains('认') && seen.contains("ENDOFREQUEST"));
        assert!(!seen.contains('\u{fffd}'));
        app.handle(Key::PageUp, &mut FakeScreen::default());
        assert!(
            buffer_rows(&render(&mut app, width, height))
                .iter()
                .any(|line| line.contains("WHERE & WHO"))
        );
    }
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn request_detail_popup_uses_the_complete_capture_not_the_card_summary() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "needs-you", "dotfiles", "/r/dotfiles", 1);
    row["request"] = json!("Permission needed");
    row["detail"] = json!("Bash command · short preview");
    row["full_request"] = json!(format!(
        "Bash command\n{}\nCOMPLETE_REQUEST_END",
        "command argument ".repeat(40)
    ));
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    app.handle(Key::Char('G'), &mut FakeScreen::default());
    let rendered = buffer_rows(&render(&mut app, 80, 21)).join("\n");
    assert!(rendered.contains("COMPLETE_REQUEST_END"));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn grouped_opencode_cards_use_the_session_title_not_the_last_reply() {
    let fixture = Fixture::new();
    fixture.write("repos/clone/.git/HEAD", "ref: refs/heads/parallel-1249\n");
    let clone = fixture.path("repos/clone").display().to_string();
    opencode_database_with(
        &fixture,
        &[(
            OPENCODE_SESSION,
            &clone,
            "Improve invoice layout",
            None,
            2_000_000,
            None,
        )],
    );
    let mut row = opencode_session("waiting", "opusline", 1);
    row["cwd"] = json!(clone);
    row["task"] = json!("Proceed");
    fixture.snapshot(json!([
        row,
        session(SESSION_A, "working", "opusline", "/r/opusline", 2)
    ]));
    let app = fixture.app();
    let lines = texts(&app, 1);
    assert!(
        lines.contains(&"opusline › Improve invoice layout".to_string()),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("Proceed")),
        "{lines:?}"
    );
}

#[test]
fn h_resumes_the_past_session_picked() {
    let fixture = Fixture::new();
    fixture.write("home/.claude/history.jsonl", &format!("{}\n{}\n",
        json!({"sessionId": "past-1", "project": "/r/runway", "timestamp": 1_000_000, "display": "fix login"}),
        json!({"sessionId": "past-1", "project": "/r/runway", "timestamp": 2_000_000, "display": "/compact"})));
    fixture.shim("fzf", "head -n 1");
    let mut app = fixture.app();
    app.handle(Key::Char('H'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec!["cockpit resume /r/runway past-1".to_string()]
    );
}

#[test]
fn h_offers_opencode_sessions_with_claudes_newest_first_and_resumes_the_one_picked() {
    let fixture = Fixture::new();
    fixture.write("home/.claude/history.jsonl", &format!("{}\n", json!({"sessionId": "past-1", "project": "/r/runway", "timestamp": 1_000_000, "display": "fix login"})));
    opencode_database_with(
        &fixture,
        &[
            (
                OPENCODE_SESSION,
                "/r/opusline",
                "Prompt FigCraft",
                None,
                2_000_000,
                None,
            ),
            (
                "ses_3f2a9c0b2000AbCdEfGhIjKlMn",
                "/r/opusline",
                "a subagent's",
                Some(OPENCODE_SESSION),
                3_000_000,
                None,
            ),
            (
                "ses_3f2a9c0b3000AbCdEfGhIjKlMn",
                "/r/opusline",
                "archived",
                None,
                4_000_000,
                Some(5),
            ),
        ],
    );
    let mut app = fixture.app();
    let past = app.past_sessions();
    let listed: Vec<(&str, &str)> = past
        .iter()
        .map(|(entry, title)| (entry.session.as_str(), title.as_str()))
        .collect();
    assert_eq!(
        listed,
        [(OPENCODE_SESSION, "Prompt FigCraft"), ("past-1", "")]
    );
    fixture.shim("fzf", "head -n 1");
    app.handle(Key::Char('H'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit resume /r/opusline {OPENCODE_SESSION}")]
    );
}

#[test]
fn h_leaves_a_running_opencode_session_out() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([opencode_session("waiting", "opusline", 1)]));
    opencode_database_with(
        &fixture,
        &[(
            OPENCODE_SESSION,
            "/r/opusline",
            "Prompt FigCraft",
            None,
            2_000_000,
            None,
        )],
    );
    let mut app = fixture.app();
    assert!(app.past_sessions().is_empty());
}

#[test]
fn the_latest_real_prompt_wins_over_commands_and_pastes() {
    let fixture = Fixture::new();
    fixture.write("home/.claude/history.jsonl", &[
        json!({"sessionId": "s", "project": "/r", "timestamp": 1000, "display": "fix the  login"}),
        json!({"sessionId": "s", "project": "/r", "timestamp": 2000, "display": "/compact"}),
        json!({"sessionId": "s", "project": "/r", "timestamp": 3000, "display": "[Pasted text #1 +9 lines]"}),
    ].iter().map(|row| row.to_string() + "\n").collect::<String>());
    let mut history = sessions::History::default();
    history.refresh();
    assert_eq!(
        (history.entries[0].prompt.as_str(), history.entries[0].since),
        ("fix the login", 3)
    );
}

#[test]
fn a_title_written_later_is_picked_up() {
    let fixture = Fixture::new();
    let transcript = fixture.write(
        &format!("home/.claude/projects/r/{SESSION_A}.jsonl"),
        "{\"type\":\"user\"}\n",
    );
    let mut history = sessions::History::default();
    history.load_titles(&[SESSION_A.to_string()], 8);
    assert_eq!(history.title(SESSION_A), "");
    fs::write(
        &transcript,
        "{\"type\":\"ai-title\",\"aiTitle\":\"Fix login\"}\n",
    )
    .unwrap();
    assert_eq!(sessions::read_title(SESSION_A), "Fix login");
}

#[test]
fn git_branch_reads_head_through_worktrees_and_detached_heads() {
    let fixture = Fixture::new();
    fixture.write("main/.git/HEAD", "ref: refs/heads/main\n");
    fixture.write("worktrees/one/HEAD", "ref: refs/heads/feature\n");
    fixture.write(
        "tree/.git",
        &format!("gitdir: {}\n", fixture.path("worktrees/one").display()),
    );
    fixture.write("detached/.git/HEAD", "0123456789abcdef\n");
    assert_eq!(
        util::git_branch(&fixture.path("main").display().to_string()),
        "main"
    );
    assert_eq!(
        util::git_branch(&fixture.path("tree").display().to_string()),
        "feature"
    );
    assert_eq!(
        util::git_branch(&fixture.path("detached").display().to_string()),
        "0123456"
    );
}

#[test]
fn the_picker_lists_recently_used_projects_first() {
    let fixture = Fixture::new();
    for name in ["alpha", "beta", ".parallel"] {
        fs::create_dir_all(fixture.path(&format!("home/IdeaProjects/{name}"))).unwrap();
    }
    let beta = fixture.path("home/IdeaProjects/beta").display().to_string();
    fixture.write(
        "home/.claude/history.jsonl",
        &format!(
            "{}\n",
            json!({"project": format!("{beta}/sub"), "timestamp": 5})
        ),
    );
    let choices = sessions::project_choices();
    assert_eq!(
        choices,
        vec![
            beta,
            fixture
                .path("home/IdeaProjects/alpha")
                .display()
                .to_string()
        ]
    );
}

#[test]
fn a_design_session_shows_its_canvas_and_o_opens_it() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        1
    )]));
    fixture.write(
        &format!("home/.cache/claude/designs/{SESSION_A}.json"),
        &json!({"url": CANVAS, "title": "Checkout redesign"}).to_string(),
    );
    let mut app = fixture.app();
    assert!(selected_session_badges(&app).contains(&"◆ Checkout redesign".into()));
    assert!(app.sessions_hints().contains(&("o", "canvas")));
    app.handle(Key::Char('o'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("xdg-open", 1),
        vec![format!("xdg-open {CANVAS}")]
    );
}

#[test]
fn a_link_that_is_not_a_canvas_is_ignored_on_the_card() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        1
    )]));
    fixture.write(
        &format!("home/.cache/claude/designs/{SESSION_A}.json"),
        &json!({"url": "https://evil.example/x", "title": "Trap"}).to_string(),
    );
    let app = fixture.app();
    assert!(!texts(&app, 1).iter().any(|line| line.contains("Trap")));
}

#[test]
fn the_design_action_starts_a_design_session_on_the_canvas_in_the_chosen_project() {
    let fixture = Fixture::new();
    let project = fixture.root.join("home/IdeaProjects/opusline");
    fs::create_dir_all(&project).unwrap();
    let mut app = fixture.app();
    let chat_link = "https://claude.ai/chat/7efffff6-0466-41ab-83d1-3f1d403d357d?artifact=d31175a3-dc24-4166-aa81-2ccdf5efe83a";
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    for key in [Key::BackTab, Key::Right, Key::Tab, Key::Tab] {
        app.handle(key, &mut FakeScreen::default());
    }
    type_text(&mut app, chat_link);
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert_eq!(app.message, "starting a design session in opusline …");
    let log = {
        fixture.wait_for_calls("cockpit", 1);
        fs::read_to_string(fixture.path("calls.log")).unwrap()
    };
    assert!(
        log.contains(&format!(
            "cockpit new {} This is a design session for the Claude Design canvas {CANVAS}",
            project.display()
        )),
        "{log}"
    );
    assert!(
        log.contains(&format!("cockpit-design link {CANVAS}")) && log.contains("ArtifactComments")
    );
}

#[test]
fn the_design_action_refuses_a_link_that_is_not_a_canvas() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("home/IdeaProjects/opusline")).unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('N'), &mut FakeScreen::default());
    for key in [Key::BackTab, Key::Right, Key::Tab, Key::Tab] {
        app.handle(key, &mut FakeScreen::default());
    }
    type_text(&mut app, "https://example.com/x");
    app.handle(Key::Ctrl('s'), &mut FakeScreen::default());
    assert!(app.message.starts_with("not a canvas link"));
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn the_column_says_when_sessions_are_paused_for_a_game() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.write(
        "run/game-performance.frozen",
        "tmux-spawn-aaa.scope\ntmux-spawn-bbb.scope\n",
    );
    let app = fixture.app();
    assert!(
        texts(&app, 1)
            .iter()
            .any(|line| line.contains("2 sessions paused while you game")),
        "{:?}",
        texts(&app, 1)
    );
}

#[test]
fn no_pause_line_without_a_game() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    let app = fixture.app();
    assert!(
        !texts(&app, 1)
            .iter()
            .any(|line| line.contains("paused while you game"))
    );
}

#[test]
fn sessions_show_two_notices_then_point_to_status_which_lists_them_all() {
    let fixture = Fixture::new();
    fixture.write("run/game-performance.frozen", "tmux-spawn-aaa.scope\n");
    let mut app = fixture.app();
    app.disk = Some(("/home".into(), 92.0));
    app.disk_checked = util::now();
    app.restore = vec![sessions::Restorable {
        session: SESSION_B.into(),
        cwd: "/r/shop".into(),
        project: "shop".into(),
    }];
    app.restore_checked = util::now();
    app.build();
    assert_eq!(
        &texts(&app, 1)[..3],
        &[
            "‖1 session paused while you game · resume after",
            "▲Disk 92% full on /home",
            "+1 more · s for Status"
        ]
    );
    app.build();
    let status = texts(&app, 0);
    for expected in [
        "‖1 session paused while you game · resume after",
        "▲Disk 92% full on /home",
        "↺1 session ran before the restart",
    ] {
        assert!(
            status.iter().any(|line| line == expected),
            "Status lacks {expected}: {status:?}"
        );
    }
}

#[test]
fn notices_never_use_amber_which_means_needs_you() {
    let fixture = Fixture::new();
    fixture.write("run/game-performance.frozen", "tmux-spawn-aaa.scope\n");
    let mut app = fixture.app();
    app.disk = Some(("/".into(), 86.0));
    app.disk_checked = util::now();
    app.restore = vec![sessions::Restorable {
        session: SESSION_B.into(),
        cwd: "/r/shop".into(),
        project: "shop".into(),
    }];
    app.restore_checked = util::now();
    app.build();
    app.build();
    let notices: Vec<_> = app.panels[0]
        .lines
        .iter()
        .filter(|line| line.accent.is_some())
        .collect();
    assert!(notices.len() >= 3, "{}", notices.len());
    for line in notices {
        assert_ne!(
            line.accent.unwrap().fg,
            crate::app::style(crate::app::YELLOW).fg,
            "{}",
            line.texts()
        );
    }
}

#[test]
fn the_restore_notice_offers_b() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.restore = vec![sessions::Restorable {
        session: SESSION_B.into(),
        cwd: "/r/shop".into(),
        project: "shop".into(),
    }];
    app.restore_checked = util::now();
    app.build();
    assert_eq!(
        texts_with_badges(&app, 1)[0],
        "↺1 session ran before the restart B restore"
    );
}

#[test]
fn greyproxy_holds_become_a_needs_you_card_reviewed_with_g() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.held = vec![
        "claude-stray-coop → api.steampowered.com:443".into(),
        "claude-stray-coop → cdn.example:443".into(),
    ];
    app.held_checked = util::now();
    app.build();
    let lines = texts_with_badges(&app, 1);
    let heading = lines
        .iter()
        .position(|line| line.starts_with("NEEDS YOU · 1"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert!(
        lines[heading + 1].starts_with("!greyproxy held 2 requests"),
        "{lines:?}"
    );
    assert!(
        lines[heading + 2]
            .starts_with("claude-stray-coop › api.steampowered.com:443, cdn.example:443"),
        "{lines:?}"
    );
    assert!(
        lines[heading + 3].contains("Allow or deny in greyproxy")
            && lines[heading + 3].contains("G review"),
        "{lines:?}"
    );
    let summary = app.panels[1]
        .fixed_top
        .iter()
        .map(|line| line.texts())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(summary.contains("! 1 need you"), "{summary}");
}

#[test]
fn n_with_only_greyproxy_waiting_points_to_g() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.held = vec!["claude-shop → example.com:443".into()];
    app.held_checked = util::now();
    app.build();
    app.handle(Key::Char('n'), &mut FakeScreen::default());
    assert!(app.message.contains("G reviews"), "{}", app.message);
}

#[test]
fn a_card_shows_at_most_two_badges_most_urgent_first_before_its_age() {
    let fixture = Fixture::new();
    let mut row = session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        util::now_secs() - 600,
    );
    (row["unpushed"], row["update"], row["credits"]) = (json!(3), json!(true), json!(true));
    fixture.snapshot(json!([row]));
    let app = fixture.app();
    assert_eq!(first_card_parts(&app), ["↑3", "⬆", "10m"]);
}

#[test]
fn badges_give_way_before_the_age_in_a_narrow_column() {
    let fixture = Fixture::new();
    let mut row = session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        util::now_secs() - 600,
    );
    (row["unpushed"], row["update"]) = (json!(3), json!(true));
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    for (width, expected) in [(24, vec!["↑3", "10m"]), (20, vec!["10m"])] {
        app.width = width;
        app.redraw();
        assert_eq!(first_card_parts(&app), expected, "width {width}");
    }
}

#[test]
fn only_the_selected_card_adds_its_meta_line() {
    let fixture = Fixture::new();
    let mut first = session(SESSION_A, "waiting", "runway", "/r/runway", 1);
    (first["agents"], first["credits"]) = (json!(2), json!(true));
    let mut second = session(SESSION_B, "waiting", "shop", "/r/shop", 2);
    second["agents"] = json!(1);
    fixture.snapshot(json!([first, second]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app.redraw();
    let selected = app.panels[1].selected;
    let meta: Vec<_> = app.panels[1]
        .lines
        .iter()
        .filter(|line| line.texts().starts_with("Claude Code · "))
        .collect();
    assert_eq!(meta.len(), 1, "{:?}", texts(&app, 1));
    assert_eq!(meta[0].item, Some(selected));
    let selected_session = &app
        .sessions
        .iter()
        .find(|session| {
            matches!(&app.sidebar_rows[selected],
        crate::teams::sidebar::SidebarRow::Session { session_id } if *session_id == session.session)
        })
        .unwrap()
        .session;
    let expected = if selected_session == SESSION_A {
        "Claude Code · ◷ 2 agents · $ API"
    } else {
        "Claude Code · ◷ 1 agent · subscription"
    };
    assert_eq!(meta[0].texts(), expected);
}

#[test]
fn a_restarting_card_reads_restarting_in_working() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    note_restart(&fixture, SESSION_A, util::now() as i64, "runway", "/r");
    fixture.snapshot(json!([]));
    app.build();
    let lines = texts(&app, 1);
    let heading = lines
        .iter()
        .position(|line| line.starts_with("WORKING"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert_eq!(lines[heading + 1], "⟳Restarting…");
}

#[test]
fn an_unseen_result_reads_bright_in_quiet_until_opened() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([continuity_result_row(
        "11111111111111111111111111111111",
        "waiting"
    )]));
    let app = fixture.app();
    let lines = texts(&app, 1);
    let heading = lines
        .iter()
        .position(|line| line.starts_with("QUIET"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert_eq!(lines[heading + 1], "●Finished · new result");
    let card = app.panels[1]
        .lines
        .iter()
        .find(|line| line.item == Some(0))
        .unwrap();
    assert_eq!(
        card.segments[0].2.fg,
        crate::app::style(crate::app::GREEN).fg
    );
    assert_eq!(
        card.segments[1].2.fg,
        crate::app::style(crate::app::FG).fg,
        "bright, not the quiet grey"
    );
}

#[test]
fn an_unknown_session_says_how_long_it_has_not_reported() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "unknown",
        "runway",
        "/r",
        util::now_secs() - 600
    )]));
    let app = fixture.app();
    let card = app.panels[1]
        .lines
        .iter()
        .find(|line| line.item == Some(0))
        .unwrap();
    assert_eq!(card.texts(), "·State unknown (no report for 10m)");
}

#[test]
fn r_on_a_blocked_card_runs_the_todo_it_waits_on_after_a_yes() {
    let fixture = Fixture::new();
    let mut app = blocked_card(&fixture);
    app.handle(
        Key::Char('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    let call = fixture.wait_for_calls("cockpit", 1).remove(0);
    assert!(
        call.starts_with("cockpit --agent claude parallel /r/opusline todo-abcd1234 "),
        "{call}"
    );
    assert_eq!(app.todos.status_of("abcd1234"), "doing");
}

#[test]
fn r_on_a_blocked_card_does_nothing_without_a_yes() {
    let fixture = Fixture::new();
    let mut app = blocked_card(&fixture);
    app.handle(
        Key::Char('r'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
    assert_eq!(app.todos.status_of("abcd1234"), "todo");
}

#[test]
fn a_card_shows_its_unpushed_commits() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1);
    row["unpushed"] = json!(2);
    fixture.snapshot(json!([
        row,
        session(SESSION_B, "waiting", "runway", "/r/runway", 2)
    ]));
    let app = fixture.app();
    let badges = selected_session_badges(&app);
    assert_eq!(
        badges
            .iter()
            .filter(|badge| badge.starts_with('↑'))
            .collect::<Vec<_>>(),
        ["↑2 unpushed"]
    );
}

#[test]
fn a_card_shows_its_background_work_with_the_agents_counted() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "runway", "/r/runway", 1);
    row["agents"] = json!(3);
    row["background"] = json!(1);
    fixture.snapshot(json!([
        row,
        session(SESSION_B, "waiting", "dotfiles", "/r/dotfiles", 2)
    ]));
    let app = fixture.app();
    let badges = selected_session_badges(&app);
    assert_eq!(
        badges
            .iter()
            .filter(|badge| badge.starts_with('◷'))
            .collect::<Vec<_>>(),
        ["◷ 3 agents + background"]
    );
}

#[test]
fn ask_to_run_badges_stay_on_their_session_and_clear_when_finished() {
    let fixture = Fixture::new();
    for phase in ["approval", "running", "input", ""] {
        let mut row = session(SESSION_A, "working", "runway", "/r/runway", 1);
        row["ask_to_run"] = json!(phase);
        fixture.snapshot(json!([
            row,
            session(SESSION_B, "working", "runway", "/r/runway-clone", 2)
        ]));
        let mut app = fixture.app();
        for index in 0..app.sessions.len() {
            app.panels[1].selected = index;
            render(&mut app, 140, 45);
            let badges: Vec<_> = selected_session_badges(&app)
                .into_iter()
                .filter(|part| part.starts_with("ask-to-run:"))
                .collect();
            let expected = if app.sessions[index].session == SESSION_A && !phase.is_empty() {
                vec![format!("ask-to-run: {phase}")]
            } else {
                vec![]
            };
            assert_eq!(
                badges, expected,
                "session {} in phase {phase}",
                app.sessions[index].session
            );
        }
    }
}

#[test]
fn the_background_badge_names_what_runs() {
    let badge = |agents, background| {
        sessions::background_badge(&sessions::Session {
            agents,
            background,
            ..Default::default()
        })
    };
    assert_eq!(badge(0, 0), None);
    assert_eq!(badge(1, 0).as_deref(), Some("◷ 1 agent"));
    assert_eq!(badge(0, 2).as_deref(), Some("◷ background"));
}

#[test]
fn a_reviewed_clone_counts_unapplied_commits_and_u_applies_them() {
    for repository in crate::paths::REVIEWED_REPOSITORIES {
        let fixture = Fixture::new();
        let clone = fixture
            .root
            .join("home/IdeaProjects/.parallel")
            .join(repository)
            .join("p1")
            .display()
            .to_string();
        let mut row = session(SESSION_A, "waiting", repository, &clone, 1);
        row["unpushed"] = json!(3);
        fixture.snapshot(json!([row]));
        let app = fixture.app();
        let badges = selected_session_badges(&app);
        assert_eq!(
            badges
                .iter()
                .filter(|badge| badge.starts_with('↑'))
                .collect::<Vec<_>>(),
            ["↑3 unapplied"],
            "{repository}"
        );
        assert!(
            app.sessions_hints().contains(&("U", "apply")),
            "{repository}"
        );
    }
}

#[test]
fn a_session_left_behind_by_a_change_puts_restart_first() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        100
    )]));
    let mut app = fixture.app();
    app.start_files = vec![("CLAUDE.md", 500.0)];
    assert_eq!(
        app.sessions_hints()[0],
        ("R", "restart to pick up the change")
    );
}

#[test]
fn every_need_of_a_session_shows_before_the_everyday_keys() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1);
    row["unpushed"] = json!(2);
    row["update"] = json!(true);
    fixture.snapshot(json!([row]));
    fixture.write(
        &format!("home/.cache/claude/statusline-{SESSION_A}.json"),
        r#"{"context": 93}"#,
    );
    let app = fixture.app();
    assert_eq!(first_hint_keys(&app, 4), ["R", "C", "U", "⏎"]);
    assert_eq!(
        app.sessions_hints()
            .iter()
            .filter(|(key, _)| *key == "R")
            .count(),
        1
    );
}

#[test]
fn a_quiet_session_starts_with_open() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    assert_eq!(first_hint_keys(&fixture.app(), 1), ["⏎"]);
}

#[test]
fn only_the_sessions_of_the_project_figcraft_serves_show_it() {
    let fixture = Fixture::new();
    let open = CraftProcess::new("/r/opusline");
    fixture.shim("pgrep", &format!("echo {}", open.id()));
    let mut opusline = session(SESSION_A, "waiting", "opusline", "/r/opusline", 1);
    opusline["apps"] = json!("figcraft:on");
    let mut dotfiles = session(SESSION_B, "waiting", "dotfiles", "/r/dotfiles", 2);
    dotfiles["apps"] = json!("figcraft:on");
    fixture.snapshot(json!([opusline, dotfiles]));
    let mut app = fixture.app();
    assert_eq!(
        craft_badges(&mut app),
        ["◆ figcraft"],
        "shown on dotfiles, which FigCraft does not serve"
    );
}

#[test]
fn independent_figcraft_instances_show_on_both_served_checkouts_only() {
    let fixture = Fixture::new();
    let first = CraftProcess::new("/r/opusline");
    let second = CraftProcess::new("/r/a project");
    fixture.shim(
        "pgrep",
        &format!("printf '%s\\n' {} {}", first.id(), second.id()),
    );
    let mut rows = vec![
        session(SESSION_A, "waiting", "opusline", "/r/opusline", 1),
        session(SESSION_B, "waiting", "project", "/r/a project", 2),
        session(OPENCODE_SESSION, "waiting", "other", "/r/other", 3),
    ];
    for row in &mut rows {
        row["apps"] = json!("figcraft:on");
    }
    fixture.snapshot(json!(rows));
    assert_eq!(
        craft_badges(&mut fixture.app()),
        ["◆ figcraft", "◆ figcraft"]
    );
}

#[test]
fn a_craft_app_that_does_not_answer_shows_no_badge() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "opusline", "/r/opusline", 1);
    row["apps"] = json!("figcraft:off");
    fixture.snapshot(json!([row]));
    assert!(craft_badges(&mut fixture.app()).is_empty());
}

#[test]
fn the_folder_figcraft_serves_comes_from_its_command_line() {
    let fixture = Fixture::new();
    let open = CraftProcess::new("/r/a project");
    fixture.shim("pgrep", &format!("echo {}", open.id()));
    assert_eq!(
        sessions::craft_roots("figcraft"),
        Some(["/r/a project".to_string()].into())
    );
}

#[test]
fn a_reviewed_clones_card_shows_what_its_running_copy_has_not_pushed_and_u_pushes_it() {
    // U on a dotfiles card pushes ~/dotfiles after the apply: its count belongs on the card, and
    // likewise for the other running copies.
    for repository in crate::paths::REVIEWED_REPOSITORIES {
        let fixture = Fixture::new();
        let live = crate::paths::running_copy(repository);
        let git = |arguments: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&live)
                    .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                    .args(arguments)
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "git {arguments:?}"
            );
        };
        fs::create_dir_all(&live).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "on GitHub"]);
        git(&["remote", "add", "origin", "git@github.com:o/dotfiles.git"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&["branch", "-q", "--set-upstream-to=origin/main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "applied one"]);
        git(&["commit", "-q", "--allow-empty", "-m", "applied two"]);
        let work = crate::paths::work_clone(repository).display().to_string();
        fixture.snapshot(json!([session(
            SESSION_A, "waiting", repository, &work, 100
        )]));
        let app = fixture.app();
        let badges = selected_session_badges(&app);
        assert!(
            badges.contains(&"↑2 unpushed".to_string()),
            "{repository}: {badges:?}"
        );
        assert!(
            app.sessions_hints().contains(&("U", "push")),
            "{repository}: {:?}",
            app.sessions_hints()
        );
    }
}

#[test]
fn an_opencode_session_wears_a_badge() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        opencode_session("waiting", "runway", 1),
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    let app = fixture.app();
    assert!(selected_session_badges(&app).contains(&"◇ opencode".into()));
    assert_eq!(app.sessions[app.panels[1].selected].project, "runway");
}

#[test]
fn an_unreported_opencode_tmux_session_is_an_opencode_card() {
    // opencode reports at its first prompt: until then its tmux session stands for it.
    let fixture = Fixture::new();
    fixture.shim(
        "tmux",
        "printf 'claude-retry-opencode-7\\t%%5\\t/r/opusline\\topencode\\t100\\n'",
    );
    let app = fixture.app();
    assert_eq!(
        app.sessions
            .iter()
            .map(|s| (s.state.as_str(), s.agent.as_str()))
            .collect::<Vec<_>>(),
        vec![("unknown", "opencode")]
    );
}

#[test]
fn a_restarting_opencode_card_keeps_its_title_before_its_first_report() {
    let fixture = Fixture::new();
    fixture.write("repos/clone/.git/HEAD", "ref: refs/heads/parallel-1249\n");
    let clone = fixture.path("repos/clone").display().to_string();
    opencode_database_with(
        &fixture,
        &[(
            OPENCODE_SESSION,
            &clone,
            "Improve invoice layout",
            None,
            2_000_000,
            None,
        )],
    );
    fixture.snapshot(json!([session(
        SESSION_A, "waiting", "clone", "/r/clone", 1
    )]));
    fixture.shim("tmux", &format!("printf 'claude-retry-opencode-7\\t%%5\\t{clone}\\tarchlinux\\t100\\t{OPENCODE_SESSION}\\n'"));
    let app = fixture.app();
    let lines = texts(&app, 1);
    assert!(
        lines.contains(&"clone › Improve invoice layout".to_string()),
        "{lines:?}"
    );
}

#[test]
fn a_restarting_opencode_card_replaces_its_old_pane_without_a_duplicate() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([opencode_session("waiting", "runway", 1)]));
    fixture.shim("tmux", &format!("printf 'claude-retry-opencode-7\\t%%99\\t/r/runway\\tarchlinux\\t100\\t{OPENCODE_SESSION}\\n'"));
    let app = fixture.app();
    assert_eq!(
        app.sessions
            .iter()
            .map(|session| (
                session.session.as_str(),
                session.tmux_pane.as_str(),
                session.state.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![("claude-retry-opencode-7", "%99", "unknown")]
    );
}

#[test]
fn an_unreported_pane_rejects_a_session_identity_from_another_agent() {
    let fixture = Fixture::new();
    fixture.shim(
        "tmux",
        &format!(
            "printf 'claude-retry-opencode-7\\t%%5\\t/r/runway\\topencode\\t100\\t{SESSION_A}\\n'"
        ),
    );
    let app = fixture.app();
    assert_eq!(app.sessions[0].session, "claude-retry-opencode-7");
}

#[test]
fn a_second_live_pane_for_the_same_conversation_does_not_hide_the_reported_one() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([opencode_session("waiting", "runway", 1)]));
    fixture.shim("tmux", &format!("printf 'claude-retry-opencode-6\\t%%1\\t/r/runway\\topencode\\t90\\t{OPENCODE_SESSION}\\nclaude-retry-opencode-7\\t%%99\\t/r/runway\\tarchlinux\\t100\\t{OPENCODE_SESSION}\\n'"));
    let app = fixture.app();
    assert_eq!(
        app.sessions
            .iter()
            .map(|session| session.tmux_pane.as_str())
            .collect::<Vec<_>>(),
        vec!["%1", "%99"]
    );
}

#[test]
fn an_unreported_pane_does_not_hide_an_external_session_of_the_same_conversation() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([external_session(
        OPENCODE_SESSION,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.shim("tmux", &format!("printf 'claude-retry-opencode-7\\t%%99\\t/r/runway\\tarchlinux\\t100\\t{OPENCODE_SESSION}\\n'"));
    let running = sessions::running();
    assert_eq!(
        running
            .iter()
            .map(|session| session.tmux_pane.as_str())
            .collect::<Vec<_>>(),
        vec!["", "%99"]
    );
}

#[test]
fn what_only_claude_code_does_is_not_offered_on_an_opencode_card() {
    for key in ['$', 'C'] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([opencode_session("needs-you", "runway", 1)]));
        let mut app = fixture.app();
        app.handle(
            Key::Char(key),
            &mut FakeScreen::answering(&[Key::Char('y')]),
        );
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            fixture.calls("cockpit").is_empty(),
            "{key}: {:?}",
            fixture.calls("cockpit")
        );
        assert!(
            app.message.contains("runs opencode"),
            "{key}: {}",
            app.message
        );
        assert!(
            app.sessions_hints().iter().all(|(hint, _)| *hint != "C"),
            "{key}: {:?}",
            app.sessions_hints()
        );
    }
}

#[test]
fn a_message_can_be_queued_for_an_opencode_session() {
    let _fixture = Fixture::new();
    crate::prompts::add(OPENCODE_SESSION, "and the README").unwrap();
    assert_eq!(crate::prompts::count(OPENCODE_SESSION), 1);
    assert!(crate::prompts::add("ses_../../x", "no").is_err());
}

#[test]
fn m_refuses_remote_control_for_an_opencode_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([opencode_session("waiting", "runway", 1)]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(Key::Char('m'), &mut FakeScreen::default());
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        fixture.calls("cockpit").is_empty(),
        "{:?}",
        fixture.calls("cockpit")
    );
    assert!(app.message.contains("runs opencode"), "{}", app.message);
}

/// The column's tmux server can start before Hyprland exports its signature (login autostart):
/// the workspace checks behind the chief's focus and the queue's viewed-session guard must still
/// find the running Hyprland instead of reporting the cockpit workspace as inactive.
#[test]
fn workspace_checks_find_the_running_hyprland_without_an_inherited_signature() {
    let fixture = Fixture::new();
    let instance = fixture.root.join("run/hypr/running_instance");
    fs::create_dir_all(&instance).unwrap();
    fs::write(instance.join(".socket.sock"), "").unwrap();
    fixture.shim(
        "hyprctl",
        r#"[ "$HYPRLAND_INSTANCE_SIGNATURE" = running_instance ] || exit 1
case "$*" in *activeworkspace*) echo '{"id": 6}' ;; submap) echo gamemode ;; esac"#,
    );
    assert!(util::claude_workspace_active());
    // SAFETY: the fixture holds the environment lock.
    unsafe { std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE") };
    assert!(util::gaming());
}

#[test]
fn the_column_publishes_chief_focus_without_an_inherited_hyprland_signature() {
    let fixture = Fixture::new();
    let instance = fixture.root.join("run/hypr/running_instance");
    fs::create_dir_all(&instance).unwrap();
    fs::write(instance.join(".socket.sock"), "").unwrap();
    fixture.shim(
        "hyprctl",
        r#"[ "$HYPRLAND_INSTANCE_SIGNATURE" = running_instance ] || exit 1
case "$*" in *activeworkspace*) echo '{"id": 6}' ;; esac"#,
    );
    let requests = team_host(&fixture, team_snapshot(), vec![]);
    let mut app = fixture.app();
    app.check_team();
    let focus: Vec<Value> = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["request"] == "chief_focus")
        .cloned()
        .collect();
    assert_eq!(
        focus.len(),
        1,
        "the chief never learns Theo's focus: {:?}",
        requests.lock().unwrap()
    );
    assert_eq!(focus[0]["workspace_active"], true);
}

#[test]
fn a_card_shows_how_many_answers_the_chief_gave() {
    use ratatui::style::Modifier;
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(
        &fixture,
        vec![(SESSION_A, 2, vec![chief_answer(3, "Which?", "A.")])],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    let card = card_of(&app, SESSION_A);
    let count = card
        .right
        .iter()
        .find(|part| part.text == "Chief ✓2")
        .unwrap_or_else(|| {
            panic!(
                "no Chief ✓2 in {:?}",
                card.right.iter().map(|part| &part.text).collect::<Vec<_>>()
            )
        });
    assert!(
        count.style.add_modifier.contains(Modifier::DIM),
        "the count is not dim"
    );
    assert_eq!(count.style.fg, crate::app::style(crate::app::FG_DARK).fg);
    assert_ne!(count.style.fg, crate::app::style(crate::app::YELLOW).fg);
    assert_eq!(
        card.right[0].text, "Chief ✓2",
        "the count comes before the activity and age"
    );
    assert_eq!(card.right.len(), 2);
    app.panels[1].selected = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_A)).unwrap();
    render(&mut app, 100, 40);
    let badges = app.panels[1]
        .fixed_bottom
        .iter()
        .flat_map(|line| line.right.iter())
        .find(|part| part.text == "Chief ✓2")
        .expect("badge in the selected card");
    assert!(badges.style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn a_card_without_chief_answers_shows_no_count() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1),
        session(SESSION_B, "waiting", "dotfiles", "/r/dotfiles", 2)
    ]));
    chief_answers_host(
        &fixture,
        vec![
            (SESSION_B, 1, vec![chief_answer(1, "Which?", "A.")]),
            (SESSION_A, 0, vec![]),
        ],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    app.panels[1].selected = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_B)).unwrap();
    let rendered = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert_eq!(
        rendered.matches("Chief ✓").count(),
        2,
        "one card line and one selected badge for the single session with answers: {rendered}"
    );
    assert!(
        card_of(&app, SESSION_A)
            .right
            .iter()
            .all(|part| !part.text.contains("Chief"))
    );
    assert!(!texts(&app, 1).iter().any(|text| text.contains("✓0")));
}

#[test]
fn a_card_without_room_on_line_one_puts_the_count_in_line_two() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "needs-you",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(
        &fixture,
        vec![(SESSION_A, 3, vec![chief_answer(1, "Which?", "A.")])],
    );
    let mut app = fixture.app();
    app.check_team();
    app.width = 26;
    app.build();
    let index = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_A)).unwrap();
    let lines: Vec<_> = app.panels[1]
        .lines
        .iter()
        .filter(|line| line.item == Some(index))
        .collect();
    assert!(
        lines[0]
            .right
            .iter()
            .all(|part| !part.text.contains("Chief"))
    );
    assert!(
        lines[1].texts().starts_with("Chief ✓3 · "),
        "{}",
        lines[1].texts()
    );
}

#[test]
fn a_gone_team_host_clears_the_chief_counts() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(
        &fixture,
        vec![(SESSION_A, 2, vec![chief_answer(1, "Which?", "A.")])],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    assert!(
        card_of(&app, SESSION_A)
            .right
            .iter()
            .any(|part| part.text == "Chief ✓2")
    );
    fs::remove_file(crate::paths::team_host_socket()).unwrap();
    app.team_checked = 0.0;
    app.check_team();
    app.build();
    assert!(
        card_of(&app, SESSION_A)
            .right
            .iter()
            .all(|part| !part.text.contains("Chief"))
    );
}

#[test]
fn session_details_show_why_where_whats_around_my_note_then_the_keys_by_group() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "working", "runway", "/r/runway", 1);
    row["unpushed"] = json!(3);
    fixture.snapshot(json!([row]));
    crate::prompts::add(SESSION_A, "queued thought").unwrap();
    let mut app = fixture.app();
    app.build();
    let details = open_details(&mut app);
    let order: Vec<usize> = [
        "runway",
        "WHERE & WHO",
        "↑3 unpushed",
        "Billing: subscription",
        "AROUND IT",
        "✉ 1 queued message",
        "MY NOTE (never sent to the agent)",
        "none · w writes one",
        "WORK",
        "drop queued",
        "CODE",
        "lazygit",
        "SESSION",
        "remote control",
        "esc back · the same keys work on the selected card",
    ]
    .iter()
    .map(|needle| {
        details
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} missing:\n{details}"))
    })
    .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{details}");
    assert!(
        !details.contains("Chief answers"),
        "no answers, no c:\n{details}"
    );
}

#[test]
fn a_key_in_the_session_details_closes_them_and_acts_on_the_session() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.build();
    open_details(&mut app);
    app.handle(Key::Char('w'), &mut FakeScreen::typing(&["after lunch"]));
    assert_eq!(app.message, "note saved");
    assert!(
        !buffer_rows(&render(&mut app, 100, 50))
            .join("\n")
            .contains("WHERE & WHO")
    );
    assert!(open_details(&mut app).contains("after lunch"));
}

#[test]
fn session_details_list_the_recent_chief_answers() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(
        &fixture,
        vec![(
            SESSION_A,
            4,
            vec![
                chief_answer(1, "Which database?", "Use SQLite."),
                chief_answer(5, "Which port?", "Use 8080."),
                chief_answer(9, "Which name?", "Call it ledger."),
            ],
        )],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    let details = open_details(&mut app);
    for expected in [
        "Chief answers",
        "Which database?",
        "Chief: Use SQLite.",
        "Which port?",
        "Chief: Use 8080.",
        "Which name?",
        "Chief: Call it ledger.",
        "c all answers",
    ] {
        assert!(details.contains(expected), "{expected} missing:\n{details}");
    }
    let newest = details.find("Which database?").unwrap();
    assert!(
        newest < details.find("Which port?").unwrap()
            && details.find("Which port?").unwrap() < details.find("Which name?").unwrap()
    );
}

#[test]
fn session_details_without_chief_answers_have_no_chief_block() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(&fixture, vec![]);
    let mut app = fixture.app();
    app.check_team();
    app.build();
    let details = open_details(&mut app);
    assert!(
        !details.contains("Chief answers") && !details.contains("c all answers"),
        "{details}"
    );
}

#[test]
fn c_opens_the_answers_of_the_selected_session() {
    const SESSION_C: &str = "cccccccc-0000-4000-8000-00000000beef";
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1),
        session(SESSION_C, "waiting", "dotfiles", "/r/dotfiles", 2)
    ]));
    chief_answers_host(
        &fixture,
        vec![(SESSION_C, 1, vec![chief_answer(1, "Which?", "A.")])],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    app.panels[1].selected = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_C)).unwrap();
    app.handle(Key::Char('c'), &mut FakeScreen::default());
    let rendered = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(
        rendered.contains("Answers · ses_…00beef · Chief ✓1 this task"),
        "the caption states the card's rule: {rendered}"
    );
}

#[test]
fn c_on_a_session_without_answers_opens_an_empty_list() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(&fixture, vec![]);
    let mut app = fixture.app();
    app.check_team();
    app.build();
    app.handle(Key::Char('c'), &mut FakeScreen::default());
    let rendered = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(
        rendered.contains("Answers ·") && rendered.contains("No Chief answers yet."),
        "{rendered}"
    );
}

#[test]
fn a_long_answer_stays_on_one_line_with_an_ellipsis() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    let long = format!("ab\u{202e}{}", "x".repeat(2000));
    chief_answers_host(
        &fixture,
        vec![(SESSION_A, 1, vec![chief_answer(1, "Which?", &long)])],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    let details = open_details(&mut app);
    assert!(
        !details.contains('\u{202e}'),
        "the bidi character reached the screen"
    );
    let rows: Vec<&str> = details.lines().filter(|row| row.contains("xxxx")).collect();
    assert_eq!(rows.len(), 1, "the answer wrapped:\n{details}");
    assert!(
        rows[0].contains("Chief: ab\\u202e") && rows[0].trim_end_matches([' ', '│']).ends_with('…'),
        "{}",
        rows[0]
    );
}

#[test]
fn session_details_show_at_most_three_chief_answers() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(
        &fixture,
        vec![(
            SESSION_A,
            4,
            vec![
                chief_answer(1, "Question one?", "A."),
                chief_answer(2, "Question two?", "B."),
                chief_answer(3, "Question three?", "C."),
                chief_answer(4, "Question four?", "D."),
            ],
        )],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    let details = open_details(&mut app);
    for shown in ["Question one?", "Question two?", "Question three?"] {
        assert!(details.contains(shown), "{shown} missing:\n{details}");
    }
    assert!(
        !details.contains("Question four?"),
        "a fourth answer showed:\n{details}"
    );
}

#[test]
fn c_in_the_session_details_opens_the_answers_of_that_session() {
    const SESSION_C: &str = "cccccccc-0000-4000-8000-00000000beef";
    let fixture = Fixture::new();
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "dotfiles", "/r/dotfiles", 1),
        session(SESSION_C, "waiting", "dotfiles", "/r/dotfiles", 2)
    ]));
    chief_answers_host(
        &fixture,
        vec![(SESSION_C, 1, vec![chief_answer(1, "Which?", "A.")])],
    );
    let mut app = fixture.app();
    app.check_team();
    app.build();
    app.panels[1].selected = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == SESSION_C)).unwrap();
    let details = open_details(&mut app);
    assert!(details.contains("c all answers"));
    app.handle(Key::Char('c'), &mut FakeScreen::default());
    let rendered = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(
        rendered.contains("Answers · ses_…00beef") && !rendered.contains("Session details"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("✓0"),
        "no count on a session without answers: {rendered}"
    );
}

#[test]
fn c_in_the_session_details_without_chief_answers_does_nothing() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "dotfiles",
        "/r/dotfiles",
        1
    )]));
    chief_answers_host(&fixture, vec![]);
    let mut app = fixture.app();
    app.check_team();
    app.build();
    open_details(&mut app);
    app.handle(Key::Char('c'), &mut FakeScreen::default());
    let rendered = buffer_rows(&render(&mut app, 100, 40)).join("\n");
    assert!(
        rendered.contains("WHERE & WHO") && !rendered.contains("Answers ·"),
        "{rendered}"
    );
}

#[test]
fn capital_i_opens_images_from_the_latest_reply_in_one_floating_viewer_on_workspace_6() {
    let fixture = Fixture::new();
    let folder = session_with_latest_reply(
        &fixture,
        "Here:\n![Inbox](previews/a.png)\n![More](previews/b.png)",
    );
    fs::write(folder.join("previews/old.png"), PNG_BYTES).unwrap();
    fs::write(folder.join("previews/a.png"), PNG_BYTES).unwrap();
    fs::write(folder.join("previews/b.png"), b"\xff\xd8\xffsecond").unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('I'), &mut FakeScreen::default());
    let calls = viewer_calls(&fixture);
    assert_eq!(calls.len(), 1, "{calls:?} {}", app.message);
    let first = regex::Regex::new(
        r#"hl\.exec_cmd\("loupe (/[^"]+/01\.png)", \{ workspace = "6", float = true \}\)"#,
    )
    .unwrap()
    .captures(&calls[0])
    .unwrap_or_else(|| panic!("{}", calls[0]))[1]
        .to_string();
    let copies = Path::new(&first).parent().unwrap();
    assert!(copies.starts_with(fixture.path("run")), "{first}");
    assert_eq!(
        fs::metadata(copies).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let mut names: Vec<String> = fs::read_dir(copies)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["01.png", "02.jpg"]);
    assert_eq!(fs::read(copies.join("01.png")).unwrap(), PNG_BYTES);
    assert_eq!(app.message, "opening 2 images from work");
}

#[test]
fn capital_i_says_so_when_the_latest_reply_shows_no_image() {
    let fixture = Fixture::new();
    session_with_latest_reply(&fixture, "Nothing to see, only `![code](x.png)` inline");
    let mut app = fixture.app();
    app.handle(Key::Char('I'), &mut FakeScreen::default());
    assert_eq!(app.message, "No images in its latest reply.");
    assert!(viewer_calls(&fixture).is_empty());
}

#[test]
fn capital_i_refuses_images_outside_the_session_folder_symlinked_out_or_not_raster() {
    let fixture = Fixture::new();
    let folder = session_with_latest_reply(
        &fixture,
        "![Escape](../secret.png)\n![Link](previews/link.png)\n![Fake](previews/fake.png)",
    );
    fs::write(fixture.path("secret.png"), PNG_BYTES).unwrap();
    std::os::unix::fs::symlink(fixture.path("secret.png"), folder.join("previews/link.png"))
        .unwrap();
    fs::write(folder.join("previews/fake.png"), "<svg/>").unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('I'), &mut FakeScreen::default());
    assert!(
        viewer_calls(&fixture).is_empty(),
        "{:?}",
        viewer_calls(&fixture)
    );
    assert_eq!(
        app.message,
        "../secret.png is outside the session's folder; previews/link.png is outside the session's folder; \
        previews/fake.png is not a PNG, JPEG, GIF or WebP image"
    );
}

#[test]
fn capital_i_opens_what_it_may_and_names_what_it_refused() {
    let fixture = Fixture::new();
    let folder =
        session_with_latest_reply(&fixture, "![Escape](../secret.png)\n![Ok](previews/a.png)");
    fs::write(fixture.path("secret.png"), PNG_BYTES).unwrap();
    fs::write(folder.join("previews/a.png"), PNG_BYTES).unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('I'), &mut FakeScreen::default());
    assert_eq!(viewer_calls(&fixture).len(), 1);
    assert_eq!(
        app.message,
        "opening 1 image from work; ../secret.png is outside the session's folder"
    );
}

#[test]
fn capital_i_in_session_details_is_listed_and_opens_the_images() {
    let fixture = Fixture::new();
    let folder = session_with_latest_reply(&fixture, "![Inbox](previews/a.png)");
    fs::write(folder.join("previews/a.png"), PNG_BYTES).unwrap();
    let mut app = fixture.app();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    let rendered = buffer_rows(&render(&mut app, 100, 50)).join("\n");
    assert!(rendered.contains("I  images"), "{rendered}");
    app.handle(Key::Char('I'), &mut FakeScreen::default());
    assert_eq!(viewer_calls(&fixture).len(), 1, "{}", app.message);
}

#[test]
fn session_details_have_one_background_behind_their_text() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "project-a",
        "/r/project-a",
        1
    )]));
    let mut app = fixture.app();
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    let buffer = render(&mut app, 80, 30);
    assert!(
        buffer_rows(&buffer).iter().any(|row| row.contains('┌')),
        "details are open"
    );
    assert_one_background(&buffer, 2..28, 2..78, &[DESIGNED_HIGHLIGHT]);
}

#[test]
fn lowercase_r_no_longer_restarts_a_session_only_capital_r_does() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        1
    )]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app.handle(
        Key::Char('r'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        !fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains("restart")),
        "{:?}",
        fixture.calls("cockpit")
    );
    assert!(app.message.contains("R restarts"), "{}", app.message);
    app.handle(
        Key::Char('R'),
        &mut FakeScreen::answering(&[Key::Char('y')]),
    );
    assert!(
        fixture
            .wait_for_calls("cockpit", 1)
            .iter()
            .any(|call| call.contains("restart"))
    );
}

#[test]
fn the_sessions_footer_offers_no_find_and_capital_r_for_restarts() {
    let fixture = Fixture::new();
    let mut updated = session(SESSION_A, "waiting", "opusline", "/r/opusline", 1);
    updated["update"] = json!(true);
    fixture.snapshot(json!([updated]));
    let mut app = fixture.app();
    app.focus_panel(1);
    let hints = app.sessions_hints();
    assert!(
        hints.iter().all(|(key, _)| *key != "/" && *key != "r"),
        "{hints:?}"
    );
    assert!(
        hints.contains(&("R", "restart to apply the update")),
        "{hints:?}"
    );
}

#[test]
fn session_details_name_the_plan_and_what_the_session_cost_today_at_api_prices() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = details_with_costs(
        &fixture,
        &[(SESSION_A, 3.0), (SESSION_A, 1.2), ("someone-else", 9.0)],
    );
    app.plan = "Max".into();
    let details = open_details(&mut app);
    assert!(
        details.contains("Billing: Claude Max (subscription) · ≈ $4.20 at API prices today"),
        "{details}"
    );
}

#[test]
fn session_details_leave_out_a_cost_they_do_not_know() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = details_with_costs(&fixture, &[("someone-else", 9.0)]);
    let details = open_details(&mut app);
    let billing = details
        .lines()
        .find(|line| line.contains("Billing:"))
        .expect("a billing line");
    assert!(
        billing.contains("Billing: subscription") && !billing.contains('$'),
        "{billing}"
    );
}

#[test]
fn session_details_say_why_with_the_project_and_the_todo_the_session_works_on() {
    let fixture = Fixture::new();
    fixture.write("state/cockpit/todo.json", &json!({"items": [
        {"id": "b77a0881", "project": "runway", "cwd": "/r/runway", "text": "Run agents without you", "status": "doing", "session": SESSION_A}
    ]}).to_string());
    let mut row = session(SESSION_A, "working", "runway", "/r/runway", 1);
    row["label"] = json!("Running the Rust tests");
    fixture.snapshot(json!([row]));
    let mut app = fixture.app();
    app.build();
    render(&mut app, 100, 50);
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    let buffer = render(&mut app, 100, 50);
    let rows = buffer_rows(&buffer);
    let why = rows
        .iter()
        .position(|row| row.contains("runway ›"))
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(
        rows[why].contains("runway › Running the Rust tests · todo b77a0881"),
        "{}",
        rows[why]
    );
    let column = rows[why]
        .find("runway")
        .map(|byte| rows[why][..byte].chars().count())
        .unwrap() as u16;
    assert_eq!(
        buffer[(column, why as u16)].fg,
        crate::app::color(crate::app::BLUE),
        "the project stands out"
    );
    assert_eq!(
        buffer[(column + 9, why as u16)].fg,
        crate::app::color(crate::app::FG_DARK),
        "the task reads quieter"
    );
}
