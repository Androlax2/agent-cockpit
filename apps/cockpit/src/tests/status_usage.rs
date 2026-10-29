use super::*;

#[test]
fn status_details_take_no_space_until_requested() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let rows = settings_screen_rows(&render(&mut app, 100, 45));
    assert!(rows[1].contains(" 1  Sessions"), "{rows:?}");
    assert!(!rows.iter().any(|row| row.contains("USAGE")));
}

#[test]
fn s_opens_status_over_the_tabs_and_s_or_escape_returns_to_the_tab() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let screen = &mut FakeScreen::default();
    for close in [Key::Char('s'), Key::Esc] {
        app.focus_panel(3);
        app.handle(Key::Char('s'), screen);
        let rows = settings_screen_rows(&render(&mut app, 100, 45));
        for heading in ["USAGE", "SPENDING", "CHIEF", "SERVICES", "PULL REQUESTS"] {
            assert!(
                rows.iter().any(|row| row.contains(heading)),
                "{heading}: {rows:?}"
            );
        }
        app.handle(close, screen);
        assert_eq!(app.focus, 3);
    }
}

#[test]
fn p_in_status_moves_to_its_pull_requests_and_escape_closes_status() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let screen = &mut FakeScreen::default();
    app.handle(Key::Char('s'), screen);
    app.handle(Key::Char('p'), screen);
    assert_eq!(app.focus, 2);
    app.handle(Key::Esc, screen);
    assert_eq!(app.focus, 1);
}

#[test]
fn status_totals_all_usage_at_api_prices_and_keeps_time_by_project() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    app.handle(Key::Char('s'), &mut FakeScreen::default());
    let rows = settings_screen_rows(&render(&mut app, 120, 60)).join("\n");
    assert!(
        !rows.contains("Loading usage…") || rows.contains("SPENDING"),
        "{rows}"
    );
}

#[test]
fn clicking_the_header_status_link_toggles_the_details() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"seven_day":{"utilization":50}}"#,
    );
    let mut app = fixture.app();
    app.focus_panel(3);
    render(&mut app, 100, 45);
    let &(start, _, _) = app.hit_actions.get(&0).unwrap();
    let screen = &mut FakeScreen::default();
    app.handle(
        Key::Click {
            x: start,
            y: 0,
            double: false,
        },
        screen,
    );
    assert_eq!(app.focus, 0);
    render(&mut app, 100, 45);
    app.handle(
        Key::Click {
            x: start,
            y: 0,
            double: false,
        },
        screen,
    );
    assert_eq!(app.focus, 3);
}

#[test]
fn action_counts_do_not_push_allowances_out_of_a_narrow_header() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"seven_day":{"utilization":95}}"#,
    );
    fixture.write(
        "run/codex-usage.json",
        r#"{"rate_limit":{"primary_window":{"used_percent":48,"limit_window_seconds":604800}}}"#,
    );
    fixture.snapshot(json!([session(SESSION_A, "needs-you", "runway", "/r", 1)]));
    let mut app = fixture.app();
    propose(&fixture, "An idea", "Why");
    app.todos.apply_updates();
    app.build();
    app.held = vec!["blocked.example".into()];
    let rows = settings_screen_rows(&render(&mut app, 60, 45));
    for text in ["Claude ▮▯▯▯ 5%", "Codex ▮▮▮▯ 52%"] {
        assert!(rows[0].contains(text), "{}", rows[0]);
    }
    // The session and greyproxy's hold both need Theo.
    assert!(
        rows[1].contains("Sessions  2") && rows[1].contains("Ideas  1"),
        "the counts moved to the tab badges: {}",
        rows[1]
    );
}

#[test]
fn the_header_never_says_claude_sessions() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let rows = settings_screen_rows(&render(&mut app, 80, 45));
    assert!(!rows[0].contains("Claude sessions"), "{}", rows[0]);
}

#[test]
fn clicking_a_tab_opens_it() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let rows = settings_screen_rows(&render(&mut app, 80, 45));
    let column = rows[1].find("Ideas").unwrap() as u16;
    app.handle(
        Key::Click {
            x: column,
            y: 1,
            double: false,
        },
        &mut FakeScreen::default(),
    );
    assert_eq!(app.focus, 4);
}

#[test]
fn the_header_shows_each_providers_tightest_allowance_without_overlapping_attention() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":0},"seven_day":{"utilization":77}}"#,
    );
    fixture.write(
        "run/codex-usage.json",
        r#"{"rate_limit":{"primary_window":{"used_percent":48,"limit_window_seconds":604800}}}"#,
    );
    fixture.snapshot(json!([session(SESSION_A, "needs-you", "runway", "/r", 1)]));
    let mut app = fixture.app();
    for width in [60, 130] {
        let rows = settings_screen_rows(&render(&mut app, width, 45));
        for text in ["Claude ▮▯▯▯ 23%", "Codex ▮▮▮▯ 52%"] {
            assert!(rows[0].contains(text), "{}", rows[0]);
        }
    }
}

#[test]
fn the_header_never_presents_stale_or_blocked_allowances_as_available() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":0}}"#,
    );
    fixture.write("run/claude-usage-error.json", r#"{"error":"failed"}"#);
    fixture.write("run/codex-usage.json", r#"{"rate_limit":{"allowed":false,"primary_window":{"used_percent":0,"limit_window_seconds":18000}}}"#);
    let mut app = fixture.app();
    let rows = settings_screen_rows(&render(&mut app, 100, 45));
    assert!(rows[0].contains("Claude stale"), "{}", rows[0]);
    assert!(rows[0].contains("Codex blocked"), "{}", rows[0]);
    assert!(!rows[0].contains("100%"));
}

#[test]
fn low_allowances_remain_red_in_the_header() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"seven_day":{"utilization":95}}"#,
    );
    let mut app = fixture.app();
    let buffer = render(&mut app, 100, 45);
    let rows = settings_screen_rows(&buffer);
    let start = rows[0]
        .chars()
        .take_while(|_| true)
        .collect::<String>()
        .find("Claude")
        .unwrap();
    let start = rows[0][..start].chars().count();
    assert!(rows[0].contains("Claude ▮▯▯▯ 5%"), "{}", rows[0]);
    assert_eq!(
        buffer[(start as u16, 0)].fg,
        crate::app::color(crate::app::RED)
    );
}

#[test]
fn the_header_keeps_service_outages_visible_when_status_is_closed() {
    for (provider, variable) in [
        ("Claude", "CLAUDE_STATUS_URL"),
        ("Codex", "CODEX_STATUS_URL"),
    ] {
        let fixture = Fixture::new();
        let summary = fixture.write(
            "status.json",
            &json!({"status": {"indicator": "major", "description": "Service disruption"}})
                .to_string(),
        );
        unsafe { std::env::set_var(variable, format!("file://{}", summary.display())) };
        let mut app = fixture.app();
        app.claude_status.fetch_now();
        app.codex_status.fetch_now();
        app.build();
        let rows = settings_screen_rows(&render(&mut app, 60, 45));
        assert!(rows[0].contains(&format!("! {provider}")), "{}", rows[0]);
    }
}

#[test]
fn status_groups_confirmed_windows_in_aligned_columns_at_each_terminal_width() {
    let fixture = Fixture::new();
    let reset = chrono::Utc::now() + chrono::Duration::hours(3);
    fixture.write(
        "run/claude-usage.json",
        &json!({"limits": [
            {"kind": "session", "percent": 80, "resets_at": reset.to_rfc3339()},
            {"kind": "weekly_all", "percent": 0},
            {"kind": "weekly_scoped", "percent": 65, "scope": {"model": {"display_name": "Fable"}}}
        ]})
        .to_string(),
    );
    fixture.write(
        "run/codex-usage.json",
        r#"{"rate_limit":{"secondary_window":{"used_percent":67,"limit_window_seconds":604800}}}"#,
    );
    let mut app = fixture.app();
    app.focus_panel(0);
    for width in [40, 60, 80, 140] {
        app.width = width;
        app.redraw();
        let rows = texts(&app, 0);
        assert_eq!(
            rows.iter().filter(|row| row.as_str() == "Claude").count(),
            1,
            "{rows:?}"
        );
        assert_eq!(
            rows.iter().filter(|row| row.as_str() == "Codex").count(),
            1,
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("Fable weekly")),
            "{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("Available")),
            "{rows:?}"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains("window-average pace"))
                .count(),
            1
        );
        let quota_lines: Vec<_> = app.panels[0]
            .lines
            .iter()
            .filter(|line| line.item.is_some_and(|index| index < 4))
            .cloned()
            .collect();
        for (index, label) in [
            (0, "5-hour"),
            (1, "Weekly"),
            (2, "Fable weekly"),
            (3, "Weekly"),
        ] {
            assert!(
                quota_lines
                    .iter()
                    .any(|line| line.item == Some(index) && line.texts().contains(label))
            );
        }
        assert_eq!(
            app.panels[0].items, 8,
            "four quotas and four Status actions"
        );
        assert!(
            !quota_lines
                .iter()
                .any(|line| line.item == Some(3) && line.texts().contains("5-hour"))
        );
        for line in &app.panels[0].lines {
            for adjacent in line.segments.windows(2) {
                assert!(
                    adjacent[0].0 as usize + crate::app::width_of(&adjacent[0].1)
                        <= adjacent[1].0 as usize,
                    "overlap at {width}: {}",
                    line.texts()
                );
            }
            if line.texts().contains("LEFT")
                || line.texts().contains("window-average")
                || line.texts().contains("SERVICE HEALTH")
            {
                assert_eq!(line.item, None);
            }
        }
        let buffer = render(&mut app, width, 65);
        let rendered: Vec<String> = (0..65)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        for text in [
            "5-hour",
            "20%",
            "Low",
            "100%",
            "Fable weekly",
            "33%",
            "~30m",
            "2h 59m",
        ] {
            assert!(
                rendered.iter().any(|row| row.contains(text)),
                "width {width}, missing {text}: {rendered:#?}"
            );
        }
        if width >= 80 {
            let first = quota_lines
                .iter()
                .find(|line| line.item == Some(0))
                .unwrap();
            assert!(first.texts().contains("~30m") && first.texts().contains("2h 59m"));
            let offsets: Vec<_> = quota_lines.iter().map(|line| line.segments[1].0).collect();
            assert!(
                offsets.iter().all(|offset| *offset == offsets[0]),
                "{offsets:?}"
            );
        } else if width == 40 {
            assert!(
                quota_lines
                    .iter()
                    .filter(|line| line.item == Some(0))
                    .count()
                    > 1
            );
        }
    }
}

