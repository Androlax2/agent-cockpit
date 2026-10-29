use super::*;

#[test]
fn the_effort_is_a_clickable_stamp_that_opens_the_menu() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    fixture.write(
        &format!("home/.cache/claude/statusline-{SESSION_A}.json"),
        r#"{"model": "Opus 5.5", "effort": "high", "context": 20}"#,
    );
    let mut app = fixture.app();
    render(&mut app, 60, 40);
    let (&row, &(start, _, _)) = app
        .hit_actions
        .iter()
        .find(|(_, (_, _, action))| *action == "effort")
        .expect("a clickable effort");
    app.handle(
        Key::Click {
            x: start,
            y: row,
            double: false,
        },
        &mut FakeScreen::default(),
    );
    assert_eq!(
        app.menu.as_ref().map(|menu| menu.current.as_str()),
        Some("high")
    );
}

#[test]
fn enter_in_the_settings_menu_flips_the_setting_through_cockpit() {
    for (stored, wanted) in [
        (None, "off"),
        (Some(r#"{"phone-on-lock": false}"#), "on"),
        (Some("not json"), "off"),
    ] {
        let fixture = Fixture::new();
        if let Some(content) = stored {
            fixture.write("state/cockpit/settings.json", content);
        }
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Setting(0));
        app.handle(Key::Enter, &mut FakeScreen::default());
        assert_eq!(
            fixture.calls("cockpit"),
            vec![format!("cockpit setting phone-on-lock {wanted}")],
            "stored {stored:?}"
        );
        assert_eq!(app.message, format!("Phone on lock: {wanted}"));
    }
}

#[test]
fn any_other_key_closes_the_settings_menu_without_changing_anything() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(app.settings_menu.is_none());
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn settings_group_sessions_first_then_automation_then_phone() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let rows = settings_screen_rows(&render(&mut app, 60, 60));
    let row_of = |label: &str| {
        rows.iter()
            .position(|row| row.contains(label))
            .unwrap_or_else(|| panic!("missing {label}"))
    };
    for (before, after) in [
        ("SESSIONS", "Default agent"),
        ("OpenCode effort", "AUTOMATION"),
        ("AUTOMATION", "Todo queue"),
        ("Daily digest", "Phone on lock"),
        ("Phone on lock", "Pair a phone"),
    ] {
        assert!(
            row_of(before) < row_of(after),
            "{before} must precede {after}: {rows:?}"
        );
    }
}

#[test]
fn opening_settings_selects_the_default_agent_and_marks_keyboard_selection() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let rows = settings_screen_rows(&render(&mut app, 60, 40));
    assert!(
        rows.iter()
            .any(|row| row.contains('›') && row.contains("Default agent")),
        "{rows:?}"
    );
    app.handle(Key::Down, &mut FakeScreen::default());
    let rows = settings_screen_rows(&render(&mut app, 60, 40));
    assert!(
        rows.iter()
            .any(|row| row.contains('›') && row.contains("OpenCode model")),
        "{rows:?}"
    );
}

#[test]
fn settings_values_end_at_one_column_across_sections() {
    let fixture = Fixture::new();
    fixture.write("state/cockpit/settings.json", r#"{"default-agent":"opencode","opencode-model":"openai/gpt-6.1-sol","opencode-effort":"medium","todo-queue":false}"#);
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let rows = settings_screen_rows(&render(&mut app, 60, 40));
    let columns: Vec<usize> = [
        ("Default agent", "opencode ▾"),
        ("OpenCode model", "openai/gpt-6.1-sol ▾"),
        ("OpenCode effort", "medium ▾"),
        ("Todo queue", "off"),
    ]
    .iter()
    .map(|(label, value)| {
        let row = rows.iter().find(|row| row.contains(label)).unwrap();
        row.chars()
            .collect::<Vec<_>>()
            .windows(value.chars().count())
            .position(|characters| characters.iter().collect::<String>() == *value)
            .unwrap()
            + value.chars().count()
    })
    .collect();
    assert!(
        columns.iter().all(|column| *column == columns[0]),
        "unaligned columns: {columns:?}"
    );
}

#[test]
fn settings_sections_have_a_blank_line_between_groups() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let buffer = render(&mut app, 60, 40);
    let rows = settings_screen_rows(&buffer);
    for section in ["AUTOMATION", "PHONE"] {
        let row = rows.iter().position(|row| row.contains(section)).unwrap();
        let content: String = (4..56)
            .map(|column| buffer[(column, row as u16 - 1)].symbol())
            .collect();
        assert!(
            content.trim().is_empty(),
            "no space above {section}: {content:?}"
        );
    }
}

