//! The Claude Code hook (config/claude/.claude/hooks/session-state.sh): one report per event, on the
//! daemon's report socket.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct HookTest {
    sandbox: Sandbox,
    socket: PathBuf,
    lines: Arc<Mutex<Vec<String>>>,
}

impl HookTest {
    /// A stand-in for the daemon's report socket: every line any connection sends is kept.
    fn new() -> HookTest {
        Self::with_reply("ok")
    }

    fn with_reply(reply: &str) -> HookTest {
        let sandbox = Sandbox::new();
        let socket = sandbox.runtime.join("claude-report/osd-report.sock");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let collected = lines.clone();
        let reply = format!("{reply}\n");
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut text = String::new();
                let _ = stream.read_to_string(&mut text);
                if text.starts_with("session-request ") {
                    let _ = stream.write_all(reply.as_bytes());
                }
                collected.lock().unwrap().extend(text.lines().map(str::to_string));
            }
        });
        sandbox.shim("notify-send", "echo 42");
        sandbox.shim("gdbus", "");
        HookTest { sandbox, socket, lines }
    }

    fn run_hook(&self, payload: Value, extra: &[(&str, &str)]) -> Output {
        let mut environment = vec![("KITTY_PID", "4242"), ("TMUX_PANE", "%3")];
        environment.extend_from_slice(extra);
        self.sandbox.run_with_input("bash", &[hook().to_str().unwrap()], &environment, &payload.to_string())
    }

    fn received(&mut self) -> Vec<Value> {
        std::thread::sleep(Duration::from_millis(300));
        self.lines.lock().unwrap().iter()
            .filter_map(|line| line.strip_prefix("session ").or_else(|| line.strip_prefix("session-request ")))
            .map(|payload| serde_json::from_str(payload).unwrap())
            .collect()
    }
}

fn with_session(payload: Value, cwd: &str) -> Value {
    let mut payload = payload;
    payload["session_id"] = json!(SESSION_A);
    payload["cwd"] = json!(cwd);
    payload
}

/// A stand-in process named "claude": a copy of bash under that name.
fn fake_claude(root: &Path) -> PathBuf {
    fake_agent(root, "claude")
}

/// A stand-in agent process: a copy of bash under the agent's name.
fn fake_agent(root: &Path, name: &str) -> PathBuf {
    let bash = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("bash"))
        .find(|candidate| candidate.is_file())
        .expect("bash on PATH");
    let fake = root.join(name);
    copy_executable(&bash, &fake);
    fake
}

#[test]
fn turn_ends_never_take_team_messages_themselves() {
    // cockpit-team watch (an asyncRewake hook) is the only taker for Claude, the plugin for
    // opencode: a second taker at Stop would race them.
    for agent in ["", "opencode"] {
        let mut test = HookTest::new();
        let team = test.sandbox.team_socket(json!({"dispatch_id": "d1", "message_id": "m1", "text": "hi"}));
        test.sandbox.shim("cockpit-team", &format!("exec '{}' \"$@\"", env!("CARGO_BIN_EXE_cockpit-team")));
        let output = test.run_hook(with_session(json!({"hook_event_name": "Stop"}), "/x/proj"), &[("COCKPIT_AGENT", agent)]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(output.stdout.is_empty(), "{agent}");
        assert_eq!(test.received().len(), 1, "{agent}");
        assert!(team.lock().unwrap().is_empty(), "{agent}");
    }
}

#[test]
fn continuity_stop_reuses_its_turn_id_but_a_new_prompt_gets_another() {
    for session in [SESSION_A, OPENCODE_SESSION] {
        let mut test = HookTest::new();
        for event in ["UserPromptSubmit", "Stop", "Stop", "UserPromptSubmit", "Stop"] {
            let mut payload = with_session(json!({"hook_event_name": event, "prompt": "work"}), "/x/proj");
            payload["session_id"] = json!(session);
            assert!(test.run_hook(payload, &[]).status.success());
        }
        let reports = test.received();
        let first = reports[1]["turn_id"].as_str().expect("completion has a stable turn id");
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()));
        assert_eq!(reports[2]["turn_id"], first);
        assert_ne!(reports[4]["turn_id"], first);
        assert_eq!(reports[1]["turn_complete"], true);
        assert!(reports[0].get("turn_complete").is_none());
    }
}