#[test]
fn status_spending_keeps_all_time_cost_out_of_the_plan_subtitle() {
    let fixture = Fixture::new();
    fixture.write(
        "home/.claude/.credentials.json",
        r#"{"claudeAiOauth":{"subscriptionType":"max"}}"#,
    );
    let stamp = chrono::Utc::now();
    fixture.write("home/.claude/projects/p/s.jsonl", &format!("{}\n{}\n", json!({"timestamp": (stamp - chrono::Duration::minutes(2)).to_rfc3339(), "sessionId": "s", "message": {
        "id": "m", "model": "claude-opus-5-5", "usage": {"output_tokens": 1000000}
    }}), json!({"timestamp": stamp.to_rfc3339(), "sessionId": "s", "message": {
        "id": "m2", "model": "claude-opus-5-5", "usage": {"output_tokens": 0}
    }})));
    let mut app = fixture.app();
    app.costs.tick();
    for _ in 0..200 {
        if app.costs.all_time() > 0.0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    app.build();
    assert_eq!(app.subtitle(0), "Max");
    let rows = texts(&app, 0);
    let spending = rows
        .iter()
        .position(|row| row.contains("SPENDING"))
        .unwrap();
    let health = rows.iter().position(|row| row == "SERVICES").unwrap();
    let all_time = rows
        .iter()
        .position(|row| row.contains("All-time API value") && row.contains("≈ $20"))
        .unwrap();
    assert!(spending < all_time && all_time < health, "{rows:?}");
    assert!(rows[spending].contains("token-based estimates, not your bill"));
    assert!(
        rows.iter()
            .any(|row| row.contains("TODAY") && row.contains("WEEK") && row.contains("MONTH"))
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Active time") && row.contains("2m")),
        "{rows:?}"
    );
    app.focus_panel(0);
    let buffer = render(&mut app, 80, 60);
    let rendered: String = (0..60)
        .flat_map(|y| (0..80).map(move |x| (x, y)))
        .map(|cell| buffer[cell].symbol())
        .collect();
    assert!(rendered.contains("All-time API value") && rendered.contains("$20"));
}

#[test]
fn status_keeps_cost_errors_visible_without_fabricating_spending() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path("state/cockpit/usage.sqlite")).unwrap();
    let mut app = fixture.app();
    app.costs.tick();
    for _ in 0..200 {
        if !app.costs.error().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    app.build();
    let rows = texts(&app, 0).join(" ");
    assert!(
        rows.contains("SPENDING") && rows.contains("usage archive:"),
        "{rows}"
    );
    assert!(
        !rows.contains("All-time API value")
            && !rows.contains("Active time")
            && !rows.contains("$0.00")
    );
    app.focus_panel(0);
    let buffer = render(&mut app, 80, 60);
    let rendered: String = (0..60)
        .flat_map(|y| (0..80).map(move |x| (x, y)))
        .map(|cell| buffer[cell].symbol())
        .collect();
    assert!(rendered.contains("usage archive:"));
}

#[test]
fn the_header_shows_all_of_todays_usage_at_api_prices() {
    // Theo's call in the redesign: the estimate shows even on the subscription (DESIGN.md, Layout).
    let fixture = Fixture::new();
    let mut app = app_with_billing(
        &fixture,
        &[
            ("api", Some("api"), Some(12.0)),
            ("plan", Some("subscription"), Some(54.0)),
            ("legacy", None, Some(8.0)),
        ],
    );
    let buffer = render(&mut app, 140, 70);
    let header: String = (0..140)
        .map(|column| buffer[(column, 0)].symbol())
        .collect();
    assert!(header.contains("≈ $74.00 today"), "{header}");
    app.handle(Key::Char('s'), &mut FakeScreen::default());
    let rows = texts(&app, 0);
    assert!(
        rows.iter()
            .any(|row| row.contains("≈ Total at API prices") && row.contains("$74.00")),
        "{rows:?}"
    );
    let narrow = render(&mut app, 24, 70);
    let header: String = (0..24).map(|column| narrow[(column, 0)].symbol()).collect();
    assert!(!header.contains('$'), "partial monetary fragment: {header}");
}

#[test]
fn historical_billing_stays_visible_in_status() {
    let fixture = Fixture::new();
    let mut app = app_with_billing(&fixture, &[("legacy", None, Some(74.0))]);
    render(&mut app, 140, 70);
    let rows = texts(&app, 0);
    assert!(
        rows.iter()
            .any(|row| row.contains("Unknown billing (API value)") && row.contains("$74.00")),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("API credits (est.)") && row.contains("$0.00")),
        "{rows:?}"
    );
}

#[test]
fn billing_rows_keep_their_meaning_at_wide_and_narrow_widths() {
    let fixture = Fixture::new();
    let mut app = app_with_billing(
        &fixture,
        &[
            ("api", Some("api"), Some(12_345_678.0)),
            ("plan", Some("subscription"), Some(54.0)),
            ("legacy", None, Some(8.0)),
        ],
    );
    app.focus_panel(0);
    for width in [24, 40, 80, 140] {
        let _ = render(&mut app, width, 120);
        let rows = texts(&app, 0).join(" ");
        assert!(rows.contains("API credits (est.)"), "{width}: {rows}");
        assert!(rows.contains("Subscription (API value)"), "{width}: {rows}");
        assert!(
            rows.contains("Unknown billing (API value)"),
            "{width}: {rows}"
        );
        assert!(
            rows.contains("not a subscription charge.")
                && rows.contains("includes historical records."),
            "{width}: {rows}"
        );
        for line in &app.panels[0].lines {
            for pair in line.segments.windows(2) {
                assert!(
                    pair[0].0 as usize + pair[0].1.chars().count() <= pair[1].0 as usize,
                    "overlapping spending columns at width {width}: {}",
                    line.texts()
                );
            }
        }
        if width == 40 {
            assert!(
                texts(&app, 0)
                    .iter()
                    .any(|row| row.contains("TODAY") && row.contains("$12345.7k")),
                "{rows}"
            );
        }
    }
}

#[test]
fn no_recorded_usage_is_not_presented_as_a_zero_bill() {
    let fixture = Fixture::new();
    let app = app_with_billing(&fixture, &[]);
    let rows = texts(&app, 0).join(" ");
    assert!(rows.contains("No recorded usage"), "{rows}");
    assert!(!rows.contains("$0.00"), "{rows}");
}

#[test]
fn an_unpriced_reply_makes_the_estimate_visibly_incomplete() {
    let fixture = Fixture::new();
    let app = app_with_billing(&fixture, &[("unpriced", Some("api"), None)]);
    let rows = texts(&app, 0).join(" ");
    assert!(
        rows.contains("Incomplete estimate") && rows.contains("1 unpriced"),
        "{rows}"
    );
    assert!(
        !rows.contains("$0.00"),
        "unpriced usage is not free: {rows}"
    );
    assert!(
        rows.contains("API credits (est.)") && rows.contains("Unknown billing (API value)"),
        "{rows}"
    );
}

#[test]
fn usage_loading_does_not_imply_a_verified_zero() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let rows = texts(&app, 0).join(" ");
    assert!(rows.contains("Loading usage"), "{rows}");
    assert!(!rows.contains("$0.00"), "{rows}");
}

#[test]
fn healthy_services_are_independently_selectable_at_every_width() {
    let fixture = Fixture::new();
    let feed = fixture.write(
        "status.json",
        r#"{"status":{"indicator":"none","description":"All Systems Operational"}}"#,
    );
    unsafe {
        std::env::set_var("CLAUDE_STATUS_URL", format!("file://{}", feed.display()));
        std::env::set_var("CODEX_STATUS_URL", format!("file://{}", feed.display()));
    }
    let mut app = fixture.app();
    app.claude_status.fetch_now();
    app.codex_status.fetch_now();
    app.build();
    app.focus_panel(0);
    for width in [40, 60, 80, 140] {
        app.width = width;
        app.redraw();
        let rows = texts(&app, 0);
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("All Systems Operational"))
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains("Operational"))
                .count(),
            2,
            "{rows:?}"
        );
        let service_items: Vec<_> = app.panels[0]
            .lines
            .iter()
            .filter(|line| line.texts().contains("Operational"))
            .map(|line| line.item.unwrap())
            .collect();
        assert_ne!(service_items[0], service_items[1]);
        let buffer = render(&mut app, width, 60);
        let rendered: Vec<String> = (0..60)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert!(
            rendered
                .iter()
                .any(|row| row.contains("Claude") && row.contains("Operational"))
        );
        assert!(
            rendered
                .iter()
                .any(|row| row.contains("Codex") && row.contains("Operational"))
        );
    }
}