#[test]
fn settings_distinguish_selected_values_enabled_and_disabled_toggles_by_color() {
    let fixture = Fixture::new();
    fixture.write("state/cockpit/settings.json", r#"{"default-agent":"opencode","opencode-model":"openai/gpt-6.1-sol","opencode-effort":"medium","todo-queue":false}"#);
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let buffer = render(&mut app, 60, 40);
    let rows = settings_screen_rows(&buffer);
    for (label, value, color) in [
        ("Default agent", "opencode", crate::app::BLUE),
        ("OpenCode model", "openai/gpt-6.1-sol", crate::app::BLUE),
        ("OpenCode effort", "medium", crate::app::BLUE),
        ("Error hand-off", "on", crate::app::GREEN),
        ("Todo queue", "off", crate::app::FG_DARK),
    ] {
        let row = rows.iter().position(|row| row.contains(label)).unwrap();
        let column = rows[row]
            .chars()
            .collect::<Vec<_>>()
            .windows(value.chars().count())
            .position(|characters| characters.iter().collect::<String>() == value)
            .unwrap();
        assert_eq!(
            buffer[(column as u16, row as u16)].fg,
            ratatui::style::Color::Indexed(color),
            "{label}"
        );
    }
}

#[test]
fn short_settings_panels_scroll_the_selected_phone_into_view() {
    for (width, height) in [(32, 12), (60, 20)] {
        let fixture = Fixture::new();
        let phones: Vec<Value> = (0..12)
            .map(|index| phone(&format!("Mobile-{index:02}"), &format!("aa{index:02}")))
            .collect();
        phone_page(&fixture, json!(phones));
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Phone(11));
        let rows = settings_screen_rows(&render(&mut app, width, height));
        assert!(
            rows.iter()
                .any(|row| row.contains('›') && row.contains("Mobile-11")),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("x forget")),
            "phone action is not visible: {rows:?}"
        );
        app.handle(
            Key::Char('x'),
            &mut FakeScreen::answering(&[Key::Char('n')]),
        );
        assert_eq!(crate::remote::phones().len(), 12);
    }
}

#[test]
fn settings_can_save_a_default_agent_with_keyboard_or_mouse() {
    for click in [false, true] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Launch(0));
        if click {
            let drawn = render(&mut app, 60, 40);
            let row = (0..40)
                .find(|row| {
                    (0..60)
                        .map(|column| drawn[(column, *row)].symbol())
                        .collect::<String>()
                        .contains("Default agent")
                })
                .expect("no default agent row");
            app.handle(
                Key::Click {
                    x: 20,
                    y: row,
                    double: false,
                },
                &mut FakeScreen::typing(&["opencode"]),
            );
        } else {
            app.handle(Key::Enter, &mut FakeScreen::typing(&["opencode"]));
        }
        assert_eq!(
            fixture.calls("cockpit"),
            ["cockpit setting default-agent opencode"]
        );
    }
}

#[test]
fn cancelling_a_default_agent_change_writes_nothing() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    open_settings_row(&mut app, SettingsRow::Launch(0));
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn settings_show_the_saved_launch_preferences() {
    let fixture = Fixture::new();
    fixture.write("state/cockpit/settings.json", r#"{"default-agent":"opencode","opencode-model":"openai/gpt-6.1-sol","opencode-effort":"high"}"#);
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let drawn = format!("{:?}", render(&mut app, 60, 40));
    for text in [
        "Default agent",
        "opencode",
        "OpenCode model",
        "openai/gpt-6.1-sol",
        "OpenCode effort",
        "high",
    ] {
        assert!(drawn.contains(text), "missing {text}");
    }
}

#[test]
fn the_model_picker_loads_the_active_projects_catalog_and_saves_the_choice() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    model_catalog(&fixture, json!([{"id":"high"}]));
    let mut app = fixture.app();
    open_settings_row(&mut app, SettingsRow::Launch(1));
    choose_loaded_preference(&mut app, &fixture, "openai/gpt-6.1-sol");
    assert_eq!(
        fixture.calls("cockpit"),
        [
            "cockpit opencode-models /r/runway",
            "cockpit setting opencode-model openai/gpt-6.1-sol",
        ]
    );
}