#[test]
fn continuity_a_stop_question_still_reports_completion() {
    let mut test = HookTest::new();
    for payload in [json!({"hook_event_name": "NeedsInput", "message": "Which provider?"}), json!({"hook_event_name": "Stop"})] {
        assert!(test.run_hook(with_session(payload, "/x/proj"), &[]).status.success());
    }
    let reports = test.received();
    assert_eq!(reports[1]["needs_input"], true);
    assert_eq!(reports[1]["turn_complete"], true);
    assert!(reports[1]["turn_id"].is_string());
}

#[test]
fn continuity_fallback_completion_does_not_reuse_a_prior_turn() {
    let mut test = HookTest::new();
    for event in ["Stop", "Stop", "PostToolUse", "Stop"] {
        assert!(test.run_hook(with_session(json!({"hook_event_name": event}), "/x/proj"), &[]).status.success());
    }
    let reports = test.received();
    assert!(reports[0]["turn_id"].is_string());
    assert_eq!(reports[0]["turn_id"], reports[1]["turn_id"]);
    assert_ne!(reports[0]["turn_id"], reports[3]["turn_id"]);
}

#[test]
fn continuity_repository_and_branch_are_read_inside_the_sandbox() {
    for (origin, expected) in [("git@github.com:Acme/Shop.git", "acme/shop"), ("https://github.com/Acme/Shop.git", "acme/shop"), ("ssh://git@github.com/Acme/Shop.git", "acme/shop"), ("/local/shop", "")] {
        let mut test = HookTest::new();
        test.sandbox.shim("git", &format!("case \"$*\" in *'remote get-url origin') echo '{origin}';; *'symbolic-ref --quiet --short HEAD') echo 'todo-aabbccdd';; esac"));
        assert!(test.run_hook(with_session(json!({"hook_event_name": "SessionStart", "source": "startup"}), "/x/proj"), &[]).status.success());
        let report = &test.received()[0];
        assert_eq!(report["repository"], expected);
        assert_eq!(report["branch"], "todo-aabbccdd");
    }
}

#[test]
fn continuity_a_branch_or_origin_the_daemon_would_refuse_is_left_out_of_the_report() {
    let long_branch = "b".repeat(301);
    for (origin, branch) in [("https://github.com/acme/shop", "feat#{x}"), ("https://github.com/acme/shop", "feat#(x)"), ("https://github.com/acme/shop", long_branch.as_str()),
                             ("https://github.com/../shop", "main"), ("https://github.com/acme/..", "main")] {
        let mut test = HookTest::new();
        test.sandbox.shim("git", &format!("case \"$*\" in *'remote get-url origin') echo '{origin}';; *'symbolic-ref --quiet --short HEAD') echo '{branch}';; esac"));
        assert!(test.run_hook(with_session(json!({"hook_event_name": "Stop"}), "/x/proj"), &[]).status.success());
        let report = &test.received()[0];
        assert_eq!(report["turn_complete"], true, "{origin} {branch}");
        let expected_repository = if origin.contains("..") { "" } else { "acme/shop" };
        let expected_branch = if branch == "main" { "main" } else { "" };
        assert_eq!((report["repository"].as_str(), report["branch"].as_str()), (Some(expected_repository), Some(expected_branch)), "{origin} {branch}");
    }
}

#[test]
fn events_map_to_states() {
    let mut test = HookTest::new();
    let cases = [
        (json!({"hook_event_name": "SessionStart", "source": "startup"}), "waiting", None),
        (json!({"hook_event_name": "UserPromptSubmit", "prompt": "add the chip\nmore"}), "working", None),
        (json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}), "working", None),
        (json!({"hook_event_name": "Notification", "message": "perm", "notification_type": "permission_prompt"}), "needs-you", Some(true)),
        (json!({"hook_event_name": "Notification", "message": "idle", "notification_type": "idle_prompt"}), "waiting", Some(true)),
        (json!({"hook_event_name": "Notification", "message": "done", "notification_type": "push_notification"}), "waiting", None),
        (json!({"hook_event_name": "Stop"}), "waiting", None),
        (json!({"hook_event_name": "SessionEnd", "reason": "exit"}), "gone", None),
    ];
    for (payload, _, _) in &cases {
        test.run_hook(with_session(payload.clone(), "/x/proj"), &[]);
    }
    let lines = test.received();
    let states: Vec<&str> = lines.iter().map(|line| line["state"].as_str().unwrap()).collect();
    assert_eq!(states, cases.iter().map(|(_, state, _)| *state).collect::<Vec<_>>());
    let stickies: Vec<Option<bool>> = lines.iter().map(|line| line.get("sticky").map(|sticky| sticky.as_bool().unwrap())).collect();
    assert_eq!(stickies, cases.iter().map(|(_, _, sticky)| *sticky).collect::<Vec<_>>());
    assert_eq!(lines[1]["task"], "add the chip");
    assert_eq!(lines[0]["tmux_pane"], "%3");
}

