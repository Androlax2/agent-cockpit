//! claude-todo, the command sessions use inside the sandbox to update Theo's todo list: reads the
//! list, hands its changes to the OSD daemon on the report socket.

mod common;
#[path = "common/link_fixture.rs"]
mod link_fixture;
use cockpit::teams::store::Connection as LinkConnection;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

fn claude_todo(sandbox: &Sandbox, arguments: &[&str], cwd: &Path) -> Output {
    let todo = sandbox.state.join("todo.json").display().to_string();
    sandbox.command(bin().join("cockpit-todo"), &[("CLAUDE_TODO_PATH", &todo), ("CLAUDE_CODE_SESSION_ID", SESSION_A)])
        .args(arguments).current_dir(cwd).output().unwrap()
}

/// The daemon's report socket, answering every update `reply`.
fn daemon(sandbox: &Sandbox, reply: &str) -> ReportSocket {
    ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), reply)
}

/// The updates the daemon received.
fn updates(daemon: &ReportSocket) -> Vec<Value> {
    daemon.received(Duration::from_millis(500)).iter()
        .map(|line| serde_json::from_str(line.strip_prefix("todo-update ").unwrap_or_else(|| panic!("not a todo update: {line}"))).unwrap()).collect()
}

#[test]
fn start_and_done_append_an_update_with_the_session() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["start", "0123abcd"], &sandbox.root).status.code(), Some(0));
    assert_eq!(claude_todo(&sandbox, &["done", "o/runway#12"], &sandbox.root).status.code(), Some(0));
    let recorded: Vec<(Value, Value, Value)> = updates(&daemon).into_iter()
        .map(|update| (update["action"].clone(), update["id"].clone(), update["session"].clone())).collect();
    assert_eq!(recorded, [(json!("doing"), json!("0123abcd"), json!(SESSION_A)), (json!("done"), json!("o/runway#12"), json!(SESSION_A))]);
}

#[test]
fn cli_done_closes_shared_work_when_the_host_processes_its_update() {
    if std::env::var_os("TODO_LINK_COMPLETION_CHILD").is_none() {
        let sandbox = Sandbox::new();
        let output = sandbox.command(std::env::current_exe().unwrap(), &[("TODO_LINK_COMPLETION_CHILD", "1")])
            .args(["--exact", "cli_done_closes_shared_work_when_the_host_processes_its_update", "--nocapture"]).output().unwrap();
        assert!(output.status.success(), "{}\n{}", stdout(&output), stderr(&output));
        return;
    }
    let project = cockpit::paths::home().join("IdeaProjects/project");
    fs::create_dir_all(project.join(".git")).unwrap();
    let mut todos = cockpit::todo::TodoList::load();
    let id = todos.add("project", project.to_str().unwrap(), "Joint work");
    let connection = cockpit::teams::store::open(&cockpit::paths::team_database()).unwrap();
    link_fixture::approve(&connection, &format!("path:{}", project.display()), &id);
    let daemon = ReportSocket::start(&cockpit::paths::osd_report_socket(), "ok");
    let output = std::process::Command::new(bin().join("cockpit-todo")).args(["done", &id])
        .env("CLAUDE_CODE_SESSION_ID", SESSION_A).current_dir(&project).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let mut records = updates(&daemon);
    assert_eq!(records.len(), 1);
    records[0]["started_in"] = json!(project);
    fs::write(cockpit::paths::todo_updates(), format!("{}\n", records[0])).unwrap();
    assert_eq!(todos.apply_updates(), 1);
    assert_eq!(todos.status_of(&id), "done");
    assert!(todos.set_status(&id, "todo", ""));
    assert_eq!(link_fixture::state(&connection, &id), "closed");
}

#[test]
fn a_transport_acknowledgment_does_not_claim_a_handoff_was_saved() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["add", "Deploy pinned greywall", "--project", "agent-cockpit"], &sandbox.root);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(124), "{text}");
    assert!(text.contains("pending"), "{text}");
    assert!(!text.contains("Added to") && !text.contains("and queued"), "{text}");
    let recorded = updates(&daemon);
    assert!(recorded[0]["request_id"].as_str().is_some_and(|id| id.len() == 12));
}