#[test]
fn effort_choices_come_from_the_selected_models_variants() {
    for variants in [json!([{"id":"high"}]), json!([])] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            1
        )]));
        fixture.write(
            "state/cockpit/settings.json",
            r#"{"opencode-model":"openai/gpt-6.1-sol"}"#,
        );
        model_catalog(&fixture, variants.clone());
        fixture.shim(
            "fzf",
            &format!(
                "cat > '{}'\nhead -n 1 '{}'",
                fixture.root.join("choices").display(),
                fixture.root.join("choices").display()
            ),
        );
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Launch(2));
        choose_loaded_preference(&mut app, &fixture, "");
        let choices = fs::read_to_string(fixture.root.join("choices")).unwrap();
        assert_eq!(
            choices.contains("high"),
            !variants.as_array().unwrap().is_empty()
        );
        assert!(
            !choices.contains("low") && !choices.contains("max"),
            "invented variants: {choices}"
        );
    }
}

#[test]
fn catalog_errors_are_visible_and_do_not_write_preferences() {
    for body in ["echo 'catalog unavailable' >&2; exit 1", "echo 'not json'"] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            1
        )]));
        fixture.shim("cockpit", body);
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Launch(1));
        for _ in 0..100 {
            app.handle(Key::Enter, &mut FakeScreen::default());
            if app.message.starts_with("cannot load OpenCode models:") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            app.message.starts_with("cannot load OpenCode models:"),
            "{}",
            app.message
        );
        assert!(
            !fixture
                .calls("cockpit")
                .iter()
                .any(|call| call.contains(" setting "))
        );
    }
}

#[test]
fn a_malformed_model_preference_can_be_replaced_in_settings() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        1
    )]));
    fixture.write("state/cockpit/settings.json", r#"{"opencode-model":false}"#);
    model_catalog(&fixture, json!([]));
    let mut app = fixture.app();
    open_settings_row(&mut app, SettingsRow::Launch(1));
    choose_loaded_preference(&mut app, &fixture, "openai/gpt-6.1-sol");
    assert!(
        fixture
            .calls("cockpit")
            .contains(&"cockpit setting opencode-model openai/gpt-6.1-sol".into())
    );
}

#[test]
fn model_and_effort_overrides_can_be_reset_without_loading_a_catalog() {
    for (index, name) in [(1, "opencode-model"), (2, "opencode-effort")] {
        let fixture = Fixture::new();
        fixture.write(
            "state/cockpit/settings.json",
            r#"{"opencode-model":false,"opencode-effort":false}"#,
        );
        fixture.shim(
            "cockpit",
            "if [ \"$1\" = opencode-models ]; then exit 1; fi",
        );
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Launch(index));
        app.handle(Key::Char('d'), &mut FakeScreen::default());
        assert_eq!(
            fixture.calls("cockpit"),
            [format!("cockpit setting {name} ")]
        );
    }
}

#[test]
fn failed_catalogs_and_unavailable_models_do_not_prevent_resetting_effort() {
    for unavailable in [false, true] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([session(
            SESSION_A,
            "waiting",
            "runway",
            "/r/runway",
            1
        )]));
        fixture.write(
            "state/cockpit/settings.json",
            r#"{"opencode-model":"removed/model","opencode-effort":"high"}"#,
        );
        if unavailable {
            model_catalog(&fixture, json!([]));
        } else {
            fixture.shim(
                "cockpit",
                "if [ \"$1\" = opencode-models ]; then echo 'unavailable' >&2; exit 1; fi",
            );
        }
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Launch(2));
        for _ in 0..100 {
            app.handle(Key::Enter, &mut FakeScreen::default());
            if app.message.contains("cannot load") || app.message.contains("model unavailable") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            app.message.contains("cannot load") || app.message.contains("model unavailable"),
            "{}",
            app.message
        );
        app.handle(Key::Char('d'), &mut FakeScreen::default());
        assert!(
            fixture
                .calls("cockpit")
                .contains(&"cockpit setting opencode-effort ".into())
        );
    }
}

#[test]
fn the_settings_menu_lists_the_paired_phones_under_phone() {
    let fixture = Fixture::new();
    phone_page(
        &fixture,
        json!([phone("iPhone", "aa11"), phone("iPad", "bb22")]),
    );
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let drawn = format!("{:?}", render(&mut app, 60, 60));
    for shown in [
        "PHONE",
        "Pair a phone",
        "QR code",
        "iPhone",
        "iPad",
        "seen now",
    ] {
        assert!(drawn.contains(shown), "{shown} missing");
    }
}