#[test]
fn task_label_wrapper_reports_only_session_metadata_for_both_agents() {
    for session in [SESSION_A, OPENCODE_SESSION] {
        let mut test = HookTest::new();
        let output = test.sandbox.run("bash", &[bin().join("claude-session-label").to_str().unwrap(), "  Free   disk space  "], &[("CLAUDE_CODE_SESSION_ID", session)]);
        assert!(output.status.success(), "{}", stderr(&output));
        let report = test.received().remove(0);
        assert_eq!(report["label"], "Free disk space");
        assert_eq!(report["session"], session);
        for field in ["state", "message", "task", "needs_input"] {
            assert!(report.get(field).is_none(), "label overwrites {field}: {report}");
        }
    }
}

#[test]
fn task_label_reports_reject_invalid_names_without_sending_anything() {
    let mut test = HookTest::new();
    for label in ["Two words".into(), "one two three four five six seven".into(), "a".repeat(57) + " b c", "Free\ndisk space".into(), "Read #(id) output".into()] {
        let output = test.run_hook(with_session(json!({"hook_event_name": "TaskLabel", "label": label}), "/x/proj"), &[]);
        assert_eq!(output.status.code(), Some(2));
    }
    assert!(test.received().is_empty());
}

#[test]
fn task_label_reporting_does_not_clear_a_pending_question() {
    let mut test = HookTest::new();
    for payload in [json!({"hook_event_name": "NeedsInput", "message": "Which option?"}),
                    json!({"hook_event_name": "TaskLabel", "label": "Fix session navigation"}),
                    json!({"hook_event_name": "Stop"})] {
        let output = test.run_hook(with_session(payload, "/x/proj"), &[]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let reports = test.received();
    assert_eq!(reports[1]["label"], "Fix session navigation");
    assert!(reports[1].get("needs_input").is_none());
    assert_eq!(reports[2]["needs_input"], true);
    assert_eq!(reports[2]["message"], "Which option?");
}

#[test]
fn task_label_rejections_fail_without_touching_existing_question_toasts() {
    for daemon_missing in [false, true] {
        let test = HookTest::with_reply("refused: no session target");
        if daemon_missing { fs::remove_file(&test.socket).unwrap(); }
        let toast = test.sandbox.home.join(format!(".cache/claude/toast-{SESSION_A}.id"));
        fs::create_dir_all(toast.parent().unwrap()).unwrap();
        fs::write(&toast, "42").unwrap();
        let output = test.sandbox.run("bash", &[bin().join("claude-session-label").to_str().unwrap(), "Fix session navigation"], &[("CLAUDE_CODE_SESSION_ID", SESSION_A)]);
        assert!(!output.status.success(), "label publication falsely succeeded");
        assert_eq!(fs::read_to_string(&toast).unwrap(), "42");
        assert!(test.sandbox.calls("gdbus").is_empty());
        assert!(test.sandbox.calls("notify-send").is_empty());
    }
}

#[test]
fn informational_notification_carries_no_state() {
    let mut test = HookTest::new();
    test.run_hook(
        with_session(json!({"hook_event_name": "Notification", "message": "ok", "notification_type": "auth_success"}), "/x/proj"),
        &[],
    );
    let line = test.received().remove(0);
    assert!(line.get("state").is_none(), "{line}");
    assert_eq!(line["message"], "ok");
}

#[test]
fn an_explicit_question_waits_for_turn_end_for_both_agents() {
    for session in [SESSION_A, OPENCODE_SESSION] {
        let mut test = HookTest::new();
        let result = test.sandbox.run("bash", &[bin().join("claude-needs-input").to_str().unwrap(), "Which option should I use?"],
            &[("CLAUDE_CODE_SESSION_ID", session)]);
        assert!(result.status.success(), "{}", stderr(&result));
        for event in ["PostToolUse", "Notification", "Stop", "PostToolUse"] {
            let mut payload = with_session(json!({"hook_event_name": event, "message": "Still running", "notification_type": "auth_success"}), "/x/proj");
            payload["session_id"] = json!(session);
            let result = test.run_hook(payload, &[("COCKPIT_AGENT", if session == SESSION_A { "claude" } else { "opencode" })]);
            assert!(result.status.success(), "{}", stderr(&result));
        }
        let lines = test.received();
        assert_eq!(lines.len(), 5);
        for line in &lines[..3] {
            assert_ne!(line["state"], "needs-you");
            assert_ne!(line["needs_input"], true);
            assert_ne!(line["message"], "Which option should I use?");
        }
        for line in &lines[3..] {
            assert_eq!(line["state"], "needs-you");
            assert_eq!(line["message"], "Which option should I use?");
            assert_eq!(line["needs_input"], true);
            assert_eq!(line["agent"].as_str().unwrap_or("claude"), if session == SESSION_A { "claude" } else { "opencode" });
        }
    }
}

#[test]
fn a_pending_question_does_not_delay_a_permission_prompt() {
    let mut test = HookTest::new();
    for payload in [json!({"hook_event_name": "NeedsInput", "message": "Which option?"}),
        json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Allow Bash?"}),
        json!({"hook_event_name": "Stop"})] {
        let result = test.run_hook(with_session(payload, "/x/proj"), &[]);
        assert!(result.status.success(), "{}", stderr(&result));
    }
    let lines = test.received();
    assert_eq!(lines[1]["state"], "needs-you");
    assert_eq!(lines[1]["message"], "Allow Bash?");
    assert_eq!(lines[2]["message"], "Which option?");
    assert_eq!(lines[2]["needs_input"], true);
}

#[test]
fn a_real_dialog_overrides_an_already_published_chat_question() {
    for notification in ["permission_prompt", "worker_permission_prompt", "elicitation_dialog", "elicitation_url_dialog", "agent_needs_input"] {
        let mut test = HookTest::new();
        for payload in [json!({"hook_event_name": "NeedsInput", "message": "Which option?"}),
            json!({"hook_event_name": "Stop"}),
            json!({"hook_event_name": "Notification", "notification_type": notification, "message": "Allow tool?"})] {
            let result = test.run_hook(with_session(payload, "/x/proj"), &[]);
            assert!(result.status.success(), "{}", stderr(&result));
        }
        let lines = test.received();
        assert_eq!(lines[2]["needs_input"], false, "{notification}: {}", lines[2]);
        assert_eq!(lines[2]["message"], "Allow tool?");
    }
}

#[test]
fn lifecycle_resets_clear_an_explicit_question() {
    for event in ["UserPromptSubmit", "SessionStart", "SessionEnd"] {
        for has_finished in [false, true] {
            let mut test = HookTest::new();
            let result = test.run_hook(with_session(json!({"hook_event_name": "NeedsInput", "message": "Which option?"}), "/x/proj"), &[]);
            assert!(result.status.success(), "{}", stderr(&result));
            if has_finished {
                let result = test.run_hook(with_session(json!({"hook_event_name": "Stop"}), "/x/proj"), &[]);
                assert!(result.status.success(), "{}", stderr(&result));
            }
            for payload in [json!({"hook_event_name": event, "prompt": "Use option A"}), json!({"hook_event_name": "Stop"})] {
                let result = test.run_hook(with_session(payload, "/x/proj"), &[]);
                assert!(result.status.success(), "{event}: {}", stderr(&result));
            }
            let lines = test.received();
            let reset_index = if has_finished { 2 } else { 1 };
            assert_eq!(lines[reset_index]["needs_input"], false, "{event}");
            assert_eq!(lines[reset_index + 1]["state"], "waiting", "{event}");
            assert_eq!(lines[reset_index + 1]["needs_input"], false, "{event}");
        }
    }
}

#[test]
fn an_explicit_question_does_not_leak_to_another_session() {
    let mut test = HookTest::new();
    test.run_hook(with_session(json!({"hook_event_name": "NeedsInput", "message": "Which option?"}), "/x/proj"), &[]);
    let mut payload = with_session(json!({"hook_event_name": "Stop"}), "/x/proj");
    payload["session_id"] = json!(SESSION_B);
    test.run_hook(payload, &[]);
    let lines = test.received();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["state"], "waiting");
    assert_eq!(lines[1]["needs_input"], false);
}

#[test]
fn an_idle_notification_does_not_clear_an_explicit_question() {
    let mut test = HookTest::new();
    test.run_hook(with_session(json!({"hook_event_name": "NeedsInput", "message": "Which option?"}), "/x/proj"), &[]);
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), "/x/proj"), &[]);
    test.run_hook(with_session(json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "idle"}), "/x/proj"), &[]);
    let lines = test.received();
    assert_eq!(lines[2]["state"], "needs-you");
    assert_eq!(lines[2]["message"], "Which option?");
}

