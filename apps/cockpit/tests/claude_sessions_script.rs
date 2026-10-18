//! The session manager script (config/bin/.local/bin/claude-sessions) against a fake tmux.

mod common;
use common::sandbox::*;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::json;

// The manager's panes are found by their @role tag; the fake keeps those tags in $SHIM_ROLES.
const FAKE_TMUX: &str = r#"
roles="${SHIM_ROLES:-/dev/null}"
case $1 in
  list-panes)
    if [[ $2 == -t && $3 == =claude-retry-* ]]; then echo "%4"
    elif [[ $* == *@role* ]]; then cat "$roles" 2>/dev/null
    else printf '0\n1\n'; fi ;;
  has-session) [[ $3 == "=claude-manager" ]] && exit 0; (( $(grep -c "send-keys" "$SHIM_LOG") < ${SHIM_EXIT_AFTER:-1} )) ;;
  display-message) if [[ $* == *pane_dead* ]]; then echo 0; elif [[ $* == *pane_active* ]]; then echo "${SHIM_VIEWER_ACTIVE:-0}"; elif [[ $* == *pane_id* ]]; then echo "%10"; else echo "claude-retry-9-9"; fi ;;
  split-window) [[ $* == *-P* ]] && echo "%1$(( $(wc -l < "$roles" 2>/dev/null || echo 0) + 1 ))" ;;
  set-option) if [[ $2 == -p && $5 == @role ]]; then echo "$4 $6" >> "$roles"; fi ;;
  capture-pane) printf '%b' "$SHIM_PANE_TEXT" ;;
  list-clients) echo "/dev/pts/9" ;;
  kill-pane) if [[ -n $SHIM_PANE_GONE ]]; then echo "can't find pane: $3" >&2; exit 1; fi ;;
esac
"#;

const IDLE: &str = "done\n❯ \n";
const BUSY: &str = "✻ Flowing… (11s)\n❯ \n";
const BUSY_WITHOUT_PROMPT: &str = "✻ Flowing… (11s)\n";
const DIALOG: &str = " Bash command\n   npm test\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n";

struct Manager {
    sandbox: Sandbox,
    project: PathBuf,
    snapshot: PathBuf,
    roles: PathBuf,
}