#[test]
fn enter_on_pair_a_phone_shows_a_one_time_link_and_its_qr_code() {
    let fixture = Fixture::new();
    phone_page(&fixture, json!([]));
    let mut app = fixture.app();
    open_settings_row(&mut app, SettingsRow::PairPhone);
    app.handle(Key::Enter, &mut FakeScreen::default());
    let pairing = app.pairing.as_ref().expect("no pairing shown");
    assert!(
        pairing
            .link
            .starts_with("https://box.tail1234.ts.net/pair/"),
        "{}",
        pairing.link
    );
    let drawn = format!("{:?}", render(&mut app, 60, 40));
    assert!(
        drawn.contains("▀") && drawn.contains("scan within 10 minutes"),
        "no QR code drawn"
    );
    let digits = &pairing_digits(&app);
    assert!(
        drawn.contains(&format!("{} {}", &digits[..3], &digits[3..])),
        "the 6 digits are not shown"
    );
}

#[test]
fn pairing_from_settings_says_when_the_phone_opened_it() {
    let fixture = Fixture::new();
    phone_page(&fixture, json!([]));
    let mut app = fixture.app();
    open_settings_row(&mut app, SettingsRow::PairPhone);
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert!(
        app.pairing.is_some(),
        "Pair a phone in Settings shows no QR code"
    );
    fixture.write(
        "state/claude-remote/devices.json",
        &json!({"devices": [phone("iPhone", "aa11")]}).to_string(),
    );
    app.tick();
    assert_eq!(
        app.pairing
            .as_ref()
            .and_then(|pairing| pairing.paired.as_deref()),
        Some("iPhone")
    );
}

#[test]
fn pairing_without_tailscale_says_why_and_shows_nothing() {
    let fixture = Fixture::new();
    phone_page(&fixture, json!([]));
    fixture.shim("tailscale", "exit 1");
    let mut app = fixture.app();
    open_settings_row(&mut app, SettingsRow::PairPhone);
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert!(app.pairing.is_none());
    assert!(app.message.contains("is Tailscale up?"), "{}", app.message);
}

#[test]
fn x_on_a_phone_forgets_it_after_a_yes_only() {
    for (answer, kept) in [
        (Key::Char('y'), vec!["iPad"]),
        (Key::Char('n'), vec!["iPhone", "iPad"]),
    ] {
        let fixture = Fixture::new();
        phone_page(
            &fixture,
            json!([phone("iPhone", "aa11aa11ff"), phone("iPad", "bb22bb22ff")]),
        );
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Phone(0));
        app.handle(
            Key::Char('x'),
            &mut FakeScreen::answering(std::slice::from_ref(&answer)),
        );
        let names: Vec<String> = crate::remote::phones()
            .iter()
            .map(|phone| phone["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, kept, "answered {answer:?}");
    }
}

#[test]
fn choosing_another_effort_restarts_the_session_on_it() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    fixture.write(
        &format!("home/.cache/claude/statusline-{SESSION_A}.json"),
        r#"{"effort": "high"}"#,
    );
    let mut app = fixture.app();
    app.handle(Key::Char('e'), &mut FakeScreen::default());
    app.handle(Key::Down, &mut FakeScreen::default());
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit restart {SESSION_A} xhigh")]
    );
}

#[test]
fn dollar_switches_a_session_to_the_other_billing_after_a_yes() {
    for (credits, billing) in [(false, "credits"), (true, "subscription")] {
        let fixture = Fixture::new();
        let mut row = session(SESSION_A, "waiting", "runway", "/r", 1);
        row["credits"] = json!(credits);
        fixture.snapshot(json!([row]));
        let mut app = fixture.app();
        app.handle(
            Key::Char('$'),
            &mut FakeScreen::answering(&[Key::Char('y')]),
        );
        assert_eq!(
            fixture.wait_for_calls("cockpit", 1),
            vec![format!("cockpit billing {SESSION_A} {billing}")]
        );
    }
}

#[test]
fn dollar_without_a_yes_restarts_nothing() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    app.handle(
        Key::Char('$'),
        &mut FakeScreen::answering(&[Key::Char('n')]),
    );
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn a_credits_session_wears_a_badge() {
    let fixture = Fixture::new();
    let mut row = session(SESSION_A, "waiting", "runway", "/r", 1);
    row["credits"] = json!(true);
    fixture.snapshot(json!([
        row,
        session(SESSION_B, "waiting", "opusline", "/o", 2)
    ]));
    let app = fixture.app();
    assert!(selected_session_badges(&app).contains(&"$ credits".into()));
    assert_eq!(app.sessions[app.panels[1].selected].project, "runway");
}