#[test]
fn a_system_prompt_does_not_answer_an_explicit_question() {
    let mut test = HookTest::new();
    for payload in [json!({"hook_event_name": "NeedsInput", "message": "Which option?"}),
        json!({"hook_event_name": "Stop"}),
        json!({"hook_event_name": "UserPromptSubmit", "source": "system", "prompt": "Background task finished"}),
        json!({"hook_event_name": "Stop"})] {
        let result = test.run_hook(with_session(payload, "/x/proj"), &[]);
        assert!(result.status.success(), "{}", stderr(&result));
    }
    for line in test.received().into_iter().skip(1) {
        assert_eq!(line["state"], "needs-you");
        assert_eq!(line["message"], "Which option?");
    }
}

#[test]
fn the_question_command_requires_the_daemons_acceptance() {
    let test = HookTest::new();
    for reply in ["", "refused: session.cwd contains a tmux format sequence"] {
        test.sandbox.shim("socat", &format!("cat >/dev/null\nprintf '%s\\n' '{reply}'"));
        let result = test.sandbox.run("bash", &[bin().join("claude-needs-input").to_str().unwrap(), "Which option?"],
            &[("CLAUDE_CODE_SESSION_ID", SESSION_A)]);
        assert_eq!(result.status.code(), Some(1), "{}", stderr(&result));
    }
}