#[test]
fn narrow_status_forecasts_use_state_sensitive_sentences() {
    let fixture = Fixture::new();
    for (used, age, expected) in [
        (60, 0, "Runs out in ~1h 20m"),
        (10, 0, "Lasts until reset"),
        (99, 300, "May run out now"),
        (0, 0, "Runs out in —"),
    ] {
        let fetched = std::time::SystemTime::now() - Duration::from_secs(age);
        let reset = chrono::DateTime::<chrono::Utc>::from(fetched) + chrono::Duration::hours(3);
        let cache = fixture.write(
            "run/claude-usage.json",
            &json!({"five_hour": {"utilization": used, "resets_at": reset.to_rfc3339()}})
                .to_string(),
        );
        fs::File::options()
            .write(true)
            .open(cache)
            .unwrap()
            .set_modified(fetched)
            .unwrap();
        let mut app = fixture.app();
        app.focus_panel(0);
        app.width = 40;
        app.redraw();
        let buffer = render(&mut app, 40, 45);
        let rows: Vec<String> = (0..45)
            .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert!(
            rows.iter().any(|row| row.contains(expected)),
            "{used}: {rows:#?}"
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("Runs out in until") || row.contains("Runs out in may"))
        );
    }
}

#[test]
fn focused_status_keeps_capacity_forecast_and_reset_colours() {
    use crate::app::{FG_DARK, GREEN, RED, YELLOW};
    use ratatui::style::{Color, Modifier};

    let fixture = Fixture::new();
    for (used, age, forecast, capacity_color, forecast_color) in [
        (60, 0, "~1h 20m", YELLOW, YELLOW),
        (80, 0, "~30m", RED, RED),
        (99, 300, "may run out now", RED, RED),
        (10, 0, "until reset", GREEN, FG_DARK),
    ] {
        let fetched = std::time::SystemTime::now() - Duration::from_secs(age);
        let reset = chrono::DateTime::<chrono::Utc>::from(fetched) + chrono::Duration::hours(3);
        let cache = fixture.write(
            "run/claude-usage.json",
            &json!({"five_hour": {"utilization": used, "resets_at": reset.to_rfc3339()}})
                .to_string(),
        );
        fs::File::options()
            .write(true)
            .open(cache)
            .unwrap()
            .set_modified(fetched)
            .unwrap();
        let mut app = fixture.app();
        app.focus_panel(0);
        app.width = 80;
        app.redraw();
        let buffer = render(&mut app, 80, 45);
        let rendered: Vec<String> = (0..45)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        for (text, tint) in [
            (format!("{}%", 100 - used), capacity_color),
            (forecast.into(), forecast_color),
        ] {
            let y = rendered
                .iter()
                .position(|row| row.contains("5-hour") && row.contains(&text))
                .unwrap();
            let x = crate::app::width_of(&rendered[y][..rendered[y].find(&text).unwrap()]);
            assert_eq!(
                buffer[(x as u16, y as u16)].fg,
                Color::Indexed(tint),
                "{used}: {text}"
            );
        }
        let reset = app.panels[0]
            .lines
            .iter()
            .find(|line| line.item == Some(0))
            .unwrap()
            .segments
            .last()
            .unwrap()
            .1
            .clone();
        let y = rendered
            .iter()
            .position(|row| row.contains("5-hour"))
            .unwrap();
        let x = crate::app::width_of(&rendered[y][..rendered[y].rfind(&reset).unwrap()]);
        let cell = &buffer[(x as u16, y as u16)];
        assert_eq!(cell.fg, Color::Indexed(FG_DARK));
        assert!(cell.modifier.contains(Modifier::DIM));
    }
}

#[test]
fn status_keeps_exceptional_quota_states_explicit_at_each_width() {
    let fixture = Fixture::new();
    for (used, reset, flags, expected) in [
        (json!(null), None, json!({}), "Unknown"),
        (json!(100), None, json!({}), "Exhausted"),
        (json!(80), None, json!({}), "Low"),
        (json!(10), None, json!({"allowed": false}), "Blocked"),
        (
            json!(28),
            Some(1577836800_i64),
            json!({}),
            "Stale · last 72%",
        ),
    ] {
        let mut limits = flags;
        limits["secondary_window"] =
            json!({"used_percent": used, "limit_window_seconds": 604800, "reset_at": reset});
        fixture.write(
            "run/codex-usage.json",
            &json!({"rate_limit": limits}).to_string(),
        );
        let mut app = fixture.app();
        app.focus_panel(0);
        for width in [40, 60, 80, 140] {
            app.width = width;
            app.redraw();
            let buffer = render(&mut app, width, 60);
            let rendered: Vec<String> = (0..60)
                .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            assert!(
                rendered
                    .iter()
                    .any(|row| row.contains("Weekly") && row.contains(expected)),
                "width {width}: {rendered:#?}"
            );
            assert!(!quota_texts(&app).iter().any(|row| row.contains('~')
                || row.contains("until reset")
                || row.contains("may run out now")));
        }
    }
}

