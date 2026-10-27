//! The 4 Teams and 5 Chief tabs (DESIGN.md "Round 4: Teams and Chief tabs", frames 3409, 3670,
//! 3737, 3811, 3973). The Chief tab's own tests are in tests/chief_ui.rs, beside its host fixture.
use super::*;

const TASK: &str = "a5dba5b2";
use crate::app::TEAMS_PANEL;

fn now_ms() -> i64 { util::now_secs() * 1000 }

/// The Session list redesign team: Designer and Developer live, QA planned, a pending link request,
/// a handoff to QA queued and a handoff today.
fn team_state() -> Value {
    let mut state = sidebar_snapshot();
    for member in state["members"].as_array_mut().unwrap() { member["task_id"] = json!(TASK); }
    state["qa_plans"] = json!([{"project": SIDEBAR_PROJECT, "task_id": TASK, "agent": "claude"}]);
    state["handoffs"] = json!([{"project": SIDEBAR_PROJECT, "task_id": TASK, "handoff_id": "h1", "status": "current", "developer": OPENCODE_SESSION,
        "created_at": now_ms() - 60_000, "payload": {"commit": "34c1876aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}]);
    let envelope = json!({"task": {"project": SIDEBAR_PROJECT, "task_id": TASK}, "kind": "qa_request", "handoff": {"commit": "34c1876aaaa"},
        "sender": {"session_id": OPENCODE_SESSION, "role": "developer"}, "recipient": {"session_id": "qa-not-started", "role": "qa"}});
    state["messages"] = json!([{"seq": 1, "message_id": "m1", "envelope": envelope.to_string(), "state": "queued", "attempts": 0, "next_at": 0, "error": "",
        "created_at": now_ms() - 720_000}]);
    state["link_proposals"] = json!([{"proposal_id": "lp1", "sender": SESSION_A, "purpose": "To share the game-test manifest format.", "status": "pending",
        "revision": 1, "mapping_hash": "h", "task": {"project": SIDEBAR_PROJECT, "task_id": TASK},
        "participants": [{"session_id": SESSION_A, "project": SIDEBAR_PROJECT}, {"session_id": "peer", "project": "path:/r/stray-coop"}]}]);
    state["links"] = json!([]);
    state
}

fn todo_item(id: &str, text: &str, status: &str) -> Item {
    Item(json!({"id": id, "project": "project-a", "text": text, "cwd": "/r/project-a", "status": status, "updated": util::now_secs()}).as_object().unwrap().clone())
}

fn team_app(fixture: &Fixture, state: Value) -> (App, HostRequests) {
    sidebar_sessions(fixture);
    let requests = team_host(fixture, state, vec![]);
    let mut app = fixture.app();
    app.todos.items = vec![todo_item(TASK, "Session list redesign", "todo")];
    app.redraw();
    (app, requests)
}

fn press(app: &mut App, keys: &[Key]) {
    for key in keys {
        app.handle(key.clone(), &mut FakeScreen::default());
        app.redraw();
    }
}

fn tabs_row(app: &mut App, width: u16) -> String {
    buffer_rows(&render(app, width, 30))[1].clone()
}

#[test]
fn every_tab_shows_in_full_on_a_wide_column() {
    let fixture = Fixture::new();
    let (mut app, _) = team_app(&fixture, team_state());
    let tabs = tabs_row(&mut app, 100);
    for name in ["Sessions", "Todo", "Ideas", "Teams", "Chief"] { assert!(tabs.contains(name), "{name}: {tabs}"); }
}

#[test]
fn tabs_fold_from_the_right_and_the_open_tab_never_folds() {
    let fixture = Fixture::new();
    let (mut app, _) = team_app(&fixture, team_state());
    let tabs = tabs_row(&mut app, 50);
    assert!(tabs.contains("Sessions") && !tabs.contains("Chief"), "Chief folds first: {tabs}");
    press(&mut app, &[Key::Char('5')]);
    let tabs = tabs_row(&mut app, 50);
    assert!(tabs.contains("Chief") && tabs.contains(" 4 "), "the open tab keeps its name: {tabs}");
    let tabs = tabs_row(&mut app, 30);
    for name in ["Sessions", "Todo", "Ideas", "Teams"] { assert!(!tabs.contains(name), "{name}: {tabs}"); }
    assert!(tabs.contains("Chief") && tabs.contains(" 1 ") && tabs.contains(" 2 "), "folded tabs keep their key caps: {tabs}");
}

#[test]
fn the_teams_badge_counts_teams_that_need_you_in_amber_else_live_teams() {
    for (has_request, badge, background) in [(true, "1", crate::app::YELLOW), (false, "1", crate::app::BLACK)] {
        let fixture = Fixture::new();
        let mut state = team_state();
        if !has_request { state["link_proposals"] = json!([]); }
        let (mut app, _) = team_app(&fixture, state);
        let buffer = render(&mut app, 100, 30);
        let tabs = &buffer_rows(&buffer)[1];
        let start = tabs.find("Teams").unwrap_or_else(|| panic!("{tabs}")) + "Teams ".len();
        assert_eq!(&tabs[start..start + 3], format!(" {badge} "), "{tabs}");
        let cell = &buffer[(tabs[..start + 1].chars().count() as u16, 1)];
        if background == crate::app::YELLOW { assert_eq!(cell.bg, crate::app::color(crate::app::YELLOW), "needs you is amber"); }
        else { assert_ne!(cell.bg, crate::app::color(crate::app::YELLOW), "a live team is not amber"); }
    }
}

#[test]
fn the_chief_badge_comes_with_the_team_snapshot_without_reading_the_chief() {
    for (badge, expected, tint) in [(json!({"state": "active", "needs_you": 0}), "●", crate::app::GREEN), (json!({"state": "active", "needs_you": 2}), "2", crate::app::BLACK),
                                    (json!({"state": null, "needs_you": 0}), "○", crate::app::FG_DARK)] {
        let fixture = Fixture::new();
        let mut state = team_state();
        state["chief_badge"] = badge;
        let (mut app, requests) = team_app(&fixture, state);
        let buffer = render(&mut app, 100, 30);
        let tabs = &buffer_rows(&buffer)[1];
        let start = tabs.find("Chief").unwrap_or_else(|| panic!("{tabs}")) + "Chief ".len();
        assert_eq!(&tabs[start..].chars().nth(1).unwrap().to_string(), expected, "{tabs}");
        assert_eq!(buffer[(tabs[..start + 1].chars().count() as u16, 1)].fg, crate::app::color(tint), "{tabs}");
        assert!(!requests.lock().unwrap().iter().any(|request| request["request"] == "chief_snapshot"), "no Chief polling outside its tab");
    }
}

#[test]
fn the_teams_tab_lists_teams_by_attention_and_shows_the_selected_team_below() {
    let fixture = Fixture::new();
    let (mut app, _) = team_app(&fixture, team_state());
    press(&mut app, &[Key::Char('4')]);
    let shown = texts(&app, TEAMS_PANEL).join("\n");
    for expected in ["NEEDS YOU · 1", "Session list redesign", "project-a › a5dba5b2 · Designer ● Developer ● QA ○",
        " ! Link request: Designer ⇄ stray-coop", "MEMBERS · 3", "Designer", "Developer", "Starts at the next handoff",
        "! Link request: Designer ⇄ stray-coop", "To share the game-test manifest format.", "IN FLIGHT · 1",
        "Developer → QA · handoff 34c1876 · queued until QA starts", "RECENT", "Developer handed off (commit 34c1876)"] {
        assert!(shown.contains(expected), "missing {expected}: {shown}");
    }
    assert!(!shown.contains("‹ Session list redesign"), "the list is not inside the team: {shown}");
}

#[test]
fn needs_you_keys_do_nothing_from_the_team_list() {
    let fixture = Fixture::new();
    let (mut app, requests) = team_app(&fixture, team_state());
    press(&mut app, &[Key::Char('4')]);
    for key in ['y', 'n', 'e', 'c', 'u'] { app.handle(Key::Char(key), &mut FakeScreen::answering(&[Key::Char('y')])); }
    assert!(controls(&requests).is_empty(), "{:?}", controls(&requests));
    assert!(fixture.calls("cockpit").is_empty());
}

#[test]
fn inside_a_team_y_approves_the_selected_link_request_after_confirmation() {
    let fixture = Fixture::new();
    let (mut app, requests) = team_app(&fixture, team_state());
    press(&mut app, &[Key::Char('4'), Key::Enter]);
    assert!(texts(&app, TEAMS_PANEL)[0].contains("‹ Session list redesign"), "{:?}", texts(&app, TEAMS_PANEL));
    press(&mut app, &[Key::Char('j'), Key::Char('j'), Key::Char('j')]);
    app.handle(Key::Char('y'), &mut FakeScreen::answering(&[Key::Char('n')]));
    assert!(controls(&requests).is_empty(), "approving needs its confirmation");
    app.handle(Key::Char('y'), &mut FakeScreen::answering(&[Key::Char('y')]));
    let actions = controls(&requests);
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!((&actions[0]["action"], &actions[0]["proposal_id"]), (&json!("link_approve"), &json!("lp1")));
    assert!(app.team_view.is_none(), "the Team view acted without showing");
    press(&mut app, &[Key::Esc]);
    assert!(!texts(&app, TEAMS_PANEL).join("\n").contains("‹ Session list redesign"), "esc goes back to the list");
}

#[test]
fn inside_a_team_enter_opens_a_members_session_and_u_unlinks_it() {
    let fixture = Fixture::new();
    let (mut app, requests) = team_app(&fixture, team_state());
    press(&mut app, &[Key::Char('4'), Key::Enter, Key::Enter]);
    assert_eq!(fixture.wait_for_calls("cockpit", 1), [format!("cockpit open {SESSION_A}")]);
    app.handle(Key::Char('u'), &mut FakeScreen::answering(&[Key::Char('y')]));
    let actions = controls(&requests);
    assert_eq!((&actions[0]["action"], &actions[0]["session_id"]), (&json!("unlink"), &json!(SESSION_A)), "{actions:?}");
}

#[test]
fn l_inside_a_team_opens_the_full_log() {
    let fixture = Fixture::new();
    let (mut app, _) = team_app(&fixture, team_state());
    press(&mut app, &[Key::Char('4'), Key::Char('l')]);
    assert!(app.team_view.is_none(), "l does nothing from the list");
    press(&mut app, &[Key::Enter, Key::Char('l')]);
    assert!(app.team_view.as_ref().is_some_and(|view| view.is_history && !view.hidden));
}

#[test]
fn i_or_4_on_a_team_block_opens_the_teams_tab_on_that_team() {
    for key in ['i', '4'] {
        let fixture = Fixture::new();
        let mut state = team_state();
        state["members"][1]["task_id"] = json!("0123abcd");
        let (mut app, _) = team_app(&fixture, state);
        sidebar_select(&mut app, "› 0123abcd");
        press(&mut app, &[Key::Char(key)]);
        assert_eq!(app.focus, TEAMS_PANEL);
        let selected = &texts(&app, TEAMS_PANEL);
        let divider = selected.iter().position(|line| line.starts_with('─')).unwrap_or_else(|| panic!("{selected:?}"));
        assert_eq!(selected[divider + 1], "0123abcd", "{key}: {selected:?}");
    }
}

#[test]
fn an_ended_team_folds_under_ended_today_and_d_shows_it() {
    let fixture = Fixture::new();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "project-a", "/r/project-a", 1), session(OPENCODE_SESSION, "waiting", "project-a", "/r/project-a", 2)]));
    team_host(&fixture, team_state(), vec![]);
    let mut app = fixture.app();
    let mut state = team_state();
    state["link_proposals"] = json!([]);
    team_host(&fixture, state, vec![]);
    app.team_checked = 0.0;
    app.check_team();
    app.todos.items = vec![todo_item(TASK, "Session list redesign", "done")];
    app.redraw();
    press(&mut app, &[Key::Char('4')]);
    let folded = texts(&app, TEAMS_PANEL).join("\n");
    assert!(folded.contains("▸ Ended today · 1") && folded.contains("No teams right now") && !folded.contains("MEMBERS"), "{folded}");
    press(&mut app, &[Key::Char('D')]);
    let shown = texts(&app, TEAMS_PANEL).join("\n");
    assert!(shown.contains("▾ Ended today · 1") && shown.contains("✓Session list redesign") && shown.contains("Handoff submitted"), "{shown}");
}