#[test]
fn the_question_command_rejects_invalid_input() {
    let test = HookTest::new();
    let long_question = "a".repeat(301);
    for (session, question) in [("", "Which option?"), ("../../other", "Which option?"), (SESSION_A, "  "),
        (SESSION_A, "line\nbreak"), (SESSION_A, "#{pane_id}"), (SESSION_A, long_question.as_str())] {
        let result = test.sandbox.run("bash", &[bin().join("claude-needs-input").to_str().unwrap(), question],
            &[("CLAUDE_CODE_SESSION_ID", session)]);
        assert_eq!(result.status.code(), Some(2), "{}", stderr(&result));
    }
}

#[test]
fn the_question_command_reports_a_missing_daemon() {
    let test = HookTest::new();
    fs::remove_file(&test.socket).unwrap();
    let result = test.sandbox.run("bash", &[bin().join("claude-needs-input").to_str().unwrap(), "Which option?"],
        &[("CLAUDE_CODE_SESSION_ID", SESSION_A)]);
    assert_eq!(result.status.code(), Some(1), "{}", stderr(&result));
    assert!(stderr(&result).contains("osd"), "{}", stderr(&result));
}

#[test]
fn unknown_event_fails_loudly() {
    let test = HookTest::new();
    let result = test.run_hook(with_session(json!({"hook_event_name": "PreCompact"}), "/x"), &[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("unexpected hook event"), "{}", stderr(&result));
}

#[test]
fn nested_claude_reports_nothing() {
    let mut test = HookTest::new();
    // A stand-in process named "claude" that a tool started (CLAUDECODE=1 in its environment),
    // between the hook and its real ancestors.
    let fake = fake_claude(&test.sandbox.root);
    let script = format!("bash {}; :", hook().display());
    let payload = with_session(json!({"hook_event_name": "Stop"}), "/x/.issues-tmp");
    test.sandbox.run_with_input(&fake, &["-c", &script], &[("CLAUDECODE", "1")], &payload.to_string());
    assert_eq!(test.received(), Vec::<Value>::new());
}

#[test]
fn a_session_nested_in_an_opencode_tool_reports_nothing() {
    // opencode marks the processes its tools run with OPENCODE=1.
    let mut test = HookTest::new();
    let fake = fake_agent(&test.sandbox.root, "opencode");
    let script = format!("bash {}; :", hook().display());
    let payload = with_session(json!({"hook_event_name": "Stop"}), "/x/proj");
    test.sandbox.run_with_input(&fake, &["-c", &script], &[("OPENCODE", "1"), ("COCKPIT_AGENT", "opencode")], &payload.to_string());
    assert_eq!(test.received(), Vec::<Value>::new());
}

#[test]
fn an_opencode_session_started_from_a_shell_reports_its_agent() {
    use std::io::Write;
    let mut test = HookTest::new();
    let fake = fake_agent(&test.sandbox.root, "opencode");
    let script = format!("bash {}; :", hook().display());
    let mut payload = json!({"hook_event_name": "SessionStart", "session_id": "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L", "cwd": "/x/proj"});
    // Unmarked, like a session you start (these tests may themselves run under an agent's tool).
    let mut child = test.sandbox.command(&fake, &[("COCKPIT_AGENT", "opencode")]).env_remove("CLAUDECODE").env_remove("OPENCODE")
        .args(["-c", &script]).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    child.wait().unwrap();
    payload["session_id"] = json!(SESSION_A);
    test.run_hook(payload, &[]);
    let lines = test.received();
    assert_eq!((&lines[0]["session"], &lines[0]["agent"], &lines[0]["state"]), (&json!("ses_3f2a9c0b1ffeQkTz8mWn4rJd2L"), &json!("opencode"), &json!("waiting")));
    assert!(lines[1].get("agent").is_none(), "a Claude report carries no agent: {}", lines[1]);
}

#[test]
fn a_session_started_from_a_shell_reports_even_under_the_daemon() {
    use std::io::Write;
    let mut test = HookTest::new();
    test.sandbox.env.insert("OPENCODE".into(), "1".into());
    // Since Claude Code 2.1.300 a session sits under processes named claude (launcher, daemon)
    // that carry no CLAUDECODE mark: two of them must not look like nesting.
    let fake = fake_claude(&test.sandbox.root);
    // The daemon scrubs the session's environment: the pane and the kitty window come from
    // the launcher above (the outer fake claude here).
    let script = format!("env -u TMUX_PANE -u KITTY_PID {} -c 'bash {}'; :", fake.display(), hook().display());
    let mut child = test
        .sandbox
        .command(&fake, &[("TMUX_PANE", "%9"), ("KITTY_PID", "4242")])
        .env_remove("CLAUDECODE")
        .env_remove("OPENCODE")
        .args(["-c", &script])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = with_session(json!({"hook_event_name": "Stop"}), "/x/proj");
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    child.wait().unwrap();
    let lines = test.received();
    let states: Vec<&str> = lines.iter().map(|line| line["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["waiting"]);
    assert_eq!((&lines[0]["tmux_pane"], &lines[0]["kitty_pid"]), (&json!("%9"), &json!("4242")));
}

#[test]
fn each_event_leaves_a_line_in_the_hook_log() {
    let mut test = HookTest::new();
    test.run_hook(
        with_session(json!({"hook_event_name": "UserPromptSubmit", "prompt": "the secret plan", "source": "user"}), "/x/proj"),
        &[],
    );
    test.received();
    let log = fs::read_to_string(test.sandbox.home.join(".cache/claude/hook.log")).unwrap();
    let line = log.lines().last().unwrap();
    assert!(regex::Regex::new(r" UserPromptSubmit aaaaaaaa working: sent to the osd daemon$").unwrap().is_match(line), "{line}");
    assert!(!line.contains("secret"), "{line}");
}

#[test]
fn fallback_toast_when_the_daemon_is_unreachable() {
    let test = HookTest::new();
    fs::remove_file(&test.socket).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), "/x/proj"), &[]);
    assert_eq!(test.sandbox.calls("notify-send").len(), 1);
    let toast_id = test.sandbox.home.join(format!(".cache/claude/toast-{SESSION_A}.id"));
    assert_eq!(fs::read_to_string(&toast_id).unwrap().trim(), "42");
    test.run_hook(with_session(json!({"hook_event_name": "UserPromptSubmit", "prompt": "x"}), "/x/proj"), &[]);
    assert_eq!(test.sandbox.calls("gdbus").len(), 1);
    assert!(!toast_id.exists());
}