const FILINGS: &[&[&str]] = &[
    &["add", "Work"],
    &["add", "Work", "--project", "greyproxy"],
    &["propose", "Work", "--why", "Needed"],
    &["blocked", "Work", "--why", "Unavailable"],
    &["stack", "greyproxy", "Work", "--why", "Broken"],
    &["greywall", "Work", "--why", "Broken"],
];

#[test]
fn filing_returns_the_persisted_result_instead_of_transport_success() {
    use std::process::Stdio;
    for arguments in FILINGS {
        for (outcome, code) in [("accepted", 0), ("rejected", 1)] {
            let sandbox = Sandbox::new();
            let daemon = daemon(&sandbox, "ok");
            let child = sandbox.command(bin().join("cockpit-todo"), &[("CLAUDE_CODE_SESSION_ID", SESSION_A)])
                .args(*arguments).current_dir(&sandbox.root).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            let recorded = updates(&daemon);
            let id = recorded[0]["request_id"].as_str().expect("every filing needs a receipt");
            let result = json!({"request_id": id, "outcome": outcome,
                               "id": if outcome == "accepted" { Some("1234abcd") } else { None },
                               "project": "greyproxy", "state": if outcome == "accepted" { Some("queued") } else { None },
                               "status": if outcome == "accepted" { Some("todo") } else { None },
                               "reason": if outcome == "accepted" { "" } else { "3 already open" }});
            cockpit::util::write_json(&sandbox.answers("todos").join(format!("{id}.json")), &result).unwrap();
            let output = finish(child, Duration::from_secs(10));
            assert_eq!(output.status.code(), Some(code), "{arguments:?}: {}{}", stdout(&output), stderr(&output));
            let text = stdout(&output);
            assert!(text.contains(outcome), "{arguments:?}: {text}");
            if outcome == "accepted" {
                assert!(text.contains("1234abcd") && text.contains("greyproxy"), "{text}");
            } else {
                assert!(text.contains("3 already open"), "{text}");
            }
        }
    }
}

#[test]
fn a_filing_receipt_exposes_the_saved_id_or_rejection_and_missing_is_pending() {
    let sandbox = Sandbox::new();
    let folder = sandbox.runtime.join("cockpit-answers").join(ANSWERS_KEY).join("todos");
    fs::create_dir_all(&folder).unwrap();
    let id = "123456abcdef";
    for (outcome, code) in [("accepted", 0), ("rejected", 1)] {
        fs::write(folder.join(format!("{id}.json")), json!({"request_id": id, "outcome": outcome, "id": "1234abcd", "reason": "filing cap"}).to_string()).unwrap();
        let output = claude_todo(&sandbox, &["receipt", id], &sandbox.root);
        assert_eq!(output.status.code(), Some(code), "{}{}", stdout(&output), stderr(&output));
        assert!(stdout(&output).contains("1234abcd"));
    }
    let output = claude_todo(&sandbox, &["receipt", "abcdef123456"], &sandbox.root);
    assert_eq!(output.status.code(), Some(124));
}

#[test]
fn both_command_names_report_each_update_once_after_migration() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for command in ["claude-todo", "cockpit-todo"] {
        let output = sandbox.command(bin().join(command), &[("CLAUDE_CODE_SESSION_ID", SESSION_A)])
            .args(["start", "0123abcd"]).current_dir(&sandbox.root).output().unwrap();
        assert!(output.status.success(), "{command}: {}", stderr(&output));
    }
    let recorded = updates(&daemon);
    assert_eq!(recorded.len(), 2);
    assert!(recorded.iter().all(|update| update["action"] == "doing" && update["id"] == "0123abcd"));
}

#[test]
fn delete_appends_an_update_with_the_item_and_the_session() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["delete", "0123abcd"], &sandbox.root).status.code(), Some(0));
    let recorded: Vec<(Value, Value, Value)> = updates(&daemon).into_iter()
        .map(|update| (update["action"].clone(), update["id"].clone(), update["session"].clone())).collect();
    assert_eq!(recorded, [(json!("delete"), json!("0123abcd"), json!(SESSION_A))]);
}

