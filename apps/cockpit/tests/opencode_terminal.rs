//! Fish's terminal choice and the manager's attachment route, without a live tmux server.
mod common;
use common::sandbox::*;
use std::fs;
use std::process::Output;

struct Terminal {
    sandbox: Sandbox,
    directory: std::path::PathBuf,
}

impl Terminal {
    fn new() -> Self {
        let mut sandbox = Sandbox::new();
        for name in ["COCKPIT_INSIDE", "COCKPIT_TERMINAL", "TMUX"] {
            sandbox.env.insert(name.into(), String::new());
        }
        let directory = sandbox.root.join("project with spaces");
        fs::create_dir(&directory).unwrap();
        sandbox.env.insert("TEST_PANES".into(), sandbox.root.join("panes").display().to_string());
        sandbox.env.insert("COCKPIT_OPENCODE".into(), sandbox.shims.join("opencode").display().to_string());
        sandbox.shim("opencode", "printf '%s\\n' \"$CLAUDE_CODE_SESSION_ID|$COCKPIT_TERMINAL|$*\" >> \"$HOME/launches\"");
        sandbox.shim("claude-audit", "");
        sandbox.shim("tmux", r#"
case $1 in
  list-panes) if [[ $* == *session_name* ]]; then cat "$TEST_PANES"; fi ;;
  display-message) if [[ $* == *pane_width* ]]; then echo '120 40'; fi ;;
  new-session)
    [[ -z ${TEST_START_FAIL:-} ]] || exit 1
    shift
    while (($#)); do
      case $1 in
        -s) name=$2; shift 2 ;;
        -c) directory=$2; shift 2 ;;
        -e) export "$2"; shift 2 ;;
        -x|-y) shift 2 ;;
        -d) shift ;;
        env) break ;;
        *) exit 90 ;;
      esac
    done
    printf '%s\t%%42\t%s\t0\n' "$name" "$directory" >> "$TEST_PANES"
    "$@" ;;
esac
"#);
        fs::write(sandbox.root.join("panes"), "").unwrap();
        let fzf = sandbox.home.join(".config/fzf");
        fs::create_dir_all(&fzf).unwrap();
        fs::write(fzf.join("tokyonight_night.sh"), "").unwrap();
        Self { sandbox, directory }
    }

    fn panes(&self, names: &[&str]) {
        let rows: String = names.iter().enumerate().map(|(index, name)| {
            format!("{name}\t%{}\t{}\t0\n", index + 4, self.directory.display())
        }).collect();
        fs::write(self.sandbox.root.join("panes"), rows).unwrap();
    }

    fn manager(&self, input: &str, extra: &[(&str, &str)]) -> Output {
        self.sandbox.run_with_input(bin().join("cockpit"),
            &["--agent", "opencode", "terminal", self.directory.to_str().unwrap()], extra, input)
    }

    fn fish(&self, arguments: &str, input: &str, extra: &[(&str, &str)]) -> Output {
        // Only the tty detection is substituted: real fish parsing, read and dispatch remain.
        let script = format!("source '{}'; function isatty; return 0; end; opencode {arguments}", repo().join("config/fish/.config/fish/functions/opencode.fish").display());
        let mut command = self.sandbox.command("fish", extra);
        command.current_dir(&self.directory).env_remove("COCKPIT_INSIDE")
            .env_remove("COCKPIT_TERMINAL").env_remove("TMUX").envs(extra.iter().copied());
        use std::io::Write;
        use std::process::Stdio;
        let mut child = command.args(["--no-config", "-c", &script]).stdin(Stdio::piped())
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    fn tmux(&self, verb: &str) -> Vec<String> {
        self.sandbox.calls("tmux").into_iter().filter(|call| call.starts_with(&format!("tmux {verb} "))).collect()
    }
}

#[test]
fn typing_claude_explicitly_keeps_claude_when_the_global_default_changes() {
    let terminal = Terminal::new();
    terminal.sandbox.shim("cockpit", "");
    fs::remove_file(terminal.sandbox.home.join(".local/bin/cockpit")).unwrap();
    std::os::unix::fs::symlink(terminal.sandbox.shims.join("cockpit"), terminal.sandbox.home.join(".local/bin/cockpit")).unwrap();
    let script = format!("source '{}'; function isatty; return 0; end; claude", repo().join("config/fish/.config/fish/functions/claude.fish").display());
    let output = terminal.sandbox.run_with_input("fish", &["--no-config", "-c", &script], &[], "\n");
    assert!(output.status.success(), "{}", stderr(&output));
    let calls = terminal.sandbox.calls("cockpit");
    assert!(calls[0].starts_with("cockpit --agent claude new "), "{calls:?}");
}