#[test]
fn status_wraps_outage_diagnostics_without_normalising_away_incidents() {
    let fixture = Fixture::new();
    let description = "Workspace requests unavailable while connection recovery continues";
    let component = "Codex Desktop integrations: major outage";
    let incident = "Investigating elevated failures affecting workspace requests";
    let feed = fixture.write(
        "status.json",
        &json!({"status": {"indicator": "major", "description": description},
        "components": [{"name": "Codex Desktop integrations", "status": "major_outage"}],
        "incidents": [{"name": incident, "status": "investigating"}]})
        .to_string(),
    );
    unsafe {
        std::env::set_var("CODEX_STATUS_URL", format!("file://{}", feed.display()));
    }
    let mut app = fixture.app();
    app.codex_status.fetch_now();
    app.build();
    app.focus_panel(0);
    for width in [40, 60, 80, 140] {
        app.width = width;
        app.redraw();
        let buffer = render(&mut app, width, 65);
        let rendered = (0..65)
            .map(|y| {
                (2..width - 2)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .map(|row| row.trim().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let rendered = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
        for detail in [description, component, incident] {
            assert!(rendered.contains(detail), "width {width}: {rendered}");
        }
        assert!(!rendered.contains("Operational"));
    }
}

#[test]
fn status_continuations_keep_quota_selection_and_scrolling() {
    let fixture = Fixture::new();
    fixture.write("run/claude-usage.json", r#"{"limits":[{"kind":"session","percent":0},{"kind":"weekly_scoped","percent":28,"scope":{"model":{"display_name":"Long scoped model with a separate quota"}}}]}"#);
    fixture.write(
        "run/codex-usage.json",
        r#"{"rate_limit":{"secondary_window":{"used_percent":67,"limit_window_seconds":604800}}}"#,
    );
    let mut app = fixture.app();
    app.focus_panel(0);
    app.width = 40;
    app.redraw();
    for index in 0..3 {
        assert_eq!(app.panels[0].selected, index);
        let buffer = render(&mut app, 40, 12);
        assert!(
            app.panels[0].lines[app.panels[0].scroll..]
                .iter()
                .take(8)
                .any(|line| line.item == Some(index))
        );
        assert!(
            app.panels[0]
                .lines
                .iter()
                .filter(|line| line.item == Some(index))
                .count()
                >= 3
        );
        let rows: Vec<String> = (0..12)
            .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert!(
            rows.iter().any(|row| row.contains(if index == 0 {
                "5-hour"
            } else if index == 1 {
                "Long scoped model"
            } else {
                "Weekly"
            })),
            "{rows:#?}"
        );
        app.handle(Key::Down, &mut FakeScreen::default());
    }
    assert_eq!(
        app.panels[0].selected, 3,
        "navigation continues into Services"
    );
    app.handle(Key::Char('s'), &mut FakeScreen::default());
    assert_ne!(app.focus, 0);
}

#[test]
fn grouped_status_leaves_external_quota_summaries_unchanged() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":28},"seven_day":{"utilization":86}}"#,
    );
    fixture.write(
        "run/codex-usage.json",
        r#"{"rate_limit":{"secondary_window":{"used_percent":100,"limit_window_seconds":604800}}}"#,
    );
    let mut app = fixture.app();
    app.focus_panel(0);
    render(&mut app, 80, 45);
    let rows: Vec<_> = usage::quota_rows()
        .iter()
        .map(|quota| (quota.label.clone(), quota.summary()))
        .collect();
    assert_eq!(
        rows,
        [
            ("Claude · 5h".into(), "Available · 72% left".into()),
            ("Claude · week".into(), "Low · 14% left".into()),
            ("Codex · week".into(), "Exhausted · 0% left".into())
        ]
    );
}

#[test]
fn the_status_panel_lists_remaining_capacity_with_its_reset() {
    let fixture = Fixture::new();
    let soon = (chrono::Utc::now() + chrono::Duration::hours(2))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    fixture.write("run/claude-usage.json", &json!({"five_hour": {"utilization": 37.4, "resets_at": soon}, "seven_day": {"utilization": 72}}).to_string());
    fixture.write(
        "home/.claude/.credentials.json",
        r#"{"claudeAiOauth": {"subscriptionType": "max"}}"#,
    );
    let app = fixture.app();
    let lines = quota_texts(&app);
    assert!(
        lines[0].starts_with("Claude · 5-hour") && lines[0].contains("63%"),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].contains("1h 5") || lines[0].contains("2h"),
        "{}",
        lines[0]
    );
    assert_eq!(app.subtitle(0), "Max");
}

#[test]
fn quota_forecasts_use_each_providers_current_and_weekly_window_average() {
    let fixture = Fixture::new();
    let checked = chrono::Utc::now();
    let session_reset = checked + chrono::Duration::hours(3);
    let week_reset = checked + chrono::Duration::days(5);
    fixture.write(
        "run/claude-usage.json",
        &json!({"limits": [
            {"kind": "session", "percent": 80, "resets_at": session_reset.to_rfc3339()},
            {"kind": "weekly_all", "percent": 80, "resets_at": week_reset.to_rfc3339()}
        ]})
        .to_string(),
    );
    fixture.write("run/codex-usage.json", &json!({"rate_limit": {
        "primary_window": {"used_percent": 80, "limit_window_seconds": 18000, "reset_at": session_reset.timestamp()},
        "secondary_window": {"used_percent": 80, "limit_window_seconds": 604800, "reset_at": week_reset.timestamp()}
    }}).to_string());
    let mut app = fixture.app();
    app.width = 60;
    app.redraw();
    for (label, expected) in [
        ("Claude · 5-hour", "~30m"),
        ("Claude · Weekly", "~12h 00m"),
        ("Codex · 5-hour", "~30m"),
        ("Codex · Weekly", "~12h 00m"),
    ] {
        let rows = quota_texts(&app);
        let index = rows.iter().position(|row| row.starts_with(label)).unwrap();
        assert!(rows[index].contains(expected), "{rows:?}");
    }
    app.focus_panel(0);
    let buffer = render(&mut app, 60, 45);
    let rows: Vec<_> = (0..45)
        .map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect();
    assert!(rows.iter().any(|row| row.contains("~30m")), "{rows:?}");
    assert!(rows.iter().any(|row| row.contains("~12h 00m")), "{rows:?}");
}

#[test]
fn quota_forecasts_show_when_capacity_should_last_until_reset() {
    let fixture = Fixture::new();
    let reset = chrono::Utc::now() + chrono::Duration::hours(1);
    fixture.write(
        "run/claude-usage.json",
        &json!({"five_hour": {"utilization": 20, "resets_at": reset.to_rfc3339()}}).to_string(),
    );
    fixture.write("run/codex-usage.json", &json!({"rate_limit": {
        "primary_window": {"used_percent": 20, "limit_window_seconds": 7200, "reset_at": reset.timestamp()}
    }}).to_string());
    let rows = quota_texts(&fixture.app());
    for label in ["Claude · 5-hour", "Codex · 2h"] {
        let index = rows.iter().position(|row| row.starts_with(label)).unwrap();
        assert!(rows[index].contains("until reset"), "{rows:?}");
    }
}

#[test]
fn quota_forecasts_do_not_predict_from_unusable_or_unconfirmed_data() {
    let fixture = Fixture::new();
    let reset = chrono::Utc::now() + chrono::Duration::hours(3);
    for (used, resets_at, failed) in [
        (json!(0), Some(reset), false),
        (json!(100), Some(reset), false),
        (json!(null), Some(reset), false),
        (json!(80), None, false),
        (json!(80), Some(reset + chrono::Duration::days(1)), false),
        (json!(80), Some(reset - chrono::Duration::days(1)), false),
        (json!(80), Some(reset), true),
    ] {
        fixture.write("run/claude-usage.json", &json!({"five_hour": {"utilization": used, "resets_at": resets_at.map(|stamp| stamp.to_rfc3339())}}).to_string());
        fixture.write("run/codex-usage.json", &json!({"rate_limit": {
            "primary_window": {"used_percent": used, "limit_window_seconds": 18000, "reset_at": resets_at.map(|stamp| stamp.timestamp())}
        }}).to_string());
        if failed {
            for provider in ["claude", "codex"] {
                fixture.write(
                    &format!("run/{provider}-usage-error.json"),
                    r#"{"error":"failed"}"#,
                );
            }
        }
        let app = fixture.app();
        assert!(
            app.panels[0]
                .lines
                .iter()
                .filter(|line| line.item.is_some())
                .all(|line| !line.texts().contains('~')
                    && !line.texts().contains("until reset")
                    && !line.texts().contains("may run out now")),
            "{:?}",
            quota_texts(&app)
        );
    }
}

#[test]
fn quota_forecasts_keep_the_sampled_pace_between_fetches() {
    let fixture = Fixture::new();
    let fetched = std::time::SystemTime::now() - Duration::from_secs(300);
    let reset = chrono::DateTime::<chrono::Utc>::from(fetched) + chrono::Duration::hours(3);
    let cache = fixture.write(
        "run/claude-usage.json",
        &json!({"five_hour": {"utilization": 80, "resets_at": reset.to_rfc3339()}}).to_string(),
    );
    fs::File::options()
        .write(true)
        .open(&cache)
        .unwrap()
        .set_modified(fetched)
        .unwrap();
    let rows = quota_texts(&fixture.app());
    assert!(rows[0].contains("~25m"), "{rows:?}");
    fs::File::options()
        .write(true)
        .open(&cache)
        .unwrap()
        .set_modified(fetched - Duration::from_secs(301))
        .unwrap();
    let rows = quota_texts(&fixture.app());
    assert!(!rows.iter().any(|row| row.contains('~')), "{rows:?}");
}

#[test]
fn quota_forecasts_show_days_for_a_weekly_limit_with_several_days_left() {
    let fixture = Fixture::new();
    let reset = chrono::Utc::now() + chrono::Duration::days(5);
    fixture.write(
        "run/claude-usage.json",
        &json!({"seven_day": {"utilization": 40, "resets_at": reset.to_rfc3339()}}).to_string(),
    );
    let rows = quota_texts(&fixture.app());
    assert!(rows[0].contains("~3d 0h"), "{rows:?}");
}

#[test]
fn quota_forecasts_do_not_suggest_remaining_time_when_codex_is_blocked() {
    let fixture = Fixture::new();
    let reset = chrono::Utc::now() + chrono::Duration::hours(3);
    fixture.write("run/codex-usage.json", &json!({"rate_limit": {"allowed": false,
        "primary_window": {"used_percent": 80, "limit_window_seconds": 18000, "reset_at": reset.timestamp()}
    }}).to_string());
    let rows = quota_texts(&fixture.app());
    assert!(
        !rows
            .iter()
            .any(|row| row.contains('~') || row.contains("until reset")),
        "{rows:?}"
    );
}

#[test]
fn quota_rows_show_both_providers_with_separate_windows_and_remaining_capacity() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":28},"seven_day":{"utilization":86}}"#,
    );
    fixture.write("run/codex-usage.json", r#"{"rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000},"secondary_window":{"used_percent":100,"limit_window_seconds":604800}}}"#);
    let rows = quota_texts(&fixture.app());
    for (label, summary) in [
        ("Claude · 5-hour", "72%"),
        ("Claude · Weekly", "14% Low"),
        ("Codex · 5-hour", "95%"),
        ("Codex · Weekly", "0% Exhausted"),
    ] {
        assert!(
            rows.iter()
                .any(|row| row.contains(label) && row.contains(summary)),
            "{rows:?}"
        );
    }
}

#[test]
fn quota_rows_distinguish_available_low_exhausted_and_invalid_percentages() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (used, summary) in [
        (json!(0), "100%"),
        (json!(79), "21%"),
        (json!(80), "20% Low"),
        (json!(99), "1% Low"),
        (json!(100), "0% Exhausted"),
        (json!(null), "Unknown"),
        (json!(-0.1), "Unknown"),
        (json!(100.1), "Unknown"),
        (json!("28"), "Unknown"),
    ] {
        fixture.write(
            "run/claude-usage.json",
            &json!({"five_hour": {"utilization": used}}).to_string(),
        );
        fixture.write("run/codex-usage.json", &json!({"rate_limit": {"primary_window": {"used_percent": used, "limit_window_seconds": 18000}}}).to_string());
        app.build();
        let rows = quota_texts(&app);
        for provider in ["Claude · 5-hour", "Codex · 5-hour"] {
            let row = rows
                .iter()
                .find(|row| row.starts_with(provider))
                .unwrap_or_else(|| panic!("{rows:?}"));
            assert!(row.contains(summary), "{used}: {row}");
            if summary == "Unknown" {
                assert!(!row.contains("left"), "{row}");
            }
        }
    }
}

#[test]
fn quota_colors_follow_remaining_capacity_for_both_providers() {
    use crate::app::{GREEN, RED, YELLOW};
    use ratatui::style::Color;

    let fixture = Fixture::new();
    for (remaining, color) in [
        (100, GREEN),
        (51, GREEN),
        (50, YELLOW),
        (21, YELLOW),
        (20, RED),
        (1, RED),
        (0, RED),
    ] {
        fixture.write(
            "run/claude-usage.json",
            &json!({"five_hour": {"utilization": 100 - remaining}}).to_string(),
        );
        fixture.write("run/codex-usage.json", &json!({"rate_limit": {"primary_window": {"used_percent": 100 - remaining, "limit_window_seconds": 18000}}}).to_string());
        let app = fixture.app();
        for row in app.panels[0]
            .lines
            .iter()
            .filter(|line| line.item.is_some_and(|index| index < 2))
        {
            assert_eq!(
                row.segments[1].2.fg,
                Some(Color::Indexed(color)),
                "{}: {remaining}% left",
                row.segments[0].1
            );
        }
    }
}