#[test]
fn the_current_effort_or_escape_restarts_nothing() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    fixture.write(
        &format!("home/.cache/claude/statusline-{SESSION_A}.json"),
        r#"{"effort": "high"}"#,
    );
    let mut app = fixture.app();
    app.handle(Key::Char('e'), &mut FakeScreen::default());
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(app.message, "runway already runs on high");
    app.handle(Key::Char('e'), &mut FakeScreen::default());
    app.handle(Key::Esc, &mut FakeScreen::default());
    assert!(app.menu.is_none());
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn a_digit_picks_an_effort_level_directly() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "runway", "/r", 1)]));
    let mut app = fixture.app();
    app.handle(Key::Char('e'), &mut FakeScreen::default());
    app.handle(Key::Char('1'), &mut FakeScreen::default());
    assert_eq!(
        fixture.wait_for_calls("cockpit", 1),
        vec![format!("cockpit restart {SESSION_A} low")]
    );
}

#[test]
fn settings_menu_cycles_the_sleep_delay() {
    for (stored, wanted) in [
        (None, "120"),
        (Some(r#"{"idle-sleep-after":"120"}"#), "240"),
        (Some(r#"{"idle-sleep-after":"240"}"#), "30"),
    ] {
        let fixture = Fixture::new();
        if let Some(content) = stored {
            fixture.write("state/cockpit/settings.json", content);
        }
        let mut app = fixture.app();
        open_settings_row(&mut app, SettingsRow::Launch(3));
        app.handle(Key::Enter, &mut FakeScreen::default());
        assert_eq!(
            fixture.calls("cockpit"),
            vec![format!("cockpit setting idle-sleep-after {wanted}")],
            "stored {stored:?}"
        );
    }
}

#[test]
fn effort_opens_opencodes_native_picker_without_restarting() {
    for state in ["waiting", "working", "needs-you"] {
        let fixture = Fixture::new();
        fixture.snapshot(json!([opencode_session(state, "runway", 1)]));
        let mut app = fixture.app();
        assert!(app.sessions_hints().contains(&("e", "effort")));
        app.handle(Key::Char('e'), &mut FakeScreen::default());
        assert_eq!(
            fixture.wait_for_calls("cockpit", 1),
            [format!("cockpit variants {OPENCODE_SESSION}")]
        );
        assert!(app.menu.is_none());
    }
}

#[test]
fn the_settings_menu_has_one_background_behind_its_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let buffer = render(&mut app, 80, 40);
    let rows = buffer_rows(&buffer);
    let top = rows
        .iter()
        .position(|row| row.contains('╭'))
        .expect("settings are open") as u16;
    let bottom = rows
        .iter()
        .position(|row| row.contains('╰'))
        .expect("settings end") as u16;
    let left = rows[top as usize]
        .chars()
        .position(|symbol| symbol == '╭')
        .unwrap() as u16;
    let right = rows[top as usize]
        .chars()
        .position(|symbol| symbol == '╮')
        .unwrap() as u16;
    // The highlight behind the selected row and key caps, and the amber primary key cap, are designed.
    assert_one_background(
        &buffer,
        top + 1..bottom,
        left + 1..right,
        &[
            DESIGNED_HIGHLIGHT,
            ratatui::style::Color::Indexed(crate::app::YELLOW),
        ],
    );
}

#[test]
fn every_setting_explains_itself_on_a_dim_line_under_its_name() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    let buffer = render(&mut app, 60, 60);
    let rows = settings_screen_rows(&buffer);
    for (name, explanation) in [
        ("Default agent", "what N starts"),
        ("Todo queue", "start queued todos by themselves"),
        ("Freeze while gaming", "hold banners and new work"),
        ("Daily digest", "one summary each morning"),
    ] {
        let row = rows
            .iter()
            .position(|row| row.contains(name))
            .unwrap_or_else(|| panic!("{name}: {rows:#?}"));
        assert!(
            rows[row + 1].contains(explanation),
            "{name}: {}",
            rows[row + 1]
        );
        let column = rows[row + 1]
            .find(explanation)
            .map(|byte| rows[row + 1][..byte].chars().count())
            .unwrap() as u16;
        assert_eq!(
            buffer[(column, row as u16 + 1)].fg,
            ratatui::style::Color::Indexed(crate::app::FG_DARK),
            "{name}"
        );
    }
    let footer = rows
        .iter()
        .find(|row| row.contains("change"))
        .expect("key caps");
    assert!(
        footer.contains("move") && footer.contains("close"),
        "{footer}"
    );
}