#[test]
fn fish_terminal_choice_attaches_the_existing_process_without_changing_ownership() {
    let terminal = Terminal::new();
    terminal.panes(&["claude-retry-opencode-123"]);
    let output = terminal.fish("", "t\n", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.tmux("attach-session"), ["tmux attach-session -t =claude-retry-opencode-123"]);
    assert!(terminal.sandbox.calls("opencode").is_empty());
    assert!(terminal.tmux("set-environment").is_empty());
}

#[test]
fn bare_fish_on_a_real_tty_attaches_the_managed_process() {
    let terminal = Terminal::new();
    terminal.panes(&["claude-retry-opencode-123"]);
    let script = format!("cd '{}'; source '{}'; opencode", terminal.directory.display(),
        repo().join("config/fish/.config/fish/functions/opencode.fish").display());
    let environment: Vec<_> = terminal.sandbox.env.iter()
        .map(|(name, value)| (name.as_str(), std::path::Path::new(value))).collect();
    let mut tty = common::Terminal::start(std::path::Path::new("fish"), &["--no-config", "--features=no-query-term", "-c", &script], &environment);
    tty.pump(2.0, Some(b"this terminal"));
    assert!(common::contains(&tty.screen, b"this terminal"), "{}", String::from_utf8_lossy(&tty.screen));
    tty.type_keys(b"t");
    tty.pump(0.3, None);
    let output = finish(tty.child, std::time::Duration::from_secs(3));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&tty.screen));
    assert_eq!(terminal.tmux("attach-session"), ["tmux attach-session -t =claude-retry-opencode-123"]);
    assert!(terminal.sandbox.calls("opencode").is_empty());
}

#[test]
fn multiple_sessions_require_selection_and_cancellation_never_creates() {
    for input in ["\n", "0\n", "99\n", ""] {
        let terminal = Terminal::new();
        terminal.panes(&["claude-retry-opencode-123", "claude-retry-opencode-456"]);
        let output = terminal.manager(input, &[]);
        assert!(!output.status.success());
        assert!(terminal.tmux("attach-session").is_empty());
        assert!(terminal.tmux("new-session").is_empty());
    }
}

#[test]
fn multiple_sessions_attach_only_the_selected_process() {
    let terminal = Terminal::new();
    terminal.panes(&["claude-retry-opencode-123", "claude-retry-opencode-456"]);
    let output = terminal.manager("2\n", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.tmux("attach-session"), ["tmux attach-session -t =claude-retry-opencode-456"]);
    assert!(terminal.tmux("new-session").is_empty());
}

#[test]
fn fish_terminal_choice_passes_the_picker_selection_to_the_manager() {
    let terminal = Terminal::new();
    terminal.panes(&["claude-retry-opencode-123", "claude-retry-opencode-456"]);
    let output = terminal.fish("", "t2\n", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.tmux("attach-session"), ["tmux attach-session -t =claude-retry-opencode-456"]);
    assert!(terminal.tmux("new-session").is_empty());
}