#[test]
fn quota_rows_keep_missing_providers_visible_as_unknown() {
    let fixture = Fixture::new();
    let rows = quota_texts(&fixture.app());
    for provider in ["Claude", "Codex"] {
        let row = rows
            .iter()
            .find(|row| row.starts_with(provider) && row.contains("Unknown"))
            .unwrap_or_else(|| panic!("{rows:?}"));
        assert!(!row.contains("left"));
    }
}

#[test]
fn quota_rows_never_claim_fresh_capacity_from_old_or_failed_fetches() {
    let fixture = Fixture::new();
    for provider in ["claude", "codex"] {
        let payload = if provider == "claude" {
            json!({"five_hour": {"utilization": 28}})
        } else {
            json!({"rate_limit": {"primary_window": {"used_percent": 28, "limit_window_seconds": 18000}}})
        };
        let relative = format!("run/{provider}-usage.json");
        let cache = fixture.write(&relative, &payload.to_string());
        let old = std::time::SystemTime::now() - Duration::from_secs(601);
        fs::File::options()
            .write(true)
            .open(&cache)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let app = fixture.app();
        let row = quota_texts(&app)
            .into_iter()
            .find(|row| row.to_lowercase().starts_with(provider))
            .unwrap();
        assert!(row.contains("Stale · last 72%"), "{row}");
        fixture.write(&relative, &payload.to_string());
        fixture.write(
            &format!("run/{provider}-usage-error.json"),
            r#"{"error":"request failed"}"#,
        );
        let app = fixture.app();
        let row = quota_texts(&app)
            .into_iter()
            .find(|row| row.to_lowercase().starts_with(provider))
            .unwrap();
        assert!(row.contains("Stale · last 72%"), "{row}");
    }
}

#[test]
fn quota_rows_wait_for_confirmation_after_the_reset_time_passes() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":100,"resets_at":"2020-01-01T00:00:00Z"}}"#,
    );
    fixture.write("run/codex-usage.json", r#"{"rate_limit":{"primary_window":{"used_percent":100,"limit_window_seconds":18000,"reset_at":1577836800}}}"#);
    let app = fixture.app();
    let rows = quota_texts(&app);
    for provider in ["Claude · 5-hour", "Codex · 5-hour"] {
        let row = rows
            .iter()
            .find(|row| row.starts_with(provider))
            .unwrap_or_else(|| panic!("{rows:?}"));
        assert!(row.contains("Stale · last 0%"), "{row}");
        assert!(!row.contains("Available"));
    }
}

#[test]
fn quota_rows_respect_codex_blocking_flags_even_with_capacity_reported() {
    let fixture = Fixture::new();
    for flags in [json!({"allowed": false}), json!({"limit_reached": true})] {
        let mut limits = flags;
        limits["primary_window"] = json!({"used_percent": 10, "limit_window_seconds": 18000});
        fixture.write(
            "run/codex-usage.json",
            &json!({"rate_limit": limits}).to_string(),
        );
        let rows = quota_texts(&fixture.app());
        let row = rows
            .iter()
            .find(|row| row.starts_with("Codex · 5-hour"))
            .unwrap_or_else(|| panic!("{rows:?}"));
        assert!(row.contains("Blocked"), "{row}");
        assert!(!row.contains("Available"), "{row}");
    }
}

#[test]
fn quota_rows_keep_scoped_claude_limits_and_actual_codex_durations() {
    let fixture = Fixture::new();
    fixture.write("run/claude-usage.json", r#"{"limits":[{"kind":"session","percent":28},{"kind":"weekly_scoped","percent":86,"scope":{"model":{"display_name":"Sonnet"}}}]}"#);
    fixture.write("run/codex-usage.json", r#"{"rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":3600},"secondary_window":{"used_percent":20,"limit_window_seconds":86400}}}"#);
    let rows = quota_texts(&fixture.app());
    for label in [
        "Claude · 5-hour",
        "Claude · Sonnet weekly",
        "Codex · 1h",
        "Codex · 1d",
    ] {
        assert!(rows.iter().any(|row| row.starts_with(label)), "{rows:?}");
    }
}

#[test]
fn quota_rows_keep_status_and_capacity_readable_in_narrow_terminals() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"seven_day":{"utilization":0,"resets_at":"2100-01-01T00:00:00Z"}}"#,
    );
    fixture.write("run/codex-usage.json", r#"{"rate_limit":{"primary_window":{"used_percent":86,"limit_window_seconds":18000,"reset_at":4102444800}}}"#);
    let mut app = fixture.app();
    app.focus_panel(0);
    for width in [40, 60] {
        app.width = width;
        app.redraw();
        let buffer = render(&mut app, width, 45);
        let rows: Vec<_> = (0..45)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();
        for (label, summary) in [("Weekly", "100%"), ("5-hour", "14% Low")] {
            assert!(
                rows.iter()
                    .any(|row| row.contains(label) && row.contains(summary)),
                "width {width}: {rows:?}"
            );
        }
    }
}

#[test]
fn quota_redraw_does_not_read_new_data_until_refresh() {
    let fixture = Fixture::new();
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":28}}"#,
    );
    let mut app = fixture.app();
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":100}}"#,
    );
    app.redraw();
    assert!(quota_texts(&app)[0].contains("72%"));
    app.build();
    assert!(quota_texts(&app)[0].contains("0% Exhausted"));
}

#[test]
fn quota_polling_refreshes_both_providers_and_forwards_forced_refresh() {
    let fixture = Fixture::new();
    for provider in ["claude", "codex"] {
        let script = fixture.write(
            &format!("home/.config/waybar/scripts/{provider}-usage.sh"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"{provider}-quota $*\" >> '{}'\n",
                fixture.path("calls.log").display()
            ),
        );
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut refresher = usage::Refresher::new();
    refresher.tick(false);
    assert_eq!(
        fixture.wait_for_calls("claude-quota", 1),
        vec!["claude-quota five_hour"]
    );
    assert_eq!(
        fixture.wait_for_calls("codex-quota", 1),
        vec!["codex-quota "]
    );
    refresher.tick(false);
    assert_eq!(fixture.calls("codex-quota").len(), 1);
    refresher.tick(true);
    assert_eq!(
        fixture.wait_for_calls("claude-quota", 2)[1],
        "claude-quota five_hour --force"
    );
    assert_eq!(
        fixture.wait_for_calls("codex-quota", 2)[1],
        "codex-quota --force"
    );
}

#[test]
fn quota_scripts_are_reaped_once_they_end() {
    let fixture = Fixture::new();
    for provider in ["claude", "codex"] {
        let script = fixture.write(
            &format!("home/.config/waybar/scripts/{provider}-usage.sh"),
            &format!(
                "#!/bin/sh\necho $$ > '{}'\n",
                fixture.path(&format!("{provider}.pid")).display()
            ),
        );
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    usage::Refresher::new().tick(true);
    for provider in ["claude", "codex"] {
        assert!(
            is_reaped(pid_written_to(&fixture, &format!("{provider}.pid"))),
            "{provider}-usage.sh stays a zombie"
        );
    }
}

#[test]
fn the_open_tab_takes_the_whole_column_under_the_header() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    assert_eq!(app.layout(45), vec![0, 43, 0, 0, 0, 0, 0]);
    app.focus_panel(3);
    assert_eq!(app.layout(45), vec![0, 0, 0, 43, 0, 0, 0]);
    app.focus_panel(4);
    assert_eq!(app.layout(45), vec![0, 0, 0, 0, 43, 0, 0]);
    app.focus_panel(5);
    assert_eq!(app.layout(45), vec![0, 0, 0, 0, 0, 43, 0]);
    app.focus_panel(6);
    assert_eq!(app.layout(45), vec![0, 0, 0, 0, 0, 0, 43]);
}

#[test]
fn status_shares_the_column_with_its_pull_requests() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.reviews.set_rows(
        (1..=20)
            .map(|number| Review {
                number,
                title: "VAT".into(),
                repository: "shop".into(),
                ..Review::default()
            })
            .collect(),
    );
    app.build();
    app.focus_panel(0);
    let heights = app.layout(45);
    assert_eq!(heights[0] + heights[2], 43);
    assert_eq!((heights[1], heights[3], heights[4]), (0, 0, 0));
    assert!(heights[2] >= 4 && heights[2] <= 14, "{heights:?}");
    app.focus_panel(2);
    assert_eq!(app.layout(45), heights);
}