fn git(repository: &Path, arguments: &[&str]) {
    let status = Command::new("git").arg("-C").arg(repository).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(arguments)
        .output().unwrap().status;
    assert!(status.success(), "git {arguments:?}");
}

#[test]
fn a_finished_turn_reports_its_commits_and_uncommitted_changes() {
    // Worked out by the hook, inside the session's sandbox: git reads configuration the session
    // can write, so the daemon outside never runs it.
    let mut test = HookTest::new();
    let repository = test.sandbox.root.join("repo");
    fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-q"]);
    fs::write(repository.join("a.txt"), "one\n").unwrap();
    git(&repository, &["add", "a.txt"]);
    git(&repository, &["commit", "-qm", "start"]);
    let cwd = repository.to_string_lossy().into_owned();
    test.run_hook(with_session(json!({"hook_event_name": "UserPromptSubmit", "prompt": "work"}), &cwd), &[]);
    fs::write(repository.join("b.txt"), "two\n").unwrap();
    git(&repository, &["add", "b.txt"]);
    git(&repository, &["commit", "-qm", "work"]);
    fs::write(repository.join("a.txt"), "one\nmore\nlines\n").unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), &cwd), &[]);
    let reports = test.received();
    assert_eq!(reports.last().unwrap()["outcome"], "1 commit · 1 file changed, +2 −0 uncommitted");
}