#[test]
fn no_match_creates_once_with_identity_and_terminal_marker_then_reuses_it() {
    let terminal = Terminal::new();
    for _ in 0..2 {
        let output = terminal.fish("", "t\n", &[]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
    assert_eq!(terminal.tmux("new-session").len(), 1);
    let launches = fs::read_to_string(terminal.sandbox.home.join("launches")).unwrap();
    let launch = launches.trim();
    let (identity, arguments) = launch.split_once("|1|").expect("terminal marker");
    assert!(identity.starts_with("ses_"));
    assert_eq!(arguments, format!("--standalone --session {identity}"));
    assert_eq!(terminal.tmux("attach-session").len(), 2);
}

#[test]
fn nested_tmux_switches_clients_instead_of_nesting_and_managed_launches_are_refused() {
    let terminal = Terminal::new();
    terminal.panes(&["claude-retry-opencode-123"]);
    let output = terminal.manager("", &[("TMUX", "/fake/socket,1,0")]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.tmux("switch-client"), ["tmux switch-client -t =claude-retry-opencode-123"]);
    assert!(terminal.tmux("attach-session").is_empty());
    terminal.sandbox.reset_log();
    let output = terminal.manager("", &[("COCKPIT_INSIDE", "1")]);
    assert!(!output.status.success());
    assert!(terminal.sandbox.calls("tmux").is_empty());
}

#[test]
fn unrelated_dead_and_format_injected_sessions_are_never_attached() {
    let terminal = Terminal::new();
    fs::write(terminal.sandbox.root.join("panes"), format!(
        "unrelated\t%4\t{0}\t0\nclaude-retry-opencode-123\t%5\t/elsewhere\t0\nclaude-retry-opencode-456\t%6\t{0}\t1\nclaude-retry-opencode-#S\t%7\t{0}\t0\n", terminal.directory.display())).unwrap();
    let output = terminal.manager("", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.tmux("new-session").len(), 1);
}

#[test]
fn hash_directories_are_refused_before_tmux() {
    let mut terminal = Terminal::new();
    terminal.directory = terminal.sandbox.root.join("project#S");
    fs::create_dir(&terminal.directory).unwrap();
    let output = terminal.manager("", &[]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("refusing directory"));
    assert!(terminal.sandbox.calls("tmux").is_empty());
}

#[test]
fn fish_arguments_and_managed_launches_still_call_the_launcher_directly() {
    for (arguments, extra) in [("--version", vec![]), ("", vec![("COCKPIT_INSIDE", "1")])] {
        let terminal = Terminal::new();
        let output = terminal.fish(arguments, "", &extra);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(terminal.sandbox.calls("opencode"), [format!("opencode {arguments}")]);
        assert!(terminal.sandbox.calls("tmux").is_empty());
    }
}

#[test]
fn fish_default_choice_keeps_the_manager_new_and_show_route() {
    let terminal = Terminal::new();
    terminal.sandbox.shim("cockpit", "");
    fs::remove_file(terminal.sandbox.home.join(".local/bin/cockpit")).unwrap();
    std::os::unix::fs::symlink(terminal.sandbox.shims.join("cockpit"), terminal.sandbox.home.join(".local/bin/cockpit")).unwrap();
    let output = terminal.fish("", "\n", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.sandbox.calls("cockpit"), [
        format!("cockpit --agent opencode new {}", terminal.directory.display()),
        "cockpit show".to_string(),
    ]);
}

#[test]
fn a_failed_tmux_start_never_attaches_a_phantom_session() {
    let terminal = Terminal::new();
    let output = terminal.manager("", &[("TEST_START_FAIL", "1")]);
    assert!(!output.status.success(), "a failed start must fail the terminal route");
    assert!(terminal.tmux("attach-session").is_empty());
}

#[test]
fn simultaneous_terminals_create_only_one_process() {
    let terminal = Terminal::new();
    let mut children: Vec<_> = (0..2).map(|_| terminal.sandbox.command(bin().join("cockpit"), &[])
        .args(["--agent", "opencode", "terminal", terminal.directory.to_str().unwrap()])
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap()).collect();
    for child in children.drain(..) {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
    }
    assert_eq!(terminal.tmux("new-session").len(), 1);
    assert_eq!(terminal.tmux("attach-session").len(), 2);
}

#[test]
fn multiple_panes_of_one_session_do_not_require_a_picker() {
    let terminal = Terminal::new();
    terminal.panes(&["claude-retry-opencode-123", "claude-retry-opencode-123"]);
    let output = terminal.manager("", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.tmux("attach-session").len(), 1);
}

#[test]
fn a_symlinked_project_attaches_the_existing_physical_directory() {
    let terminal = Terminal::new();
    terminal.panes(&["claude-retry-opencode-123"]);
    let alias = terminal.sandbox.root.join("project-alias");
    std::os::unix::fs::symlink(&terminal.directory, &alias).unwrap();
    let output = terminal.sandbox.run(bin().join("cockpit"),
        &["--agent", "opencode", "terminal", alias.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(terminal.tmux("attach-session"), ["tmux attach-session -t =claude-retry-opencode-123"]);
    assert!(terminal.tmux("new-session").is_empty());
}

#[test]
fn simultaneous_physical_and_symlinked_paths_share_the_creation_lock() {
    let terminal = Terminal::new();
    let alias = terminal.sandbox.root.join("project-alias");
    std::os::unix::fs::symlink(&terminal.directory, &alias).unwrap();
    let children: Vec<_> = [&terminal.directory, &alias].into_iter().map(|directory| {
        terminal.sandbox.command(bin().join("cockpit"), &[])
            .args(["--agent", "opencode", "terminal", directory.to_str().unwrap()])
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap()
    }).collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
    }
    assert_eq!(terminal.tmux("new-session").len(), 1);
    assert_eq!(terminal.tmux("attach-session").len(), 2);
}
