mod common;
use common::sandbox::*;
use std::fs;

fn fixture() -> Sandbox {
    let sandbox = Sandbox::new();
    let theme = sandbox.home.join(".config/fzf");
    fs::create_dir_all(&theme).unwrap();
    fs::write(theme.join("tokyonight_night.sh"), "").unwrap();
    sandbox.shim("tmux", r#"case $1 in
        display-message) echo '120 40' ;;
        new-session) printf '%s\n' "$@" > "$HOME/chief-launch"; [[ -z ${FAIL_TMUX:-} ]] || { echo 'tmux unavailable' >&2; exit 1; } ;;
        esac"#);
    sandbox
}

#[test]
fn chief_launcher_creates_its_folder_and_uses_the_reserved_native_identity() {
    let sandbox = fixture();
    let directory = sandbox.home.join("IdeaProjects/cockpit-chief");
    let output = sandbox.run(bin().join("cockpit"), &["--agent", "opencode", "chief-start", OPENCODE_SESSION, directory.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(directory.is_dir());
    let launch = fs::read_to_string(sandbox.home.join("chief-launch")).unwrap();
    assert!(launch.contains(&format!("--session\n{OPENCODE_SESSION}\n")), "{launch}");
    assert!(launch.contains(directory.to_str().unwrap()), "{launch}");
}

#[test]
fn chief_launcher_preserves_existing_folder_contents() {
    let sandbox = fixture();
    let directory = sandbox.home.join("IdeaProjects/cockpit-chief");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("AGENTS.md"), "Existing instructions").unwrap();
    let output = sandbox.run(bin().join("cockpit"), &["--agent", "opencode", "chief-start", OPENCODE_SESSION, directory.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fs::read_to_string(directory.join("AGENTS.md")).unwrap(), "Existing instructions");
}

#[test]
fn chief_launcher_rejects_a_symlink_before_attempting_to_launch() {
    let sandbox = fixture();
    let directory = sandbox.home.join("IdeaProjects/cockpit-chief");
    fs::create_dir_all(directory.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&sandbox.root, &directory).unwrap();
    let output = sandbox.run(bin().join("cockpit"), &["--agent", "opencode", "chief-start", OPENCODE_SESSION, directory.to_str().unwrap()], &[]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("symlink"));
    assert!(!sandbox.home.join("chief-launch").exists());
}

#[test]
fn a_tmux_error_is_uncertain_rather_than_a_safe_prelaunch_failure() {
    let sandbox = fixture();
    let directory = sandbox.home.join("IdeaProjects/cockpit-chief");
    let output = sandbox.run(bin().join("cockpit"), &["--agent", "opencode", "chief-start", OPENCODE_SESSION, directory.to_str().unwrap()], &[("FAIL_TMUX", "1")]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("tmux unavailable"));
}

#[test]
fn chief_launcher_keeps_the_configured_model_and_effort() {
    let sandbox = fixture();
    fs::create_dir_all(sandbox.state.join("cockpit")).unwrap();
    fs::write(sandbox.state.join("cockpit/settings.json"), r#"{"opencode-model":"test/model","opencode-effort":"high"}"#).unwrap();
    let directory = sandbox.home.join("IdeaProjects/cockpit-chief");
    let output = sandbox.run(bin().join("cockpit"), &["--agent", "opencode", "chief-start", OPENCODE_SESSION, directory.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(fs::read_to_string(sandbox.home.join("chief-launch")).unwrap().contains("COCKPIT_OPENCODE_MODEL=test/model#high"));
}

#[test]
fn invalid_launch_settings_are_a_safe_failure_with_the_actual_cause() {
    let sandbox = fixture();
    fs::create_dir_all(sandbox.state.join("cockpit")).unwrap();
    fs::write(sandbox.state.join("cockpit/settings.json"), "not json").unwrap();
    let directory = sandbox.home.join("IdeaProjects/cockpit-chief");
    let output = sandbox.run(bin().join("cockpit"), &["--agent", "opencode", "chief-start", OPENCODE_SESSION, directory.to_str().unwrap()], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("cannot read opencode-model"));
    assert!(!sandbox.home.join("chief-launch").exists());
}