#[test]
fn delete_refuses_an_issue_or_a_malformed_id() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for id in ["o/runway#12", "rm -rf"] {
        assert_eq!(claude_todo(&sandbox, &["delete", id], &sandbox.root).status.code(), Some(2), "{id}");
    }
    assert!(updates(&daemon).is_empty());
}

#[test]
fn a_malformed_id_is_refused() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["done", "rm -rf"], &sandbox.root).status.code(), Some(2));
    assert!(updates(&daemon).is_empty());
}

#[test]
fn list_shows_the_items_of_this_directorys_repository() {
    let sandbox = Sandbox::new();
    let checkout = sandbox.root.join("runway");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    let todo = json!({"items": [
        {"id": "0123abcd", "project": "runway", "text": "deploy", "status": "doing"},
        {"id": "4567cdef", "project": "homelab", "text": "postgres", "status": "todo"}], "issues": {}});
    fs::write(sandbox.state.join("todo.json"), todo.to_string()).unwrap();
    let listed = stdout(&claude_todo(&sandbox, &["list"], &checkout));
    assert!(listed.contains("0123abcd"), "{listed}");
    assert!(!listed.contains("4567cdef"), "{listed}");
    let listed_all = stdout(&claude_todo(&sandbox, &["list", "--all"], &checkout));
    assert!(listed_all.contains("4567cdef"), "{listed_all}");
}

#[test]
fn the_canonical_todo_path_wins_over_the_legacy_override() {
    let sandbox = Sandbox::new();
    let canonical = sandbox.state.join("canonical-todo.json");
    let legacy = sandbox.state.join("legacy-todo.json");
    fs::write(&canonical, json!({"items": [{"id": "0123abcd", "project": "shop", "text": "canonical task", "status": "todo"}]}).to_string()).unwrap();
    fs::write(&legacy, json!({"items": [{"id": "4567cdef", "project": "shop", "text": "legacy task", "status": "todo"}]}).to_string()).unwrap();
    let output = sandbox.run(bin().join("cockpit-todo"), &["list", "--all"],
        &[("COCKPIT_TODO_PATH", canonical.to_str().unwrap()), ("CLAUDE_TODO_PATH", legacy.to_str().unwrap())]);
    assert!(output.status.success(), "{}", stdout(&output));
    let shown = stdout(&output);
    assert!(shown.contains("canonical task"), "{shown}");
    assert!(!shown.contains("legacy task"), "{shown}");
}

#[test]
fn list_reads_the_cockpits_state_folder_under_an_agents_private_xdg_state_home() {
    // The opencode launcher moves XDG_STATE_HOME into the sandbox's /tmp for opencode's own state.
    let sandbox = Sandbox::new();
    let checkout = sandbox.root.join("runway");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    fs::create_dir_all(sandbox.state.join("cockpit")).unwrap();
    fs::write(sandbox.state.join("cockpit/todo.json"),
              json!({"items": [{"id": "0123abcd", "project": "runway", "text": "deploy", "status": "todo"}], "issues": {}}).to_string()).unwrap();
    let private = sandbox.root.join("opencode-greywall-state").display().to_string();
    let state = sandbox.state.display().to_string();
    let output = sandbox.command(bin().join("claude-todo"), &[("XDG_STATE_HOME", &private), ("COCKPIT_STATE_HOME", &state)])
        .args(["show", "0123abcd"]).current_dir(&checkout).output().unwrap();
    assert!(stdout(&output).contains("deploy"), "{}{}", stdout(&output), stderr(&output));
}

#[test]
fn propose_files_an_idea_with_why_and_notes() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["propose", "Cache the usage fetch", "--why", "it runs every minute", "--notes", "use the ETag"], &sandbox.root);
    assert_eq!(output.status.code(), Some(124));
    let update = &updates(&daemon)[0];
    assert_eq!((update["action"].as_str(), update["why"].as_str(), update["notes"].as_str()),
               (Some("propose"), Some("it runs every minute"), Some("use the ETag")));
}