#[test]
fn a_turn_outside_a_repository_reports_no_outcome() {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), &folder.to_string_lossy()), &[]);
    assert!(test.received().last().unwrap().get("outcome").is_none());
}

/// A transcript: a prompt, then each (command, failed) as a Bash call and its result.
fn transcript(folder: &Path, turns: &[(&str, &[(&str, bool)])]) -> String {
    let mut lines = Vec::new();
    for (turn, (prompt, calls)) in turns.iter().enumerate() {
        lines.push(json!({"type": "user", "message": {"role": "user", "content": prompt}}));
        for (index, (command, failed)) in calls.iter().enumerate() {
            let id = format!("toolu_{turn}_{index}");
            lines.push(json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]}}));
            lines.push(json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": failed, "content": "…"}]}}));
        }
    }
    let path = folder.join("transcript.jsonl");
    fs::write(&path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    path.to_string_lossy().into_owned()
}

fn stop_outcome(turns: &[(&str, &[(&str, bool)])]) -> Option<String> {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    let mut payload = with_session(json!({"hook_event_name": "Stop"}), &folder.to_string_lossy());
    payload["transcript_path"] = json!(transcript(&folder, turns));
    test.run_hook(payload, &[]);
    test.received().last().unwrap().get("outcome").and_then(Value::as_str).map(str::to_string)
}

#[test]
fn a_turn_whose_last_test_run_failed_says_so() {
    let outcome = stop_outcome(&[("fix it", &[("make test", false), ("cargo test -p vitals", true)])]);
    assert_eq!(outcome.as_deref(), Some("tests failed"));
}

#[test]
fn a_turn_that_passed_its_tests_and_closed_a_todo_says_both() {
    let outcome = stop_outcome(&[("fix it", &[("ask-to-run --why 'x' 'make cockpit; and make test'", false), ("claude-todo done 8e50f7a0", false)])]);
    assert_eq!(outcome.as_deref(), Some("tests passed · todo done"));
}

#[test]
fn tests_of_an_earlier_turn_do_not_count() {
    let outcome = stop_outcome(&[("first", &[("make test", true)]), ("second", &[("ls", false)])]);
    assert_eq!(outcome, None);
}

#[test]
fn a_test_command_only_mentioned_in_text_does_not_count() {
    let outcome = stop_outcome(&[("write the test", &[("cat >> t.rs <<'X'\nstop_outcome(&[(\"make test\", true), (\"claude-todo done 1\", false)]);\nX", false)])]);
    assert_eq!(outcome, None);
}

#[test]
fn a_session_started_in_its_own_terminal_says_so() {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "SessionStart"}), &folder.to_string_lossy()), &[("COCKPIT_TERMINAL", "1")]);
    test.run_hook(with_session(json!({"hook_event_name": "SessionStart"}), &folder.to_string_lossy()), &[]);
    let reports = test.received();
    assert_eq!(reports[0]["terminal"], true);
    assert!(reports[1].get("terminal").is_none(), "a cockpit session carries no flag");
}

#[test]
fn every_report_says_whether_the_session_bills_api_credits() {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "SessionStart"}), &folder.to_string_lossy()), &[("COCKPIT_CREDITS", "1")]);
    test.run_hook(with_session(json!({"hook_event_name": "SessionStart"}), &folder.to_string_lossy()), &[]);
    let reports = test.received();
    assert_eq!((&reports[0]["credits"], &reports[1]["credits"]), (&json!(true), &json!(false)));
}

#[test]
fn a_finished_turn_counts_the_commits_not_on_a_remote() {
    let mut test = HookTest::new();
    let remote = test.sandbox.root.join("remote.git");
    let repository = test.sandbox.root.join("repo");
    fs::create_dir(&repository).unwrap();
    assert!(Command::new("git").args(["init", "-q", "--bare"]).arg(&remote).status().unwrap().success());
    git(&repository, &["init", "-q"]);
    git(&repository, &["remote", "add", "origin", remote.to_str().unwrap()]);
    fs::write(repository.join("a"), "1\n").unwrap();
    git(&repository, &["add", "a"]);
    git(&repository, &["commit", "-qm", "pushed"]);
    git(&repository, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    git(&repository, &["fetch", "-q", "origin"]);
    for name in ["b", "c"] {
        fs::write(repository.join(name), "1\n").unwrap();
        git(&repository, &["add", name]);
        git(&repository, &["commit", "-qm", name]);
    }
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), &repository.to_string_lossy()), &[]);
    assert_eq!(test.received().last().unwrap()["unpushed"], 2);
}