#[test]
fn unavailable_team_data_is_red_with_retry_and_never_shown_as_independent_sessions() {
    let fixture = Fixture::new();
    sidebar_sessions(&fixture);
    let mut app = fixture.app();
    press(&mut app, &[Key::Char('4')]);
    let shown = texts(&app, TEAMS_PANEL).join(" ");
    assert!(shown.starts_with("! Team data unavailable: cockpit-teams has not answered") && shown.ends_with("Sessions still show in 1. R retry"), "{shown}");
    assert_eq!(app.panels[TEAMS_PANEL].lines[0].segments[0].2.fg, Some(crate::app::color(crate::app::RED)));
    assert!(!texts(&app, TEAMS_PANEL).join("\n").contains("Design session"));
    team_host(&fixture, team_state(), vec![]);
    press(&mut app, &[Key::Char('R')]);
    assert!(texts(&app, TEAMS_PANEL).join("\n").contains("MEMBERS · 3"), "R asks again: {:?}", texts(&app, TEAMS_PANEL));
}

#[test]
fn no_teams_says_how_a_team_starts() {
    let fixture = Fixture::new();
    let mut state = team_state();
    for field in ["members", "qa_plans", "handoffs", "messages", "link_proposals"] { state[field] = json!([]); }
    let (mut app, _) = team_app(&fixture, state);
    press(&mut app, &[Key::Char('4')]);
    assert_eq!(texts(&app, TEAMS_PANEL), ["No teams right now. A team starts when a session recruits a teammate."]);
}