#[test]
fn propose_without_a_reason_is_refused() {
    let sandbox = Sandbox::new();
    assert_eq!(claude_todo(&sandbox, &["propose", "Cache it"], &sandbox.root).status.code(), Some(2));
}

#[test]
fn an_idea_theo_declined_is_not_proposed_again() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    fs::write(sandbox.state.join("todo.json"), json!({"items": [], "declined": ["Rewrite in Go"]}).to_string()).unwrap();
    let output = claude_todo(&sandbox, &["propose", "rewrite in go", "--why", "faster"], &sandbox.root);
    assert!(stdout(&output).contains("already declined"), "{}", stdout(&output));
    assert!(updates(&daemon).is_empty());
}

#[test]
fn design_marks_an_item_for_the_designer_session() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["add", "Empty state of the order list", "--design", "--notes", "no frame for 0 orders"], &sandbox.root).status.code(), Some(124));
    assert_eq!(claude_todo(&sandbox, &["propose", "Error state", "--why", "the API fails often", "--design"], &sandbox.root).status.code(), Some(124));
    let filed = updates(&daemon);
    assert_eq!(filed.iter().map(|update| update["kind"].as_str()).collect::<Vec<_>>(), [Some("design"), Some("design")]);
    assert_eq!(filed[0]["notes"].as_str(), Some("no frame for 0 orders"));
}

#[test]
fn an_item_without_design_has_no_kind() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    claude_todo(&sandbox, &["add", "Bump the deps"], &sandbox.root);
    assert_eq!(updates(&daemon)[0].get("kind"), None);
}

#[test]
fn blocked_files_a_report_with_what_stopped_the_session() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["blocked", "Push the release tag", "--why", "greywall hides ~/.ssh"], &sandbox.root);
    assert_eq!(output.status.code(), Some(124));
    let update = &updates(&daemon)[0];
    assert_eq!((update["action"].as_str(), update["why"].as_str()), (Some("blocked"), Some("greywall hides ~/.ssh")));
}

#[test]
fn greywall_is_stack_greywall_with_its_kind_and_reason() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["greywall", "Security: masks lose to later binds", "--why", "~/.ssh/id_* readable", "--kind", "bug", "--notes", "linux.go"], &sandbox.root);
    assert_eq!(output.status.code(), Some(124), "{}", stderr(&output));
    let update = &updates(&daemon)[0];
    assert_eq!((update["action"].as_str(), update["why"].as_str(), update["category"].as_str(), update["notes"].as_str()),
               (Some("stack"), Some("~/.ssh/id_* readable"), Some("bug"), Some("linux.go")));
    assert_eq!(update["repo"].as_str(), Some("greywall"));
}

#[test]
fn stack_names_the_repository_the_report_is_for() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["stack", "greyproxy", "Security: the built-in rule opens the API", "--why", "any sandbox reaches it", "--kind", "bug"], &sandbox.root);
    assert_eq!(output.status.code(), Some(124), "{}", stderr(&output));
    let update = &updates(&daemon)[0];
    assert_eq!((update["action"].as_str(), update["repo"].as_str(), update["text"].as_str()),
               (Some("stack"), Some("greyproxy"), Some("Security: the built-in rule opens the API")));
}

#[test]
fn stack_outside_the_stack_repositories_or_without_a_title_sends_no_update() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for arguments in [vec!["stack", "dotfiles", "Title", "--why", "reason"], vec!["stack", "Title", "--why", "reason"],
                      vec!["stack", "greyproxy", "--why", "reason"], vec!["stack", "greyproxy", "Title", "--why", "reason", "--design"],
                      vec!["stack", "greyproxy", "Title", "--why", "reason", "--project", "runway"]] {
        assert_eq!(claude_todo(&sandbox, &arguments, &sandbox.root).status.code(), Some(2), "{arguments:?}");
    }
    assert!(updates(&daemon).is_empty());
}