impl Manager {
    fn new() -> Manager {
        let mut sandbox = Sandbox::new();
        sandbox.shim("tmux", FAKE_TMUX);
        let roles = sandbox.root.join("roles");
        sandbox.env.insert("SHIM_ROLES".into(), roles.display().to_string());
        // A manager whose panes are already tagged, as ensure_manager leaves them.
        fs::write(&roles, "%10 sidebar\n%11 viewer\n").unwrap();
        sandbox.shim("hyprctl", r#"[[ $1 == -j && $2 == activeworkspace ]] && echo "{\"id\": ${SHIM_WS:-2}}"; exit 0"#);
        sandbox.shim("setsid", "");
        sandbox.shim("osd-show", "");
        let project = sandbox.root.join("proj-a");
        fs::create_dir(&project).unwrap();
        let snapshot = sandbox.runtime.join("claude-sessions.json");
        fs::write(&snapshot, json!({"sessions": [
            {"session": SESSION_A, "state": "waiting", "project": "proj-a", "cwd": project.display().to_string(), "tmux_pane": "%4"}]}).to_string()).unwrap();
        // Session A has a conversation to resume (a transcript with a user message).
        let transcripts = sandbox.home.join(".claude/projects/-proj-a");
        fs::create_dir_all(&transcripts).unwrap();
        fs::write(transcripts.join(format!("{SESSION_A}.jsonl")), "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n").unwrap();
        Manager { sandbox, project, snapshot, roles }
    }

    fn run(&self, arguments: &[&str], env: &[(&str, &str)]) -> Output {
        self.sandbox.run(bin().join("claude-sessions"), arguments, env)
    }

    fn calls(&self, name: &str) -> Vec<String> {
        self.sandbox.calls(name)
    }

    fn tmux_calls(&self, verb: &str) -> Vec<String> {
        let prefix = format!("tmux {verb}");
        self.calls("tmux").into_iter().filter(|call| call.starts_with(&prefix)).collect()
    }

    fn runtime_file(&self, name: &str) -> PathBuf {
        self.sandbox.runtime.join(name)
    }

    fn log_lines(&self) -> Vec<String> {
        fs::read_to_string(self.sandbox.state.join("claude-sessions/log")).map(|text| text.lines().map(str::to_string).collect()).unwrap_or_default()
    }

    fn statusline_cache(&self) -> PathBuf {
        let cache = self.sandbox.home.join(".cache/claude");
        fs::create_dir_all(&cache).unwrap();
        cache
    }

    fn doctor_line(&self, needle: &str, env: &[(&str, &str)]) -> String {
        stdout(&self.run(&["doctor"], env)).lines().find(|line| line.contains(needle)).unwrap_or_else(|| panic!("no doctor line with {needle}")).to_string()
    }

    fn prs_shims(&self, special: &str) {
        self.sandbox.shim("hyprctl", &format!(r#"case "$*" in
  "-j activeworkspace") echo "{{\"id\": ${{SHIM_WS:-6}}}}" ;;
  "-j monitors") echo '[{{"focused": true, "specialWorkspace": {{"name": "{special}"}}}}]' ;;
esac"#));
        fs::write(self.runtime_file("claude-sessions.current"), format!("{SESSION_A}\n")).unwrap();
    }

    /// self.project as a real git repository with one commit and a GitHub origin.
    fn git_repository(&self) {
        let git = |arguments: &[&str]| {
            let output = Command::new("git").arg("-C").arg(&self.project).args(arguments).output().unwrap();
            assert!(output.status.success(), "git {arguments:?}: {}", stderr(&output));
        };
        git(&["init", "-q", "-b", "main"]);
        fs::write(self.project.join("README"), "x\n").unwrap();
        git(&["add", "README"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]);
        git(&["remote", "add", "origin", "git@github.com:o/proj-a.git"]);
    }

    fn resume_call(&self) -> String {
        self.calls("tmux").into_iter().find(|call| call.contains("claude --resume $argv")).expect("no resume")
    }
}

fn path_str(path: &Path) -> String {
    path.display().to_string()
}

fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn git_output(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(directory).args(arguments).output().unwrap();
    stdout(&output).trim().to_string()
}

#[test]
fn view_attaches_the_session_nested_in_the_viewer() {
    let manager = Manager::new();
    assert_eq!(manager.run(&["view", SESSION_A], &[]).status.code(), Some(0));
    assert_eq!(manager.tmux_calls("respawn-pane"), ["tmux respawn-pane -k -t %11 env -u TMUX tmux attach-session -t =claude-retry-9-9"]);
    assert_eq!(fs::read_to_string(manager.runtime_file("claude-sessions.current")).unwrap().trim(), SESSION_A);
}

#[test]
fn view_without_a_pane_fails_with_a_message() {
    let manager = Manager::new();
    let result = manager.run(&["view", "dddddddd-0000-4000-8000-000000000000"], &[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("has no tmux pane"));
}

#[test]
fn label_state_colours_the_viewer_title_and_refuses_nonsense() {
    let manager = Manager::new();
    assert_eq!(manager.run(&["label-state", "needs-you"], &[]).status.code(), Some(0));
    assert!(manager.calls("tmux").contains(&"tmux set-option -p -t %11 @state needs-you".to_string()));
    assert_eq!(manager.run(&["label-state", "purple"], &[]).status.code(), Some(1));
}

#[test]
fn open_focuses_the_viewer_after_viewing() {
    let manager = Manager::new();
    manager.run(&["open", SESSION_A], &[]);
    assert!(manager.calls("tmux").contains(&"tmux select-pane -t %11".to_string()));
}

#[test]
fn plain_new_reuses_the_projects_running_session() {
    let manager = Manager::new();
    manager.run(&["new", &path_str(&manager.project)], &[]);
    let respawns = manager.tmux_calls("respawn-pane");
    assert!(respawns.iter().any(|call| call.contains("attach-session -t =claude-retry-9-9")));
    assert!(!respawns.iter().any(|call| call.contains("fish -ic")));
}

#[test]
fn new_with_arguments_or_another_project_launches_fresh() {
    let manager = Manager::new();
    let other = manager.sandbox.root.join("proj-b");
    fs::create_dir(&other).unwrap();
    manager.run(&["new", &path_str(&manager.project), "--resume"], &[]);
    manager.run(&["new", &path_str(&other)], &[]);
    let launches: Vec<String> = manager.tmux_calls("respawn-pane").into_iter().filter(|call| call.contains("fish -ic")).collect();
    assert_eq!(launches.len(), 2);
    assert!(launches[0].contains("CLAUDE_SESSIONS_INSIDE=1"));
    assert!(launches[0].ends_with("--resume"));
}

#[test]
fn new_refuses_a_directory_with_a_tmux_format() {
    let manager = Manager::new();
    let weird = manager.sandbox.root.join("a#b");
    fs::create_dir(&weird).unwrap();
    assert_eq!(manager.run(&["new", &path_str(&weird)], &[]).status.code(), Some(1));
}

#[test]
fn exit_types_slash_exit_at_an_idle_prompt() {
    let manager = Manager::new();
    manager.run(&["exit", SESSION_A], &[("SHIM_PANE_TEXT", IDLE), ("SHIM_EXIT_AFTER", "1")]);
    assert_eq!(manager.tmux_calls("send-keys"), ["tmux send-keys -t %4 -l -- /exit", "tmux send-keys -t %4 Enter"]);
    assert!(manager.tmux_calls("kill-session").is_empty());
}

#[test]
fn prompt_commands_pause_between_the_text_and_its_enter() {
    // Sent in one burst, Claude reads the text and Enter as a paste: the Enter becomes a newline,
    // /exit never runs and every restart ended in kill-session ("did not exit").
    let manager = Manager::new();
    manager.sandbox.shim("sleep", r#"exec /usr/bin/sleep "$@""#);
    for (command, text) in [("exit", "/exit"), ("compact", "/compact")] {
        manager.sandbox.reset_log();
        manager.run(&[command, SESSION_A], &[("SHIM_PANE_TEXT", IDLE), ("SHIM_EXIT_AFTER", "1")]);
        let log = fs::read_to_string(&manager.sandbox.log).unwrap();
        let typed: Vec<&str> = log.lines().filter(|line| line.starts_with("tmux send-keys") || line.starts_with("sleep")).take(3).collect();
        let expected = [format!("tmux send-keys -t %4 -l -- {text}"), "sleep 0.15".into(), "tmux send-keys -t %4 Enter".into()];
        assert_eq!(typed, expected, "command {command}");
    }
}

#[test]
fn exit_sends_two_quick_ctrl_c_while_busy() {
    let manager = Manager::new();
    manager.run(&["exit", SESSION_A], &[("SHIM_PANE_TEXT", BUSY), ("SHIM_EXIT_AFTER", "2")]);
    assert_eq!(manager.tmux_calls("send-keys"), ["tmux send-keys -t %4 C-c"; 2]);
}

#[test]
fn exit_kills_the_tmux_session_as_a_last_resort() {
    let manager = Manager::new();
    let result = manager.run(&["exit", SESSION_A], &[("SHIM_PANE_TEXT", BUSY_WITHOUT_PROMPT), ("SHIM_EXIT_AFTER", "99")]);
    assert_eq!(manager.tmux_calls("kill-session"), ["tmux kill-session -t =claude-retry-9-9"]);
    assert!(stderr(&result).contains("did not exit"));
}

#[test]
fn restart_exits_then_resumes_the_same_conversation() {
    let manager = Manager::new();
    manager.run(&["restart", SESSION_A], &[("SHIM_PANE_TEXT", IDLE), ("SHIM_EXIT_AFTER", "1")]);
    let calls = manager.calls("tmux");
    let exit_index = calls.iter().position(|call| call == "tmux send-keys -t %4 -l -- /exit").expect("no /exit");
    let resume = calls.iter().position(|call| call.contains("claude --resume $argv")).expect("no resume");
    assert!(exit_index < resume);
    assert!(calls[resume].ends_with(SESSION_A));
}

#[test]
fn restart_with_an_effort_resumes_on_it() {
    let manager = Manager::new();
    manager.run(&["restart", SESSION_A, "max"], &[("SHIM_PANE_TEXT", IDLE), ("SHIM_EXIT_AFTER", "1")]);
    assert!(manager.resume_call().ends_with(&format!("{SESSION_A} --effort max")));
}

#[test]
fn restart_keeps_the_effort_the_status_line_reported() {
    let manager = Manager::new();
    fs::write(manager.statusline_cache().join(format!("statusline-{SESSION_A}.json")), r#"{"effort": "xhigh"}"#).unwrap();
    manager.run(&["restart", SESSION_A], &[("SHIM_PANE_TEXT", IDLE), ("SHIM_EXIT_AFTER", "1")]);
    assert!(manager.resume_call().ends_with(&format!("{SESSION_A} --effort xhigh")));
}

#[test]
fn restart_of_a_session_nobody_typed_in_starts_a_fresh_one() {
    let manager = Manager::new();
    let transcripts = manager.sandbox.root.join("transcripts");
    fs::create_dir_all(transcripts.join("-x")).unwrap();
    fs::write(transcripts.join("-x").join(format!("{SESSION_A}.jsonl")), "{\"type\":\"summary\"}\n").unwrap();
    manager.run(&["restart", SESSION_A, "max"], &[("SHIM_PANE_TEXT", IDLE), ("SHIM_EXIT_AFTER", "1"), ("CLAUDE_SESSIONS_TRANSCRIPTS", &path_str(&transcripts))]);
    let calls = manager.calls("tmux");
    assert!(!calls.iter().any(|call| call.contains("--resume")));
    let launch = calls.iter().find(|call| call.contains("claude $argv")).expect("no launch");
    assert!(launch.ends_with("--effort max"));
}

#[test]
fn doctor_flags_a_claude_link_behind_the_newest_download() {
    let manager = Manager::new();
    let versions = manager.sandbox.root.join("versions");
    fs::create_dir(&versions).unwrap();
    for name in ["2.1.300", "2.1.302"] {
        write_executable(&versions.join(name), "#!/bin/sh\n");
    }
    let link = manager.sandbox.root.join("claude-link");
    symlink(versions.join("2.1.300"), &link).unwrap();
    let (versions_text, link_text) = (path_str(&versions), path_str(&link));
    let env = [("CLAUDE_VERSIONS_DIR", versions_text.as_str()), ("CLAUDE_LINK", link_text.as_str())];
    assert!(manager.doctor_line("2.1.302", &env).trim().starts_with("FAIL"));
    fs::remove_file(&link).unwrap();
    symlink(versions.join("2.1.302"), &link).unwrap();
    assert!(manager.doctor_line("newest version", &env).trim().starts_with("ok"));
}

#[test]
fn compact_is_typed_only_at_the_prompt() {
    let manager = Manager::new();
    assert_eq!(manager.run(&["compact", SESSION_A], &[("SHIM_PANE_TEXT", IDLE)]).status.code(), Some(0));
    assert!(manager.calls("tmux").contains(&"tmux send-keys -t %4 -l -- /compact".to_string()));
    let busy = manager.run(&["compact", SESSION_A], &[("SHIM_PANE_TEXT", BUSY_WITHOUT_PROMPT)]);
    assert_eq!(busy.status.code(), Some(1));
    assert!(stderr(&busy).contains("busy"));
    assert_eq!(manager.calls("tmux").iter().filter(|call| call.contains("/compact")).count(), 1);
}

#[test]
fn approve_presses_enter_only_on_an_open_dialog_at_yes() {
    let manager = Manager::new();
    assert_eq!(manager.run(&["answer", SESSION_A, "yes"], &[("SHIM_PANE_TEXT", DIALOG)]).status.code(), Some(0));
    assert!(manager.calls("tmux").contains(&"tmux send-keys -t %4 Enter".to_string()));
}

#[test]
fn nothing_is_sent_when_the_dialog_closed() {
    let manager = Manager::new();
    let result = manager.run(&["answer", SESSION_A, "yes"], &[("SHIM_PANE_TEXT", IDLE)]);
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("no permission dialog"));
    assert!(manager.tmux_calls("send-keys").is_empty());
}

#[test]
fn decline_presses_escape() {
    let manager = Manager::new();
    manager.run(&["answer", SESSION_A, "no"], &[("SHIM_PANE_TEXT", DIALOG)]);
    assert!(manager.calls("tmux").contains(&"tmux send-keys -t %4 Escape".to_string()));
}

#[test]
fn say_sends_at_the_prompt_and_queues_while_working() {
    let manager = Manager::new();
    manager.run(&["say", SESSION_A, "also update the README"], &[("SHIM_PANE_TEXT", IDLE)]);
    let sent = manager.tmux_calls("send-keys");
    assert_eq!(sent[sent.len() - 2..], ["tmux send-keys -t %4 -l -- also update the README", "tmux send-keys -t %4 Enter"]);
    manager.run(&["say", SESSION_A, "and the changelog"], &[("SHIM_PANE_TEXT", BUSY_WITHOUT_PROMPT)]);
    assert_eq!(manager.tmux_calls("send-keys").last().map(String::as_str), Some("tmux send-keys -t %4 C-x Enter"));
}

#[test]
fn say_refuses_while_a_dialog_is_open_and_keeps_the_text_out_of_the_log() {
    let manager = Manager::new();
    let result = manager.run(&["say", SESSION_A, "the secret plan"], &[("SHIM_PANE_TEXT", DIALOG)]);
    assert_eq!(result.status.code(), Some(1));
    assert!(manager.tmux_calls("send-keys").is_empty());
    assert!(!manager.log_lines().iter().any(|line| line.contains("secret")));
}

#[test]
fn the_approval_pane_opens_under_the_session_without_the_cursor_and_closes() {
    let manager = Manager::new();
    manager.run(&["approval-pane", "0123456789ab"], &[]);
    let split = manager.tmux_calls("split-window").pop().expect("no split-window");
    assert!(split.contains("-v -d"));
    assert!(split.ends_with("attach-session -t =claude-run-0123456789ab"));
    // never left dead
    assert!(manager.tmux_calls("set-option").iter().any(|call| call.contains("remain-on-exit off")));
    assert_eq!(manager.run(&["approval-pane", "../x"], &[]).status.code(), Some(1));
}

#[test]
fn closing_an_approval_pane_that_already_closed_itself_succeeds() {
    let manager = Manager::new();
    fs::write(&manager.roles, "%10 sidebar\n%11 viewer\n%76 approval\n").unwrap();
    let output = manager.run(&["approval-pane", "close"], &[("SHIM_PANE_GONE", "1")]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn closing_the_approval_pane_kills_it() {
    let manager = Manager::new();
    fs::write(&manager.roles, "%10 sidebar\n%11 viewer\n%76 approval\n").unwrap();
    manager.run(&["approval-pane", "close"], &[]);
    assert_eq!(manager.tmux_calls("kill-pane"), ["tmux kill-pane -t %76"]);
}

#[test]
fn the_approval_takes_the_cursor_only_from_the_session_pane() {
    let manager = Manager::new();
    manager.run(&["approval-pane", "0123456789ab"], &[]);
    assert!(manager.tmux_calls("select-pane").is_empty());
    fs::write(&manager.roles, "%10 sidebar\n%11 viewer\n").unwrap();
    manager.run(&["approval-pane", "0123456789ab"], &[("SHIM_VIEWER_ACTIVE", "1")]);
    let roles = fs::read_to_string(&manager.roles).unwrap();
    let approval = roles.lines().filter(|line| line.ends_with(" approval")).last().expect("no approval pane").split_whitespace().next().unwrap().to_string();
    assert_eq!(manager.tmux_calls("select-pane").last().cloned(), Some(format!("tmux select-pane -t {approval}")));
}

#[test]
fn a_restart_covers_the_session_pane_with_a_loading_screen() {
    let manager = Manager::new();
    manager.run(&["restart", SESSION_A], &[]);
    let popup = manager.sandbox.wait_for_calls("tmux display-popup", 1, Duration::from_secs(3));
    assert!(!popup.is_empty(), "no popup over the viewer");
    assert!(popup[0].contains("-t %11 -x #{popup_pane_left} -y #{popup_pane_bottom}"));
    assert!(popup[0].ends_with("claude-sessions loading-screen"));
    let loading = fs::read_to_string(manager.runtime_file("claude-sessions-loading")).unwrap();
    assert_eq!(loading.lines().next(), Some("Resuming proj-a …")); // the restart's text, then the resume's
}

#[test]
fn the_loading_screen_closes_once_the_session_reports_in() {
    let manager = Manager::new();
    let since = now_seconds() - 1;
    let loading = manager.runtime_file("claude-sessions-loading");
    fs::write(&loading, format!("Resuming proj-a …\n{}\n{SESSION_A}\n{since}\nstart\n", manager.project.display())).unwrap();
    fs::write(&manager.snapshot, json!({"sessions": [{"session": SESSION_A, "cwd": path_str(&manager.project), "started": since + 1}]}).to_string()).unwrap();
    let started = Instant::now();
    assert_eq!(manager.run(&["loading-screen"], &[]).status.code(), Some(0));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!loading.exists());
}

#[test]
fn exiting_the_viewed_session_shows_a_closing_screen_until_it_is_gone() {
    let manager = Manager::new();
    fs::write(manager.runtime_file("claude-sessions.current"), format!("{SESSION_A}\n")).unwrap();
    manager.run(&["exit", SESSION_A], &[]);
    assert!(!manager.sandbox.wait_for_calls("tmux display-popup", 1, Duration::from_secs(3)).is_empty(), "no closing screen over the viewer");
    let loading = fs::read_to_string(manager.runtime_file("claude-sessions-loading")).unwrap();
    let every_fourth: Vec<&str> = loading.lines().step_by(4).collect();
    assert_eq!(every_fourth, ["Closing proj-a …", "gone"]);
}

#[test]
fn the_closing_screen_ends_once_the_session_left_the_snapshot() {
    let manager = Manager::new();
    let loading = manager.runtime_file("claude-sessions-loading");
    fs::write(&loading, format!("Closing proj-a …\n{}\n{SESSION_A}\n{}\ngone\n", manager.project.display(), now_seconds())).unwrap();
    fs::write(&manager.snapshot, json!({"sessions": []}).to_string()).unwrap();
    let started = Instant::now();
    manager.run(&["loading-screen"], &[]);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!loading.exists());
}

#[test]
fn the_loading_screen_gives_up_and_logs_it() {
    let manager = Manager::new();
    fs::write(manager.runtime_file("claude-sessions-loading"), format!("Starting Claude in proj-a …\n{}\n\n{}\nstart\n", manager.project.display(), now_seconds())).unwrap();
    manager.run(&["loading-screen"], &[("CLAUDE_SESSIONS_LOADING_TIMEOUT_S", "0")]);
    assert!(fs::read_to_string(manager.sandbox.state.join("claude-sessions/log")).unwrap().contains("loading screen timed out"));
}

#[test]
fn a_background_session_gets_its_own_tmux_session_and_leaves_the_viewer_alone() {
    let manager = Manager::new();
    assert_eq!(manager.run(&["background", &path_str(&manager.project), "fix this"], &[]).status.code(), Some(0));
    let started = manager.tmux_calls("new-session").pop().expect("no new-session");
    assert!(Regex::new(r"new-session -d -s claude-retry-bg-\d+ ").unwrap().is_match(&started), "{started}");
    assert!(started.ends_with("fish -ic claude $argv fix this"));
    assert!(manager.tmux_calls("respawn-pane").is_empty());
}

#[test]
fn zoom_toggles_the_viewer_pane() {
    let manager = Manager::new();
    manager.run(&["zoom"], &[]);
    assert!(manager.calls("tmux").contains(&"tmux resize-pane -Z -t %11".to_string()));
}

#[test]
fn restore_resumes_each_session_and_skips_what_cannot_be() {
    let manager = Manager::new();
    let result = manager.run(&["restore", &path_str(&manager.project), SESSION_A, "/no/such/dir", SESSION_B], &[("CLAUDE_SESSIONS_RESTORE_WAIT", "0")]);
    assert_eq!(result.status.code(), Some(0), "{}", stderr(&result));
    let resumes: Vec<String> = manager.tmux_calls("respawn-pane").into_iter().filter(|call| call.contains("claude --resume $argv")).collect();
    assert_eq!(resumes.len(), 1);
    assert!(resumes[0].ends_with(SESSION_A));
}

#[test]
fn an_exit_on_purpose_is_remembered() {
    let manager = Manager::new();
    manager.run(&["exit", SESSION_A], &[("SHIM_PANE_TEXT", IDLE), ("SHIM_EXIT_AFTER", "1")]);
    let ended = fs::read_to_string(manager.sandbox.state.join("claude-sessions/ended-sessions")).unwrap();
    assert_eq!(ended.split_whitespace().collect::<Vec<_>>(), [SESSION_A]);
}

#[test]
fn pull_requests_open_on_the_viewed_sessions_repository() {
    let manager = Manager::new();
    manager.git_repository();
    manager.prs_shims("");
    manager.run(&["prs"], &[]);
    assert!(manager.calls("hyprctl").contains(&r#"hyprctl eval OPEN_IN_SCRATCHPAD("prs", "https://github.com/o/proj-a/pulls")"#.to_string()));
}

#[test]
fn pull_requests_hide_when_showing_and_need_the_cockpit() {
    let manager = Manager::new();
    manager.prs_shims("special:prs");
    manager.run(&["prs"], &[]);
    assert!(manager.calls("hyprctl").contains(&r#"hyprctl dispatch hl.dsp.workspace.toggle_special("prs")"#.to_string()));
    manager.prs_shims("");
    let before = manager.calls("hyprctl").len();
    manager.run(&["prs"], &[("SHIM_WS", "2")]);
    assert!(!manager.calls("hyprctl")[before..].iter().any(|call| call.contains("SCRATCHPAD")));
}

#[test]
fn a_stale_hyprland_id_is_replaced_by_the_running_instance() {
    let manager = Manager::new();
    for (name, age) in [("old_instance", 100), ("running_instance", 0)] {
        let folder = manager.sandbox.runtime.join("hypr").join(name);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(".socket.sock"), "").unwrap();
        let when = SystemTime::now() - Duration::from_secs(age);
        fs::File::open(&folder).unwrap().set_times(fs::FileTimes::new().set_accessed(when).set_modified(when)).unwrap();
    }
    manager.sandbox.shim("hyprctl", r#"echo "id=$HYPRLAND_INSTANCE_SIGNATURE" >> "$SHIM_LOG"; echo '{"id": 2}'"#);
    manager.run(&["zoom"], &[("HYPRLAND_INSTANCE_SIGNATURE", "gone")]);
    manager.run(&["show"], &[("HYPRLAND_INSTANCE_SIGNATURE", "gone"), ("SHIM_WS", "2")]);
    let log = fs::read_to_string(&manager.sandbox.log).unwrap();
    assert!(log.contains("id=running_instance"));
    assert!(!log.contains("id=gone"));
}

#[test]
fn pull_requests_without_a_github_repository_show_the_window_as_it_was() {
    let manager = Manager::new();
    manager.prs_shims("");
    manager.run(&["prs"], &[]);
    assert!(manager.calls("hyprctl").contains(&r#"hyprctl eval TOGGLE_SCRATCHPAD("prs")"#.to_string()));
}

#[test]
fn git_opens_lazygit_on_the_repository_in_a_popup() {
    let manager = Manager::new();
    assert_eq!(manager.run(&["git", &path_str(&manager.project)], &[]).status.code(), Some(0));
    let popup = manager.tmux_calls("display-popup");
    assert_eq!(popup.len(), 1);
    assert!(popup[0].contains(&format!("-d {}", manager.project.display())));
    assert!(popup[0].ends_with("lazygit"));
}

#[test]
fn parallel_session_gets_its_own_clone_on_its_branch() {
    let manager = Manager::new();
    manager.git_repository();
    let clones = manager.sandbox.root.join("clones");
    let result = manager.run(&["parallel", &path_str(&manager.project), "feat/effort-menu"], &[("CLAUDE_SESSIONS_CLONES", &path_str(&clones))]);
    assert_eq!(result.status.code(), Some(0), "{}", stderr(&result));
    let clone = clones.join("proj-a/feat-effort-menu");
    let branch = git_output(&clone, &["branch", "--show-current"]);
    let origin = git_output(&clone, &["remote", "get-url", "origin"]);
    assert_eq!((branch.as_str(), origin.as_str()), ("feat/effort-menu", "git@github.com:o/proj-a.git"));
    let in_clone = format!("-c {} ", clone.display());
    assert!(manager.tmux_calls("respawn-pane").iter().any(|call| call.contains(&in_clone) && call.contains("claude $argv")));
}

#[test]
fn a_clone_of_a_clone_is_filed_under_the_first_repository() {
    let manager = Manager::new();
    manager.git_repository();
    let clones = manager.sandbox.root.join("clones");
    let clones_text = path_str(&clones);
    manager.run(&["parallel", &path_str(&manager.project), "first"], &[("CLAUDE_SESSIONS_CLONES", &clones_text)]);
    let result = manager.run(&["parallel", &path_str(&clones.join("proj-a/first")), "second"], &[("CLAUDE_SESSIONS_CLONES", &clones_text)]);
    assert_eq!(result.status.code(), Some(0), "{}", stderr(&result));
    assert!(clones.join("proj-a/second/.git").is_dir());
}

#[test]
fn parallel_refuses_a_bad_branch_name_in_the_viewer() {
    let manager = Manager::new();
    manager.git_repository();
    let clones = manager.sandbox.root.join("clones");
    manager.run(&["parallel", &path_str(&manager.project), "bad..name"], &[("CLAUDE_SESSIONS_CLONES", &path_str(&clones))]);
    assert!(!clones.exists());
    assert!(manager.tmux_calls("respawn-pane").iter().any(|call| call.contains("not a branch name")));
}

#[test]
fn review_refuses_what_is_not_a_pull_request_number() {
    let manager = Manager::new();
    manager.run(&["review", &path_str(&manager.project), "12; rm -rf /"], &[]);
    assert!(manager.tmux_calls("respawn-pane").iter().any(|call| call.contains("not a pull request number")));
}

#[test]
fn every_command_leaves_a_log_line_with_its_outcome() {
    let manager = Manager::new();
    manager.run(&["view", SESSION_A], &[]);
    manager.run(&["label-state", "purple"], &[]);
    let lines = manager.log_lines();
    let first = Regex::new(&format!(r"^\S+ INFO manager: view {SESSION_A} -> exit 0 \(\d+ ms\)$")).unwrap();
    assert!(first.is_match(&lines[0]), "{}", lines[0]);
    let last = lines.last().unwrap();
    assert!(Regex::new(r" ERROR manager: label-state purple -> exit 1 ").unwrap().is_match(last), "{last}");
}

#[test]
fn claude_arguments_to_new_are_counted_not_logged() {
    let manager = Manager::new();
    manager.run(&["new", &path_str(&manager.sandbox.root), "fix the secret thing"], &[]);
    let lines = manager.log_lines();
    let line = lines.iter().find(|line| line.contains(" manager: new ")).expect("no log line for new");
    assert!(!line.contains("secret"));
    assert!(line.contains("(+1 claude arguments)"));
}

#[test]
fn errors_of_a_background_run_reach_the_log() {
    let manager = Manager::new();
    manager.run(&["view", "no-such-session"], &[("CLAUDE_SESSIONS_LOG_STDERR", "1")]);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !manager.log_lines().iter().any(|line| line.contains("has no tmux pane")) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(manager.log_lines().iter().any(|line| line.contains(" ERROR manager: claude-sessions: session no-such-session has no tmux pane")));
}

#[test]
fn the_log_rotates_past_its_size() {
    let manager = Manager::new();
    let log = manager.sandbox.state.join("claude-sessions/log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "x".repeat(1_100_000)).unwrap();
    manager.run(&["log", "test", "INFO", "after rotation"], &[]);
    assert!(manager.sandbox.state.join("claude-sessions/log.1").exists());
    assert_eq!(manager.log_lines().len(), 1);
}

#[test]
fn a_failed_launch_explains_itself_in_the_pane() {
    let manager = Manager::new();
    manager.run(&["new", &path_str(&manager.sandbox.root), "--effort", "max"], &[]);
    let launch = manager.calls("tmux").into_iter().find(|call| call.contains("claude $argv")).expect("no launch");
    assert!(launch.contains("claude exited with status $code"));
    assert!(launch.contains("claude-sessions doctor"));
}

#[test]
fn restart_refuses_an_unknown_effort_before_touching_the_session() {
    let manager = Manager::new();
    let result = manager.run(&["restart", SESSION_A, "turbo"], &[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("unknown effort turbo"));
    assert!(manager.tmux_calls("send-keys").is_empty());
}

#[test]
fn show_from_the_claude_workspace_goes_back() {
    let manager = Manager::new();
    manager.run(&["show"], &[("SHIM_WS", "6")]);
    assert!(manager.calls("hyprctl").contains(&r#"hyprctl dispatch hl.dsp.focus({ workspace = "previous" })"#.to_string()));
    assert!(manager.calls("setsid").is_empty());
}

#[test]
fn show_elsewhere_hides_the_bar_and_opens_the_window() {
    let manager = Manager::new();
    manager.sandbox.shim("pkill", "");
    manager.run(&["show"], &[("SHIM_WS", "2")]);
    assert!(manager.calls("pkill").contains(&"pkill -x -USR1 waybar".to_string()));
    let home = manager.sandbox.home.display();
    assert_eq!(manager.calls("setsid"), [format!("setsid -f kitty --class claude-sessions --config {home}/.config/kitty/kitty.conf \
        --config {home}/.config/kitty/claude-sessions.conf -e claude-sessions")]);
}

#[test]
fn the_first_show_of_the_day_opens_the_digest_once() {
    let manager = Manager::new();
    manager.sandbox.shim("pkill", "");
    manager.run(&["show"], &[("SHIM_WS", "2")]);
    manager.run(&["show"], &[("SHIM_WS", "2")]);
    let digests = manager.tmux_calls("respawn-pane").into_iter().filter(|call| call.contains("claude-sessions-sidebar digest")).count();
    assert_eq!(digests, 1);
}

#[test]
fn reset_clears_the_daemon_and_the_caches() {
    let manager = Manager::new();
    let cache = manager.statusline_cache();
    fs::write(cache.join("statusline-x.json"), "{}").unwrap();
    manager.run(&["reset"], &[]);
    assert!(manager.calls("osd-show").contains(&"osd-show sessions-clear".to_string()));
    assert!(!cache.join("statusline-x.json").exists());
}

#[test]
fn unknown_subcommand_prints_usage() {
    let manager = Manager::new();
    let result = manager.run(&["bogus"], &[]);
    assert_eq!(result.status.code(), Some(2));
    assert!(stderr(&result).contains("usage:"));
}

// Remote Control hand-off: /rc typed at an idle prompt, queued while busy, only where needed.

fn mark_remote(manager: &Manager, session: &str, remote: bool) {
    fs::write(manager.statusline_cache().join(format!("statusline-{session}.json")), json!({"remote": remote}).to_string()).unwrap();
}

#[test]
fn handoff_types_rc_at_an_idle_prompt() {
    let manager = Manager::new();
    manager.run(&["handoff"], &[("SHIM_PANE_TEXT", IDLE)]);
    assert_eq!(manager.tmux_calls("send-keys"), ["tmux send-keys -t %4 /rc", "tmux send-keys -t %4 Enter"]);
}

#[test]
fn handoff_queues_rc_while_claude_works() {
    let manager = Manager::new();
    manager.run(&["handoff"], &[("SHIM_PANE_TEXT", BUSY)]);
    assert_eq!(manager.tmux_calls("send-keys"), ["tmux send-keys -t %4 /rc", "tmux send-keys -t %4 C-x Enter"]);
}

#[test]
fn unlock_takes_back_only_what_lock_handed_off() {
    let manager = Manager::new();
    // Session A starts on Remote Control by hand; B is local.
    let project = path_str(&manager.project);
    fs::write(&manager.snapshot, json!({"sessions": [
        {"session": SESSION_A, "state": "waiting", "project": "a", "cwd": project, "tmux_pane": "%4"},
        {"session": SESSION_B, "state": "waiting", "project": "b", "cwd": project, "tmux_pane": "%5"}]}).to_string()).unwrap();
    mark_remote(&manager, SESSION_A, true);
    manager.run(&["handoff", "lock"], &[("SHIM_PANE_TEXT", IDLE)]);
    let handed_off = manager.runtime_file("claude-sessions.handed-off");
    assert_eq!(fs::read_to_string(&handed_off).unwrap().split_whitespace().collect::<Vec<_>>(), [SESSION_B]);
    let locked = manager.tmux_calls("send-keys").len();
    manager.run(&["handoff", "unlock"], &[("SHIM_PANE_TEXT", IDLE)]);
    let unlocked = manager.tmux_calls("send-keys")[locked..].to_vec();
    assert!(!unlocked.is_empty() && unlocked.iter().all(|call| call.contains("-t %5")), "{unlocked:?}");
    assert!(!handed_off.exists());
}

#[test]
fn handoff_refuses_an_unknown_mode() {
    let manager = Manager::new();
    assert_eq!(manager.run(&["handoff", "sideways"], &[]).status.code(), Some(1));
}

#[test]
fn handoff_skips_sessions_already_remote_and_off_takes_them_back() {
    let manager = Manager::new();
    mark_remote(&manager, SESSION_A, true);
    let result = manager.run(&["handoff"], &[("SHIM_PANE_TEXT", IDLE)]);
    assert!(manager.tmux_calls("send-keys").is_empty());
    assert!(stdout(&result).contains("for 0 session"));
    manager.run(&["handoff", "off"], &[("SHIM_PANE_TEXT", IDLE)]);
    assert_eq!(manager.tmux_calls("send-keys").first().map(String::as_str), Some("tmux send-keys -t %4 /rc"));
}

#[test]
fn remote_toggles_one_session() {
    let manager = Manager::new();
    manager.run(&["remote", SESSION_A], &[("SHIM_PANE_TEXT", IDLE)]);
    assert_eq!(manager.tmux_calls("send-keys").first().map(String::as_str), Some("tmux send-keys -t %4 /rc"));
}

#[test]
fn manager_gets_a_tagged_viewer_pane() {
    let manager = Manager::new();
    fs::write(&manager.roles, "%10 sidebar\n").unwrap();
    manager.run(&["prepare"], &[]);
    let splits = manager.tmux_calls("split-window");
    assert_eq!(splits.len(), 1);
    assert!(splits[0].contains("claude-sessions idle"));
    let roles = fs::read_to_string(&manager.roles).unwrap();
    let added: Vec<&str> = roles.lines().skip(1).map(|line| line.split_whitespace().nth(1).unwrap()).collect();
    assert_eq!(added, ["viewer"]);
}

#[test]
fn a_manager_without_role_tags_is_refused_with_the_fix() {
    let manager = Manager::new();
    fs::write(&manager.roles, "").unwrap();
    let result = manager.run(&["prepare"], &[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("tmux kill-session -t claude-manager"));
}
