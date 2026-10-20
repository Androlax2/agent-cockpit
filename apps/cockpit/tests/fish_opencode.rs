mod common;
use common::sandbox::*;
use std::fs;

fn invoke(choice: &str, arguments: &[&str], inside: &str, terminal: bool, attach_status: &str) -> (std::process::Output, Vec<String>) {
    let sandbox = Sandbox::new();
    let fzf = sandbox.home.join(".config/fzf");
    fs::create_dir_all(&fzf).unwrap();
    fs::write(fzf.join("tokyonight_night.sh"), "").unwrap();
    sandbox.shim("claude-sessions", "if [[ $1 == attach ]]; then exit \"$ATTACH_STATUS\"; fi");
    sandbox.shim("opencode", "echo \"terminal=$CLAUDE_SESSIONS_TERMINAL\" >> \"$SHIM_LOG\"");
    let script = r#"
        source "$FISH_CONFIG"
        set -gx PATH "$TEST_SHIMS" $PATH
        set -e CLAUDE_SESSIONS_TERMINAL
        function isatty
            test "$TEST_TERMINAL" = yes
        end
        opencode $argv
    "#;
    let config = repo().join("config/fish/.config/fish/config.fish");
    let mut command_arguments = vec!["--no-config", "-c", script, "--"];
    command_arguments.extend_from_slice(arguments);
    let output = sandbox.run_with_input("fish", &command_arguments, &[
        ("FISH_CONFIG", config.to_str().unwrap()),
        ("TEST_SHIMS", sandbox.shims.to_str().unwrap()),
        ("CLAUDE_SESSIONS_INSIDE", inside),
        ("TEST_TERMINAL", if terminal { "yes" } else { "no" }),
        ("ATTACH_STATUS", attach_status),
    ], choice);
    let calls = fs::read_to_string(&sandbox.log).unwrap().lines().map(str::to_string).collect();
    (output, calls)
}

#[test]
fn resume_attaches_the_existing_cockpit_without_starting_an_agent() {
    for choice in ["r", "R"] {
        let (output, calls) = invoke(choice, &[], "", true, "0");
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(calls, ["claude-sessions attach"]);
    }
}

#[test]
fn resume_preserves_an_attach_failure() {
    let (output, calls) = invoke("r", &[], "", true, "42");
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(calls, ["claude-sessions attach"]);
}

#[test]
fn enter_and_manager_keys_keep_creating_then_showing_a_session() {
    for choice in ["\n", "m", "M"] {
        let (output, calls) = invoke(choice, &[], "", true, "0");
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(calls.len(), 2);
        assert!(calls[0].starts_with("claude-sessions --agent opencode new "));
        assert_eq!(calls[1], "claude-sessions show");
    }
}

#[test]
fn terminal_keys_keep_using_the_managed_terminal_launch() {
    for choice in ["t", "T"] {
        let (output, calls) = invoke(choice, &[], "", true, "0");
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("claude-sessions --agent opencode terminal "));
    }
}

#[test]
fn arguments_and_managed_or_nonterminal_launches_bypass_the_menu() {
    for (arguments, inside, terminal, expected) in [
        (vec!["--session", "ses_existing"], "", true, "opencode --session ses_existing"),
        (vec![], "1", true, "opencode "),
        (vec![], "", false, "opencode "),
    ] {
        let (output, calls) = invoke("r", &arguments, inside, terminal, "0");
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(calls, [expected, "terminal="]);
    }
}

#[test]
fn unknown_keys_cancel_without_starting_anything() {
    let (output, calls) = invoke("x", &[], "", true, "0");
    assert_eq!(output.status.code(), Some(1));
    assert!(calls.is_empty());
}