#[test]
fn greywall_without_a_reason_or_with_design_sends_no_update() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for arguments in [vec!["greywall", "Title"], vec!["greywall", "Title", "--why", "reason", "--design"]] {
        assert_eq!(claude_todo(&sandbox, &arguments, &sandbox.root).status.code(), Some(2), "{arguments:?}");
    }
    assert!(updates(&daemon).is_empty());
}

#[test]
fn help_says_stack_code_goes_to_its_repository_and_config_blocks_to_blocked() {
    let sandbox = Sandbox::new();
    let usage = stdout(&claude_todo(&sandbox, &["stack", "--help"], &sandbox.root));
    assert!(usage.contains("cockpit-todo stack <repo> '<title>'") && usage.contains("greyproxy"), "{usage}");
    assert!(usage.contains("greywall.json"), "{usage}");
}

#[test]
fn propose_with_project_names_the_project_it_is_for() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["propose", "Pin the greywall build", "--why", "builds drift", "--project", "dotfiles"], &sandbox.root);
    assert_eq!(output.status.code(), Some(124), "{}", stderr(&output));
    let update = &updates(&daemon)[0];
    assert_eq!((update["action"].as_str(), update["project"].as_str()), (Some("propose"), Some("dotfiles")));
}

#[test]
fn project_outside_propose_or_without_a_name_sends_no_update() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for arguments in [vec!["blocked", "Title", "--why", "reason", "--project", "runway"], vec!["greywall", "Title", "--why", "reason", "--project", "runway"],
                      vec!["propose", "Title", "--why", "reason", "--project"]] {
        assert_eq!(claude_todo(&sandbox, &arguments, &sandbox.root).status.code(), Some(2), "{arguments:?}");
    }
    assert!(updates(&daemon).is_empty());
}

#[test]
fn add_and_propose_file_each_kind_without_losing_design_routing() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for action in ["add", "propose"] {
        for kind in ["task", "feature", "bug", "improvement", "chore", "research"] {
            let mut arguments = vec![action, "Fix the empty state", "--kind", kind, "--design", "--notes", "Keep the notes"];
            if action == "propose" { arguments.extend(["--why", "It is broken"]); }
            let output = claude_todo(&sandbox, &arguments, &sandbox.root);
            assert_eq!(output.status.code(), Some(124), "{}", stderr(&output));
        }
    }
    let filed = updates(&daemon);
    let categories: Vec<_> = filed.iter().map(|update| update["category"].as_str()).collect();
    assert_eq!(categories, [Some("task"), Some("feature"), Some("bug"), Some("improvement"), Some("chore"), Some("research")].repeat(2));
    assert!(filed.iter().all(|update| update["kind"] == "design" && update["notes"] == "Keep the notes"));
}

#[test]
fn malformed_kind_options_send_no_update() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for arguments in [
        vec!["add", "Title", "--kind"],
        vec!["add", "Title", "--kind", "other"],
        vec!["add", "Title", "--kind", "design"],
        vec!["add", "Title", "--kind", "bug", "--kind", "feature"],
        vec!["propose", "Title", "--why", "reason", "--kind", ""],
        vec!["blocked", "Title", "--why", "reason", "--kind", "bug"],
    ] {
        assert_eq!(claude_todo(&sandbox, &arguments, &sandbox.root).status.code(), Some(2), "{arguments:?}");
    }
    assert!(updates(&daemon).is_empty());
}

#[test]
fn flag_names_in_titles_and_notes_are_literal_text() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for arguments in [vec!["add", "--design"], vec!["add", "Title", "--notes", "--design"], vec!["add", "--kind", "--kind", "bug"]] {
        assert_eq!(claude_todo(&sandbox, &arguments, &sandbox.root).status.code(), Some(124));
    }
    let filed = updates(&daemon);
    assert!(filed.iter().all(|update| update.get("kind").is_none()));
    assert_eq!(filed[0]["text"], "--design");
    assert_eq!(filed[1]["notes"], "--design");
    assert_eq!(filed[2]["text"], "--kind");
}