#[test]
fn the_sessions_team_block_keeps_role_doing_now_and_age_and_points_to_teams() {
    let fixture = Fixture::new();
    let (app, _) = team_app(&fixture, team_state());
    let lines = &app.panels[1].lines;
    let header = lines.iter().find(|line| line.texts() == "▾ Session list redesign").unwrap_or_else(|| panic!("{:?}", texts(&app, 1)));
    assert_eq!(header.right.iter().map(|part| part.text.as_str()).collect::<Vec<_>>(), [" 4 ", "team details"]);
    assert_eq!(header.segments[0].2.fg, Some(crate::app::color(crate::app::CYAN)));
    let block: Vec<String> = lines.iter().map(|line| line.texts()).collect();
    let members = block.iter().position(|text| text.contains("Designer")).unwrap();
    let ask = block.iter().position(|text| text.contains("! Designer: approve a link to stray-coop")).unwrap_or_else(|| panic!("{block:?}"));
    assert!(ask > members, "the team's ask closes its block: {block:?}");
    for moved in ["Handoff submitted", "handoff 34c1876", "To share the game-test"] {
        assert!(!block.iter().any(|text| text.contains(moved)), "{moved} is on the Teams tab: {block:?}");
    }
    assert!(block.iter().any(|text| text.starts_with("NEEDS YOU")) && block.iter().position(|text| text.starts_with("NEEDS YOU")) < Some(members),
        "a pending link request puts the team in NEEDS YOU: {block:?}");
}