#[test]
fn an_empty_tab_shows_no_scroll_marker() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for _ in 0..2 {
        let buffer = render(&mut app, 60, 45);
        let markers: Vec<String> = (0..45)
            .map(|y| {
                (0..60)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .filter(|row| row.contains('▲') || row.contains('▼'))
            .collect();
        assert!(markers.is_empty(), "{markers:?}");
    }
}

#[test]
fn the_pull_requests_heading_counts_open_failing_and_waiting_pull_requests() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let review = |number, ci: &str, decision: &str| Review {
        number,
        title: "VAT".into(),
        repository: "shop".into(),
        ci: ci.into(),
        review: decision.into(),
        ..Review::default()
    };
    app.reviews.set_rows(vec![
        review(1, "failing", "CHANGES_REQUESTED"),
        review(2, "passing", "REVIEW_REQUIRED"),
        review(3, "failing", "REVIEW_REQUIRED"),
        review(4, "pending", "APPROVED"),
        review(5, "", ""),
    ]);
    app.build();
    let heading = status_rows(&mut app)
        .into_iter()
        .find(|row| row.contains("PULL REQUESTS"))
        .unwrap();
    assert!(
        heading.contains("PULL REQUESTS · 5 · 5 open ✗ 2 changes 1 review 2 · "),
        "{heading}"
    );
}

#[test]
fn the_empty_pull_requests_heading_shows_no_count() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let heading = status_rows(&mut app)
        .into_iter()
        .find(|row| row.contains("PULL REQUESTS"))
        .unwrap();
    assert!(!heading.contains("open"), "{heading}");
}

#[test]
fn the_tabs_count_what_needs_you_open_todos_and_ideas() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(
        SESSION_A,
        "needs-you",
        "runway",
        "/r/runway",
        1
    )]));
    let mut app = fixture.app();
    let proposed = app.todos.add("runway", "/r/runway", "cache the manifest");
    app.todos
        .items
        .iter_mut()
        .find(|item| item.id() == proposed)
        .unwrap()
        .0
        .insert("status".into(), json!("proposed"));
    let doing = app.todos.add("runway", "/r/runway", "add a healthcheck");
    app.todos.set_status(&doing, "doing", "");
    app.todos.add("runway", "/r/runway", "rotate the logs");
    let done = app.todos.add("runway", "/r/runway", "pin the image");
    app.todos.set_status(&done, "done", "");
    app.build();
    let buffer = render(&mut app, 80, 45);
    let tabs: String = (0..80).map(|x| buffer[(x, 1)].symbol()).collect();
    assert!(
        tabs.contains(" 1  Sessions  1 ")
            && tabs.contains(" 2  Todo  2 ")
            && tabs.contains(" 3  Ideas  1 "),
        "{tabs}"
    );
    let badge = tabs.find("Sessions").unwrap() + "Sessions ".len() + 1;
    assert_eq!(
        buffer[(badge as u16, 1)].bg,
        crate::app::color(crate::app::YELLOW),
        "only what needs you is amber"
    );
}

#[test]
fn both_provider_statuses_are_visible_before_their_first_fetch() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let lines = texts(&app, 0);
    assert!(lines.iter().any(|line| line.contains("Claudenot checked")));
    assert!(lines.iter().any(|line| line.contains("Codexnot checked")));
}

#[test]
fn codexs_status_shows_outage_details_independently_of_claude() {
    let fixture = Fixture::new();
    let claude = fixture.write(
        "claude-status.json",
        &json!({"status": {"indicator": "none", "description": "All Systems Operational"},
        "components": [], "incidents": []})
        .to_string(),
    );
    let codex = fixture.write("codex-status.json", &json!({"status": {"indicator": "major", "description": "Partial System Outage"},
        "components": [{"name": "Codex in ChatGPT Desktop", "status": "major_outage"}],
        "incidents": [{"name": "Codex requests failing", "status": "investigating"}, {"name": "Old incident", "status": "resolved"}]}).to_string());
    unsafe {
        std::env::set_var("CLAUDE_STATUS_URL", format!("file://{}", claude.display()));
        std::env::set_var("CODEX_STATUS_URL", format!("file://{}", codex.display()));
    }
    let mut app = fixture.app();
    app.claude_status.fetch_now();
    app.codex_status.fetch_now();
    app.build();
    let lines = texts(&app, 0);
    assert!(lines.contains(&"Claude● Operational".to_string()));
    assert!(lines.contains(&"Codex● Partial System Outage".to_string()));
    assert!(lines.contains(&"Codex in ChatGPT Desktop: major outage".to_string()));
    assert!(lines.contains(&"Codex requests failing".to_string()));
    assert!(!lines.iter().any(|line| line.contains("Old incident")));
}

#[test]
fn codexs_status_is_operational_when_its_official_feed_is_healthy() {
    let fixture = Fixture::new();
    let summary = fixture.write(
        "codex-status.json",
        &json!({"status": {"indicator": "none", "description": "All Systems Operational"},
        "components": [], "incidents": []})
        .to_string(),
    );
    unsafe { std::env::set_var("CODEX_STATUS_URL", format!("file://{}", summary.display())) };
    let mut app = fixture.app();
    app.tick();
    for _ in 0..100 {
        if app.codex_status.snapshot().0.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    app.build();
    assert!(
        texts(&app, 0)
            .iter()
            .any(|line| line.contains("Codex● Operational"))
    );
}

#[test]
fn an_unreachable_codex_status_feed_does_not_report_a_healthy_service() {
    let fixture = Fixture::new();
    unsafe {
        std::env::set_var(
            "CODEX_STATUS_URL",
            format!("file://{}", fixture.path("missing.json").display()),
        )
    };
    let mut app = fixture.app();
    app.codex_status.fetch_now();
    app.build();
    assert!(
        texts(&app, 0)
            .iter()
            .any(|line| line.starts_with("Codexstatus.openai.com unreachable"))
    );
    assert!(
        !fs::read_to_string(fixture.path("state/cockpit/log"))
            .unwrap_or_default()
            .contains(" ERROR ")
    );
}

#[test]
fn the_sessions_panel_warns_from_85_percent_disk_use_and_points_to_make_disk() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (disk, is_warned) in [
        (Some(("/", 84.0)), false),
        (Some(("/", 85.0)), true),
        (Some(("/home", 96.0)), true),
        (None, false),
    ] {
        app.disk = disk.map(|(mount, used)| (mount.to_string(), used));
        app.disk_checked = util::now();
        app.build();
        let lines = texts(&app, 1);
        let expected = disk
            .map(|(mount, used)| format!("▲Disk {used:.0}% full on {mount}"))
            .unwrap_or_default();
        assert_eq!(lines.first() == Some(&expected), is_warned, "{lines:?}");
        app.build();
        assert_eq!(
            texts(&app, 0)
                .iter()
                .any(|line| line == "make -C ~/dotfiles disk shows what uses it"),
            is_warned,
            "Status lists the notice's detail"
        );
    }
}

#[test]
fn the_disk_fill_is_read_again_once_a_minute_has_passed() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.disk_usage = || Some(("/".into(), 90.0));
    app.build();
    assert!(!texts(&app, 1).iter().any(|line| line.starts_with("⚠ Disk")));
    app.disk_checked -= 61.0;
    app.build();
    assert_eq!(
        texts(&app, 1).first().map(String::as_str),
        Some("▲Disk 90% full on /")
    );
}

#[test]
fn claudes_status_shows_in_status_and_warns_in_sessions_only_when_degraded() {
    let fixture = Fixture::new();
    let summary = fixture.write("status.json", &json!({"status": {"indicator": "minor", "description": "Partially Degraded Service"},
        "components": [{"name": "Claude Code", "status": "partial_outage"}], "incidents": [{"name": "Elevated errors", "status": "investigating"}]}).to_string());
    unsafe { std::env::set_var("CLAUDE_STATUS_URL", format!("file://{}", summary.display())) };
    let mut app = fixture.app();
    app.claude_status.fetch_now();
    app.build();
    assert!(texts(&app, 0).contains(&"Claude● Partially Degraded Service".to_string()));
    assert_eq!(
        &texts(&app, 1)[..2],
        &[
            "!Claude: Partially Degraded Service".to_string(),
            String::new()
        ]
    );
    app.build();
    assert!(
        texts(&app, 0).contains(&"Claude Code: partial outage · Elevated errors".to_string()),
        "Status lists the incident: {:?}",
        texts(&app, 0)
    );
    fs::write(
        &summary,
        json!({"status": {"indicator": "none", "description": "All Systems Operational"}})
            .to_string(),
    )
    .unwrap();
    let mut calm = App::new();
    calm.claude_status.fetch_now();
    calm.build();
    assert!(
        !texts(&calm, 1)
            .iter()
            .any(|line| line.starts_with("!Claude"))
    );
}

#[test]
fn an_unreachable_status_page_says_so_without_an_error_hand_off() {
    let fixture = Fixture::new();
    unsafe {
        std::env::set_var(
            "CLAUDE_STATUS_URL",
            format!("file://{}", fixture.path("missing.json").display()),
        )
    };
    let mut app = fixture.app();
    app.claude_status.fetch_now();
    app.build();
    assert!(
        texts(&app, 0)
            .iter()
            .any(|line| line.contains("status.claude.com unreachable"))
    );
    assert!(
        !fs::read_to_string(fixture.path("state/cockpit/log"))
            .unwrap_or_default()
            .contains(" ERROR ")
    );
}

#[test]
fn budget_alerts_fire_once_per_threshold_and_window() {
    static SENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn send(notice: &Value) -> bool {
        SENT.lock()
            .unwrap()
            .push(notice["id"].as_str().unwrap().to_string());
        true
    }
    let _fixture = Fixture::new();
    SENT.lock().unwrap().clear();
    let window = |percentage| usage::Window {
        key: "seven_day".into(),
        label: "This week".into(),
        percentage: Some(percentage),
        resets: String::new(),
        resets_at: Some("2026-10-20T00:00:00Z".into()),
        note: String::new(),
    };
    let mut alerts = BudgetAlerts::new(send);
    alerts.check(&[window(85)]);
    alerts.check(&[window(86)]);
    BudgetAlerts::new(send).check(&[window(96)]);
    assert_eq!(
        *SENT.lock().unwrap(),
        vec!["claude-usage-80".to_string(), "claude-usage-95".to_string()]
    );
}