#[test]
fn help_prints_the_usage_and_files_nothing() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for arguments in [vec!["--help"], vec!["-h"], vec!["help"], vec!["add", "--help"], vec!["propose", "-h"], vec!["blocked", "--help"], vec!["add", "--help", "--kind", "bug"]] {
        let output = claude_todo(&sandbox, &arguments, &sandbox.root);
        assert_eq!(output.status.code(), Some(0), "{arguments:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("cockpit-todo add '<title>'"), "{arguments:?}");
    }
    assert!(updates(&daemon).is_empty());
}

#[test]
fn help_says_work_for_another_project_is_proposed_with_its_name() {
    let sandbox = Sandbox::new();
    let output = claude_todo(&sandbox, &["--help"], &sandbox.root);
    let usage = String::from_utf8_lossy(&output.stdout);
    assert!(usage.contains("[--project <name>]"), "{usage}");
    assert!(!usage.contains("linked"), "{usage}");
}

/// A checkout named after a project, where claude-todo runs as that project's session.
fn checkout(sandbox: &Sandbox, project: &str) -> std::path::PathBuf {
    let folder = sandbox.root.join(project);
    fs::create_dir_all(folder.join(".git")).unwrap();
    folder
}

#[test]
fn add_names_another_project_and_the_items_it_waits_on() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let greywall = checkout(&sandbox, "greywall");
    let output = claude_todo(&sandbox, &["add", "client secret", "--project", "greyproxy", "--after", "0123abcd", "--after", "4567abcd"], &greywall);
    assert_eq!(output.status.code(), Some(124), "{}", String::from_utf8_lossy(&output.stderr));
    let sent = updates(&daemon).remove(0);
    assert_eq!((sent["action"].clone(), sent["project"].clone(), sent["after"].clone()), (json!("add"), json!("greyproxy"), json!(["0123abcd", "4567abcd"])));
}

#[test]
fn add_hands_work_to_any_project_without_claiming_it_is_already_queued() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["add", "strict mode", "--project", "dotfiles"], &checkout(&sandbox, "runway"));
    assert_eq!(output.status.code(), Some(124), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout(&output).contains("no saved result"), "{}", stdout(&output));
    let sent = updates(&daemon).remove(0);
    assert_eq!((sent["action"].clone(), sent["project"].clone()), (json!("add"), json!("dotfiles")));
}

#[test]
fn unconfirmed_filings_time_out_without_claiming_a_saved_item() {
    let sandbox = Sandbox::new();
    let _daemon = daemon(&sandbox, "ok");
    let runway = checkout(&sandbox, "runway");
    for arguments in [
        vec!["add", "Title"],
        vec!["add", "Title", "--project", "runway"],
        vec!["add", "Title", "--project", "dotfiles"],
        vec!["add", "Title", "--after", "0123abcd"],
        vec!["add", "Title", "--project", "dotfiles", "--after", "0123abcd"],
        vec!["propose", "Title", "--why", "reason"],
        vec!["propose", "Title", "--why", "reason", "--project", "dotfiles"],
        vec!["stack", "greyproxy", "Title", "--why", "reason"],
        vec!["greywall", "Title", "--why", "reason"],
        vec!["blocked", "Title", "--why", "reason"],
    ] {
        let output = claude_todo(&sandbox, &arguments, &runway);
        assert_eq!(output.status.code(), Some(124), "{arguments:?}: {}", stderr(&output));
        let message = stdout(&output);
        assert!(message.starts_with("Submitted") && message.contains("not yet saved"), "{arguments:?}: {message}");
        assert!(message.contains("Do not file a duplicate"), "{arguments:?}: {message}");
        assert!(message.contains("cockpit-todo receipt"), "{arguments:?}: {message}");
        assert!(!message.contains("Added to") && !message.contains("Proposed to") && !message.contains("Reported in"), "{arguments:?}: {message}");
    }
}