#[test]
fn a_clone_of_a_running_copy_counts_the_commits_live_main_lacks_not_the_unpushed_ones() {
    // cockpit apply brings a commit over with git am, under a new hash, and the clone's
    // origin/main is only as fresh as its last fetch: counted by change, the applied one is done.
    for repository in ["dotfiles", "agent-cockpit", "vitals"] {
        let mut test = HookTest::new();
        let live = test.sandbox.home.join(repository);
        let clone = test.sandbox.root.join("clone");
        fs::create_dir_all(&live).unwrap();
        git(&live, &["init", "-q", "-b", "main"]);
        fs::write(live.join("a"), "1\n").unwrap();
        git(&live, &["add", "a"]);
        git(&live, &["commit", "-qm", "base"]);
        assert!(Command::new("git").args(["clone", "-q"]).arg(&live).arg(&clone).status().unwrap().success());
        for name in ["applied", "waiting"] {
            fs::write(clone.join(name), "1\n").unwrap();
            git(&clone, &["add", name]);
            git(&clone, &["commit", "-qm", name]);
        }
        let patch = Command::new("git").arg("-C").arg(&clone).args(["format-patch", "-1", "HEAD~1", "--stdout"]).output().unwrap().stdout;
        let patch_file = test.sandbox.root.join("applied.patch");
        fs::write(&patch_file, patch).unwrap();
        git(&live, &["am", "-q", patch_file.to_str().unwrap()]);
        test.run_hook(with_session(json!({"hook_event_name": "Stop"}), &clone.to_string_lossy()), &[]);
        assert_eq!(test.received().last().unwrap()["unpushed"], 1, "{repository}");
    }
}

#[test]
fn a_repository_without_a_remote_reports_no_count() {
    let mut test = HookTest::new();
    let repository = test.sandbox.root.join("repo");
    fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-q"]);
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), &repository.to_string_lossy()), &[]);
    assert!(test.received().last().unwrap().get("unpushed").is_none());
}

fn apps_reported(port: u16) -> Option<String> {
    let mut test = HookTest::new();
    test.sandbox.shim("ps", &format!("echo 'figcraft-cli mcp --attach {port}'"));
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "PostToolUse"}), &folder.to_string_lossy()), &[]);
    test.received().last().unwrap().get("apps").and_then(Value::as_str).map(str::to_string)
}

#[test]
fn a_craft_app_that_answers_is_reported_on() {
    let app = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    assert_eq!(apps_reported(app.local_addr().unwrap().port()).as_deref(), Some("figcraft:on"));
}

#[test]
fn a_craft_app_that_is_closed_is_reported_off() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert_eq!(apps_reported(port).as_deref(), Some("figcraft:off"));
}

/// The counts the hook reports for an event: (agents, background), absent when it says nothing.
fn background_reported(payload: Value) -> (Option<u64>, Option<u64>) {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    test.run_hook(with_session(payload, &folder.to_string_lossy()), &[]);
    let report = test.received().last().unwrap().clone();
    (report.get("agents").and_then(Value::as_u64), report.get("background").and_then(Value::as_u64))
}

#[test]
fn a_finished_turn_counts_its_background_agents_apart_from_other_background_work() {
    let tasks = json!([{"type": "subagent"}, {"type": "subagent"}, {"type": "teammate"}, {"type": "shell"}, {"type": "monitor"}, {"type": "dream"}]);
    assert_eq!(background_reported(json!({"hook_event_name": "Stop", "background_tasks": tasks})), (Some(3), Some(2)));
}

#[test]
fn a_finished_turn_without_a_task_list_reports_no_count() {
    assert_eq!(background_reported(json!({"hook_event_name": "Stop"})), (None, None));
}

#[test]
fn a_fresh_or_resumed_session_has_no_background_work_and_a_cleared_one_keeps_its_own() {
    assert_eq!(background_reported(json!({"hook_event_name": "SessionStart", "source": "resume"})), (Some(0), Some(0)));
    assert_eq!(background_reported(json!({"hook_event_name": "SessionStart", "source": "clear"})), (None, None));
}