#[test]
fn budget_alerts_ignore_sub_second_jitter_in_the_reset_time() {
    static SENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn send(notice: &Value) -> bool {
        SENT.lock()
            .unwrap()
            .push(notice["id"].as_str().unwrap().to_string());
        true
    }
    let _fixture = Fixture::new();
    SENT.lock().unwrap().clear();
    let window = |resets_at: &str| usage::Window {
        key: "weekly_all".into(),
        label: "This week".into(),
        percentage: Some(82),
        resets: String::new(),
        resets_at: Some(resets_at.into()),
        note: String::new(),
    };
    let mut alerts = BudgetAlerts::new(send);
    alerts.check(&[window("2026-10-23T23:59:59.630773+00:00")]);
    alerts.check(&[window("2026-10-23T23:59:59.912004+00:00")]);
    BudgetAlerts::new(send).check(&[window("2026-10-24T00:00:00.104311+00:00")]);
    assert_eq!(*SENT.lock().unwrap(), vec!["claude-usage-80".to_string()]);
}

#[test]
fn a_reply_prices_every_token_kind() {
    let usage = json!({"input_tokens": 1_000_000, "output_tokens": 1_000_000, "cache_read_input_tokens": 1_000_000,
                       "cache_creation_input_tokens": 2_000_000, "cache_creation": {"ephemeral_1h_input_tokens": 1_000_000}});
    assert_eq!(
        costs::reply_cost("claude-opus-5-5", &usage),
        Some(4.0 + 20.0 + 0.2 + 5.0 + 8.0)
    );
}

#[test]
fn haiku_5_5_switches_tier_above_100k_prompt_tokens() {
    let short = json!({"input_tokens": 50_000, "output_tokens": 1_000_000});
    let long = json!({"input_tokens": 150_000, "output_tokens": 1_000_000});
    assert_eq!(
        costs::reply_cost("claude-haiku-5-5", &short).map(|c| (c * 1000.0).round()),
        Some(505.0)
    );
    assert_eq!(
        costs::reply_cost("claude-haiku-5-5", &long).map(|c| (c * 1000.0).round()),
        Some(2575.0)
    );
}

#[test]
fn the_archive_keeps_replies_after_their_transcript_is_deleted() {
    let fixture = Fixture::new();
    let reply = r#"{"timestamp":"2026-10-18T10:00:00Z","requestId":"r","sessionId":"s","cwd":"/x/shop","message":{"id":"m","model":"claude-opus-5-5","usage":{"output_tokens":1000000}}}"#;
    let transcript = fixture.write("home/.claude/projects/p/s.jsonl", &format!("{reply}\n"));
    let mut archive = costs::open_archive(&fixture.path("state/usage.sqlite")).unwrap();
    costs::archive_transcripts(&mut archive, &fixture.path("home/.claude/projects")).unwrap();
    fs::remove_file(transcript).unwrap();
    costs::archive_transcripts(&mut archive, &fixture.path("home/.claude/projects")).unwrap();
    assert_eq!(costs::archived_costs(&archive).unwrap().2, 20.0);
}

#[test]
fn the_digest_covers_the_last_day_today_the_waiting_sessions_and_pull_requests() {
    let fixture = Fixture::new();
    let lines: Vec<String> = (0..4).map(|i| json!({"timestamp": format!("2026-10-17T10:0{i}:00Z"), "requestId": format!("r{i}"), "sessionId": "s1",
        "cwd": "/x/shop", "message": {"id": format!("m{i}"), "model": "claude-opus-5-5", "usage": {"output_tokens": 100000}}}).to_string()).collect();
    fixture.write(
        "home/.claude/projects/p/s1.jsonl",
        &(lines.join("\n") + "\n"),
    );
    let mut archive = costs::open_archive(&fixture.path("state/usage.sqlite")).unwrap();
    costs::archive_transcripts(&mut archive, &fixture.path("home/.claude/projects")).unwrap();
    let waiting = sessions::Session {
        session: "w".into(),
        state: "needs-you".into(),
        project: "runway".into(),
        task: "Deploy".into(),
        ..Default::default()
    };
    let review = reviews::Review {
        repository: "shop".into(),
        number: 7,
        title: "Fix".into(),
        ci: "failing".into(),
        ..Default::default()
    };
    let digest = costs::digest_lines(
        &archive,
        &[waiting],
        &[review],
        chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
        &costs::ChiefSection::Unavailable("test".into()),
    )
    .unwrap()
    .join("\n");
    let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
        .unwrap()
        .replace_all(&digest, "")
        .to_string();
    assert!(
        plain.contains("Yesterday  $8.00 at API prices · 1 session"),
        "{plain}"
    );
    assert!(plain.contains("! runway"));
    assert!(plain.contains("✗ shop#7  Fix"));
}

#[test]
fn the_digest_shows_the_weeks_share_spent_in_flagged_sessions() {
    let fixture = Fixture::new();
    let reply = |session: &str, context: i64| {
        json!({"timestamp": "2026-10-17T10:00:00Z", "requestId": session, "sessionId": session, "cwd": "/x/shop",
        "message": {"id": session, "model": "claude-opus-5-5", "usage": {"output_tokens": 100000, "cache_read_input_tokens": context}}}).to_string()
    };
    fixture.write(
        "home/.claude/projects/p/s.jsonl",
        &format!("{}\n{}\n", reply("bloated", 500_000), reply("lean", 10_000)),
    );
    let mut archive = costs::open_archive(&fixture.path("state/usage.sqlite")).unwrap();
    costs::archive_transcripts(&mut archive, &fixture.path("home/.claude/projects")).unwrap();
    let digest = costs::digest_lines(
        &archive,
        &[],
        &[],
        chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
        &costs::ChiefSection::Unavailable("test".into()),
    )
    .unwrap()
    .join("\n");
    let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
        .unwrap()
        .replace_all(&digest, "")
        .to_string();
    assert!(
        plain.contains("Last 7 days  $4.10 · 51% in flagged sessions (the 7 days before: –)"),
        "{plain}"
    );
    assert_eq!(plain.matches("large context").count(), 1, "{plain}");
}

#[test]
fn the_digest_lists_chief_answers_since_the_last_digest() {
    let named = sessions::Session {
        session: "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L".into(),
        label: "Fix login".into(),
        project: "shop".into(),
        ..Default::default()
    };
    let digest = crate::chief::store::ChiefDigest {
        answered: 3,
        escalated: 1,
        overruled: 2,
        more: 0,
        habits: vec![],
        new_habits: 0,
        lines: vec![
            digest_answer(
                "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L",
                "Which option?",
                "Use option B.",
            ),
            digest_answer(
                "ses_unknown0000000000abcdef",
                "Keep the flag?",
                "Yes, keep it.",
            ),
        ],
    };
    let plain = digest_with_chief(
        &[named],
        &costs::ChiefSection::Answers {
            since_ms: local_ms(18, 8, 12),
            digest,
        },
    );
    assert!(
        plain.contains("Chief answered 3, escalated 1, overruled 2 since 08:12"),
        "{plain}"
    );
    assert!(
        plain.contains("    Fix login · shop · Which option? → Use option B."),
        "{plain}"
    );
    assert!(
        plain.contains("    ses_…abcdef · Keep the flag? → Yes, keep it."),
        "{plain}"
    );
    assert!(!plain.contains("more"), "{plain}");
}

#[test]
fn the_digest_names_yesterday_and_older_start_times() {
    let empty = || crate::chief::store::ChiefDigest {
        answered: 0,
        escalated: 0,
        overruled: 0,
        more: 0,
        habits: vec![],
        new_habits: 0,
        lines: vec![],
    };
    let yesterday = digest_with_chief(
        &[],
        &costs::ChiefSection::Answers {
            since_ms: local_ms(17, 22, 30),
            digest: empty(),
        },
    );
    assert!(
        yesterday.contains("Chief: nothing answered since yesterday 22:30"),
        "{yesterday}"
    );
    let older = digest_with_chief(
        &[],
        &costs::ChiefSection::Answers {
            since_ms: local_ms(14, 9, 5),
            digest: empty(),
        },
    );
    assert!(
        older.contains("Chief: nothing answered since Wed 14 Oct 09:05"),
        "{older}"
    );
}

#[test]
fn the_digest_says_when_the_chief_answered_nothing() {
    let digest = crate::chief::store::ChiefDigest {
        answered: 0,
        escalated: 0,
        overruled: 0,
        more: 0,
        habits: vec![],
        new_habits: 0,
        lines: vec![],
    };
    let plain = digest_with_chief(
        &[],
        &costs::ChiefSection::Answers {
            since_ms: local_ms(18, 8, 12),
            digest,
        },
    );
    assert!(
        plain.contains("Chief: nothing answered since 08:12"),
        "{plain}"
    );
    assert!(!plain.contains("Chief answered"), "{plain}");
}

#[test]
fn the_digest_leaves_the_rest_intact_when_the_chief_is_unavailable() {
    let plain = digest_with_chief(
        &[],
        &costs::ChiefSection::Unavailable("no team database yet".into()),
    );
    assert!(
        plain.contains("Chief: unavailable (no team database yet)"),
        "{plain}"
    );
    assert!(plain.contains("Today so far"), "{plain}");
}