#[test]
fn after_takes_item_ids_only() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    for arguments in [&["after", "0123abcd"][..], &["after", "0123abcd", "o/r#1"], &["add", "x", "--after", "nope"]] {
        assert_eq!(claude_todo(&sandbox, arguments, &sandbox.root).status.code(), Some(2), "{arguments:?}");
    }
    assert_eq!(claude_todo(&sandbox, &["after", "0123abcd", "4567abcd"], &sandbox.root).status.code(), Some(0));
    let sent = updates(&daemon);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0]["action"].clone(), sent[0]["id"].clone(), sent[0]["after"].clone()), (json!("after"), json!("0123abcd"), json!(["4567abcd"])));
}

#[test]
fn done_carries_the_sessions_summary() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["done", "0123abcd", "--summary", "branch x, PR #3"], &sandbox.root).status.code(), Some(0));
    let sent = updates(&daemon).remove(0);
    assert_eq!((sent["action"].clone(), sent["summary"].clone()), (json!("done"), json!("branch x, PR #3")));
}

#[test]
fn a_reader_that_stops_early_ends_the_list_quietly() {
    // claude-todo list | head -3 printed a panic (failed printing to stdout: Broken pipe).
    let sandbox = Sandbox::new();
    let items: Vec<Value> = (0..40).map(|index| json!({"id": format!("{index:08x}"), "project": "runway", "text": format!("item {index}")})).collect();
    fs::write(sandbox.state.join("todo.json"), json!({"items": items}).to_string()).unwrap();
    let todo = sandbox.state.join("todo.json").display().to_string();
    let mut child = sandbox.command(bin().join("claude-todo"), &[("CLAUDE_TODO_PATH", &todo)]).args(["list", "--all"])
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert!(!stderr(&output).contains("panicked"), "{}", stderr(&output));
}

#[test]
fn list_and_show_expose_kinds_including_legacy_tasks() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.state.join("todo.json"), json!({"items": [
        {"id": "0123abcd", "project": "runway", "text": "crash", "category": "bug"},
        {"id": "4567abcd", "project": "runway", "text": "old"},
        {"id": "89abcdef", "project": "runway", "text": "future", "category": "future-kind"}
    ]}).to_string()).unwrap();
    let listed = stdout(&claude_todo(&sandbox, &["list", "--all"], &sandbox.root));
    assert!(listed.contains("[Bug]") && listed.contains("[Task]") && listed.contains("future-kind"), "{listed}");
    let shown = stdout(&claude_todo(&sandbox, &["show", "0123abcd"], &sandbox.root));
    assert!(shown.contains("Kind: Bug"), "{shown}");
}

#[test]
fn show_lists_the_paths_of_pasted_images_and_skips_malformed_ones() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.state.join("todo.json"), json!({"items": [
        {"id": "0123abcd", "project": "runway", "text": "header", "notes": "Like [Image 1]",
         "images": {"1": "0123456789abcdef.png", "2": "../../.ssh/id_ed25519"}}
    ]}).to_string()).unwrap();
    let shown = stdout(&claude_todo(&sandbox, &["show", "0123abcd"], &sandbox.root));
    let expected = format!("[Image 1] {}", sandbox.state.join("todo-images/0123456789abcdef.png").display());
    assert!(shown.contains(&expected), "{shown}");
    assert!(!shown.contains("[Image 2]") && !shown.contains(".ssh"), "{shown}");
}

#[test]
fn blocked_without_what_stopped_it_is_refused() {
    let sandbox = Sandbox::new();
    assert_eq!(claude_todo(&sandbox, &["blocked", "Push the release tag"], &sandbox.root).status.code(), Some(2));
}

#[test]
fn an_update_the_daemon_refuses_says_why() {
    let sandbox = Sandbox::new();
    let _daemon = daemon(&sandbox, "refused: the todo queue is full");
    let output = claude_todo(&sandbox, &["done", "0123abcd"], &sandbox.root);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("the todo queue is full"), "{}", stderr(&output));
}

#[test]
fn without_a_daemon_an_update_fails_loudly() {
    let sandbox = Sandbox::new();
    let output = claude_todo(&sandbox, &["done", "0123abcd"], &sandbox.root);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("cannot reach the OSD daemon"), "{}", stderr(&output));
}