#[test]
fn the_digest_shows_at_most_five_answers_then_a_count() {
    let lines = (0..5)
        .map(|index| digest_answer("ses_abcdefghijkl", &format!("Question {index}?"), "Yes."))
        .collect();
    let digest = crate::chief::store::ChiefDigest {
        answered: 12,
        escalated: 0,
        overruled: 0,
        more: 7,
        habits: vec![],
        new_habits: 0,
        lines,
    };
    let plain = digest_with_chief(
        &[],
        &costs::ChiefSection::Answers {
            since_ms: local_ms(18, 8, 12),
            digest,
        },
    );
    assert_eq!(plain.matches("→ Yes.").count(), 5, "{plain}");
    assert!(plain.contains("    +7 more"), "{plain}");
}

#[test]
fn a_chief_answer_in_the_digest_is_escaped_and_cut_to_one_line() {
    let hostile = digest_answer(
        "ses_abcdefghijkl",
        &format!("Line one\nline two \x1b[2J {}", "q".repeat(500)),
        &"a".repeat(2000),
    );
    let digest = crate::chief::store::ChiefDigest {
        answered: 1,
        escalated: 0,
        overruled: 0,
        more: 0,
        habits: vec![],
        new_habits: 0,
        lines: vec![hostile],
    };
    let plain = digest_with_chief(
        &[],
        &costs::ChiefSection::Answers {
            since_ms: local_ms(18, 8, 12),
            digest,
        },
    );
    let line = plain
        .lines()
        .find(|line| line.contains("Line one"))
        .unwrap();
    assert!(line.contains("Line one line two \\u001b[2J"), "{line}");
    assert!(line.ends_with('…') && line.chars().count() <= 100, "{line}");
    assert!(!plain.contains('\x1b'), "{plain}");
}

#[test]
fn the_digest_lists_new_habits() {
    let habits = vec![
        digest_habit(
            "Always run the tests first.",
            Some("path:/home/theo/IdeaProjects/shop"),
        ),
        digest_habit("Keep replies short.", None),
    ];
    for answered in [1, 0] {
        let plain = digest_with_habits(answered, habits.clone(), 2);
        let lines: Vec<&str> = plain.lines().collect();
        let header = lines
            .iter()
            .position(|line| *line == "  New habits: 2")
            .unwrap_or_else(|| panic!("{plain}"));
        assert_eq!(
            lines[header + 1..header + 3],
            [
                "    Always run the tests first. · shop",
                "    Keep replies short. · everywhere"
            ],
            "{plain}"
        );
        let answers = lines
            .iter()
            .position(|line| line.contains("Chief"))
            .unwrap();
        assert!(
            answers < header,
            "the habits follow the Chief's answers: {plain}"
        );
    }
}

#[test]
fn the_digest_shows_at_most_three_habits() {
    let habits = (0..3)
        .map(|index| digest_habit(&format!("Habit {index}."), None))
        .collect();
    let plain = digest_with_habits(1, habits, 7);
    assert!(plain.contains("  New habits: 7"), "{plain}");
    assert_eq!(plain.matches("· everywhere").count(), 3, "{plain}");
}

#[test]
fn a_digest_without_new_habits_has_no_habit_lines() {
    let plain = digest_with_habits(1, vec![], 0);
    assert!(!plain.contains("New habits"), "{plain}");
    assert!(!plain.contains("everywhere"), "{plain}");
}

#[test]
fn a_habit_in_the_digest_is_escaped_and_cut_to_one_line() {
    let hostile = digest_habit(
        &format!("Line one\nline two \x1b[2J {}", "h".repeat(600)),
        Some("path:/home/theo/IdeaProjects/shop"),
    );
    let plain = digest_with_habits(0, vec![hostile], 1);
    let line = plain
        .lines()
        .find(|line| line.contains("Line one"))
        .unwrap();
    assert!(line.contains("Line one line two \\u001b[2J"), "{line}");
    assert!(
        line.ends_with("… · shop") && line.chars().count() <= 100,
        "{line}"
    );
    assert!(!plain.contains('\x1b'), "{plain}");
}

#[test]
fn the_clone_review_is_in_status_utilities() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    select_status_action(&mut app, "Clones review");
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(fixture.wait_for_calls("cockpit", 1), ["cockpit clones"]);
}

#[test]
fn before_the_first_usage_fetch_the_status_labels_stay_whole() {
    // A missing quota fetch must not collapse the service label column.
    let fixture = Fixture::new();
    let summary = fixture.write(
        "status.json",
        &json!({"status": {"indicator": "none", "description": "All Systems Operational"}})
            .to_string(),
    );
    unsafe { std::env::set_var("CLAUDE_STATUS_URL", format!("file://{}", summary.display())) };
    let mut app = fixture.app();
    app.claude_status.fetch_now();
    app.build();
    app.focus_panel(0);
    let buffer = render(&mut app, 60, 30);
    let rows: Vec<String> = (0..30)
        .map(|y| {
            (0..60)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    assert!(
        rows.iter()
            .any(|row| row.contains("Claude") && row.contains("● Operational")),
        "{rows:#?}"
    );
}

#[test]
fn time_spent_counts_the_gaps_between_replies_under_five_minutes_per_session() {
    let fixture = Fixture::new();
    // 60 + 60 + 60 s, then an hour away (not counted), then 30 s; a second session's 90 s apart.
    replies_at(&fixture, "s1", "/x/shop", &[0, 60, 120, 180, 3780, 3810]);
    replies_at(&fixture, "s2", "/x/shop", &[30, 120]);
    let spent = costs::active_seconds(&archived(&fixture), "2026-10-18").unwrap();
    assert_eq!(
        spent.get("shop").map(|seconds| seconds.round()),
        Some(300.0)
    );
}

#[test]
fn a_parallel_clones_time_counts_for_its_repository() {
    // The card names a clone's session after its repository: so does the archive.
    let fixture = Fixture::new();
    let clone = crate::paths::clones_dir().join("dotfiles/todo-0123abcd/apps");
    replies_at(&fixture, "s1", &clone.display().to_string(), &[0, 60]);
    let spent = costs::active_seconds(&archived(&fixture), "2026-10-18").unwrap();
    assert_eq!(spent.keys().collect::<Vec<_>>(), ["dotfiles"]);
}

#[test]
fn an_archive_from_before_version_2_reads_every_transcript_again() {
    let fixture = Fixture::new();
    replies_at(&fixture, "s1", "/x/shop", &[0, 60]);
    {
        let archive = archived(&fixture);
        archive
            .execute_batch("update replies set project = 'old-name'; pragma user_version = 1;")
            .unwrap();
    }
    let spent = costs::active_seconds(&archived(&fixture), "2026-10-18").unwrap();
    assert_eq!(spent.keys().collect::<Vec<_>>(), ["shop"]);
}

#[test]
fn time_spent_reads_in_minutes_then_hours() {
    for ((today, all_time), expected) in [
        ((0.0, 30.0), None),
        ((0.0, 2040.0), Some("⏱ 34m")),
        ((4800.0, 122_400.0), Some("⏱ 1h20 today · 34h")),
        ((0.0, 11_100.0), Some("⏱ 3h05")),
    ] {
        assert_eq!(
            App::time_spent_text((today, all_time)).as_deref(),
            expected,
            "{today} {all_time}"
        );
    }
}

#[test]
fn time_with_claude_counts_overlapping_sessions_once() {
    // Two sessions working through the same two minutes are two minutes with Claude, not three and a half.
    let fixture = Fixture::new();
    replies_at(&fixture, "s1", "/x/shop", &[0, 60, 120]);
    replies_at(&fixture, "s2", "/x/runway", &[30, 90]);
    let start = chrono::NaiveDate::from_ymd_opt(2026, 10, 18)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp() as f64;
    let together = costs::time_with_claude(&archived(&fixture), &[start, start + 60.0]).unwrap();
    assert_eq!(
        together
            .iter()
            .map(|seconds| seconds.round())
            .collect::<Vec<_>>(),
        [120.0, 60.0]
    );
}

#[test]
fn status_refresh_does_not_turn_a_quota_selection_into_an_action() {
    let fixture = Fixture::new();
    fixture.write("run/claude-usage.json", r#"{"limits":[{"kind":"session","percent":0},{"kind":"weekly_all","percent":0},{"kind":"weekly_scoped","percent":0,"scope":{"model":{"display_name":"Extra"}}}]}"#);
    let mut app = fixture.app();
    app.focus_panel(0);
    for _ in 0..3 {
        app.handle(Key::Down, &mut FakeScreen::default());
    }
    fs::remove_file(fixture.path("run/claude-usage.json")).unwrap();
    app.build();
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(app.message, "Status selection changed: choose a row");
    assert!(fixture.calls("cockpit").is_empty() && fixture.calls("xdg-open").is_empty());
    select_status_action(&mut app, "Clones review");
    app.handle(Key::Up, &mut FakeScreen::default());
    app.handle(Key::Down, &mut FakeScreen::default());
    fixture.write(
        "run/claude-usage.json",
        r#"{"five_hour":{"utilization":0},"seven_day":{"utilization":0}}"#,
    );
    app.build();
    app.handle(Key::Enter, &mut FakeScreen::default());
    assert_eq!(fixture.wait_for_calls("cockpit", 1), ["cockpit clones"]);
}
