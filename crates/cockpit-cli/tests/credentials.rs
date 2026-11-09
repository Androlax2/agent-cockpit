//! Issue #1251: `torlek provider credential` and `torlek project token` save, show and clear
//! the owner's secret files with the user guide's modes, through the real daemon in a fixture
//! home, and no output ever holds a value.
mod common;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use common::TestHome;

const CREDENTIAL: &str = "sk-ant-oat01-fixture_Credential.value";
const OTHER_CREDENTIAL: &str = "sk-ant-api03-another_fixture";
const GITHUB_TOKEN: &str = "github_pat_fixture_Token_0123456789";

/// What a launch puts in its agent's environment.
const SESSION_ENVIRONMENT: [(&str, &str); 2] = [
    ("TORLEK_SESSION", "s0123456789ab"),
    ("TORLEK_LAUNCH_TOKEN", "the-token"),
];

fn torlek(
    home: &TestHome,
    arguments: &[&str],
    stdin: &str,
    environment: &[(&str, &str)],
) -> Output {
    let mut child = home
        .base_command(Path::new(env!("CARGO_BIN_EXE_torlek")))
        .args(arguments)
        .envs(environment.iter().copied())
        .stdin(Stdio::piped())
        .spawn()
        .expect("the torlek binary runs");
    // A verb that reads nothing closes its end first; the write may then fail.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().expect("torlek finishes")
}

/// A fixture home whose daemon wires Claude Code: its fake `claude` answers its version and
/// nothing else, so the daemon knows the variables its credential may be saved under.
fn home_with_claude() -> TestHome {
    let home = TestHome::new();
    let program = home.home().join("bin/claude");
    std::fs::write(
        &program,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '2.1.316 (Claude Code)'; exit 0; fi\nexit 127\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    home
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Neither stream holds any fixture secret.
fn assert_no_value(output: &Output) {
    let streams = format!("{}{}", text(&output.stdout), text(&output.stderr));
    for secret in [CREDENTIAL, OTHER_CREDENTIAL, GITHUB_TOKEN] {
        assert!(!streams.contains(secret), "a value was printed: {streams}");
    }
}

fn config_dir(home: &TestHome) -> PathBuf {
    home.home().join("config/torlek-dev")
}

fn claude_dir(home: &TestHome) -> PathBuf {
    config_dir(home).join("provider-credentials/claude")
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

fn set_claude(home: &TestHome, variable: &str, value: &str) -> Output {
    torlek(
        home,
        &[
            "provider",
            "credential",
            "set",
            "claude",
            "--variable",
            variable,
        ],
        &format!("{value}\n"),
        &[],
    )
}

#[test]
fn set_saves_the_credential_alone_with_mode_0600_in_a_0700_folder() {
    let home = home_with_claude();
    let _daemon = home.start();

    let output = set_claude(&home, "CLAUDE_CODE_OAUTH_TOKEN", CREDENTIAL);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let file = claude_dir(&home).join("CLAUDE_CODE_OAUTH_TOKEN");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{CREDENTIAL}\n")
    );
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&claude_dir(&home)), 0o700);
    assert_eq!(mode(&config_dir(&home).join("provider-credentials")), 0o700);
    assert!(
        text(&output.stdout).contains(&format!("usable: launches read {}", file.display())),
        "{}",
        text(&output.stdout)
    );
    assert_no_value(&output);
}

#[test]
fn set_replaces_a_file_whose_mode_stopped_launches() {
    let home = home_with_claude();
    let _daemon = home.start();
    let file = claude_dir(&home).join("CLAUDE_CODE_OAUTH_TOKEN");
    std::fs::create_dir_all(claude_dir(&home)).unwrap();
    std::fs::write(&file, format!("{OTHER_CREDENTIAL}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

    let output = torlek(
        &home,
        &["provider", "credential", "set", "claude"],
        &format!("{CREDENTIAL}\n"),
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{CREDENTIAL}\n")
    );
    assert_eq!(mode(&file), 0o600);
    assert_no_value(&output);
}

#[test]
fn show_prints_each_path_and_mode_never_the_value() {
    let home = home_with_claude();
    let _daemon = home.start();
    assert!(
        set_claude(&home, "ANTHROPIC_API_KEY", CREDENTIAL)
            .status
            .success()
    );

    let output = torlek(
        &home,
        &["provider", "credential", "show", "claude"],
        "",
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let dir = claude_dir(&home);
    assert_eq!(
        text(&output.stdout),
        format!(
            "{api}\tmode 0600\n{oauth}\tabsent\nusable: launches read {api}\n",
            api = dir.join("ANTHROPIC_API_KEY").display(),
            oauth = dir.join("CLAUDE_CODE_OAUTH_TOKEN").display(),
        )
    );
    assert_no_value(&output);
}

#[test]
fn show_names_a_file_with_a_mode_that_stops_launches_and_exits_1() {
    let home = home_with_claude();
    let _daemon = home.start();
    assert!(
        set_claude(&home, "ANTHROPIC_API_KEY", CREDENTIAL)
            .status
            .success()
    );
    let file = claude_dir(&home).join("ANTHROPIC_API_KEY");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

    let output = torlek(
        &home,
        &["provider", "credential", "show", "claude"],
        "",
        &[],
    );

    assert_eq!(output.status.code(), Some(1));
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains(&format!("{}\tmode 0644", file.display())),
        "{stdout}"
    );
    assert!(stdout.contains("not 0600 or 0400"), "{stdout}");
    assert_no_value(&output);
}

#[test]
fn clear_removes_the_credential_and_show_then_says_it_is_missing() {
    let home = home_with_claude();
    let _daemon = home.start();
    assert!(
        set_claude(&home, "CLAUDE_CODE_OAUTH_TOKEN", CREDENTIAL)
            .status
            .success()
    );
    let file = claude_dir(&home).join("CLAUDE_CODE_OAUTH_TOKEN");

    let cleared = torlek(
        &home,
        &["provider", "credential", "clear", "claude"],
        "",
        &[],
    );
    let shown = torlek(
        &home,
        &["provider", "credential", "show", "claude"],
        "",
        &[],
    );

    assert_eq!(cleared.status.code(), Some(0), "{}", text(&cleared.stderr));
    assert_eq!(
        text(&cleared.stdout),
        format!("removed {}\n", file.display())
    );
    assert!(!file.exists());
    assert_eq!(shown.status.code(), Some(1));
    assert!(
        text(&shown.stdout).contains(
            "missing: none of these files exists; `torlek provider credential set claude` saves one"
        ),
        "{}",
        text(&shown.stdout)
    );
}

#[test]
fn clear_with_nothing_saved_says_so_and_succeeds() {
    let home = home_with_claude();
    let _daemon = home.start();

    let output = torlek(
        &home,
        &["provider", "credential", "clear", "claude"],
        "",
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "no file held claude's credential: nothing to clear\n"
    );
}

#[test]
fn set_refuses_a_second_file_while_another_holds_the_providers_account() {
    let home = home_with_claude();
    let _daemon = home.start();
    assert!(
        set_claude(&home, "ANTHROPIC_API_KEY", CREDENTIAL)
            .status
            .success()
    );

    let output = set_claude(&home, "CLAUDE_CODE_OAUTH_TOKEN", OTHER_CREDENTIAL);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("a provider binds one account"),
        "{}",
        text(&output.stderr)
    );
    assert!(!claude_dir(&home).join("CLAUDE_CODE_OAUTH_TOKEN").exists());
    assert_no_value(&output);
}

#[test]
fn set_without_a_variable_asks_for_one_when_the_provider_takes_several() {
    let home = home_with_claude();
    let _daemon = home.start();

    let output = torlek(
        &home,
        &["provider", "credential", "set", "claude"],
        &format!("{CREDENTIAL}\n"),
        &[],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains(
            "claude takes its credential from ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN: \
             name one with --variable"
        ),
        "{}",
        text(&output.stderr)
    );
    assert!(!claude_dir(&home).exists());
    assert_no_value(&output);
}

#[test]
fn set_refuses_a_value_a_launch_would_refuse_and_writes_nothing() {
    let home = home_with_claude();
    let _daemon = home.start();

    for value in ["two words", "one\nline too many", "quote\"d"] {
        let output = set_claude(&home, "ANTHROPIC_API_KEY", value);

        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{value:?}: {stderr}");
        assert!(
            stderr.contains("the value is not one credential"),
            "{stderr}"
        );
        assert!(!stderr.contains(value), "the value was printed: {stderr}");
        assert!(!claude_dir(&home).join("ANTHROPIC_API_KEY").exists());
    }
}

#[test]
fn set_with_nothing_on_standard_input_writes_nothing() {
    let home = home_with_claude();
    let _daemon = home.start();

    let output = set_claude(&home, "ANTHROPIC_API_KEY", "");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("claude's credential is empty"),
        "{}",
        text(&output.stderr)
    );
    assert!(!claude_dir(&home).exists());
}

#[test]
fn an_unknown_variable_or_provider_is_refused() {
    let home = home_with_claude();
    let _daemon = home.start();

    let variable = set_claude(&home, "OPENAI_API_KEY", CREDENTIAL);
    let provider = torlek(
        &home,
        &["provider", "credential", "set", "nobody", "--variable", "X"],
        &format!("{CREDENTIAL}\n"),
        &[],
    );

    assert_eq!(variable.status.code(), Some(1));
    assert!(
        text(&variable.stderr).contains("claude reads no credential from OPENAI_API_KEY"),
        "{}",
        text(&variable.stderr)
    );
    assert_eq!(provider.status.code(), Some(1));
    assert!(!config_dir(&home).join("provider-credentials").exists());
    assert_no_value(&variable);
    assert_no_value(&provider);
}

#[test]
fn project_token_set_saves_each_github_token_in_its_own_folder() {
    let home = home_with_claude();
    let _daemon = home.start();
    let project = home.project("app");
    home.cockpit_ok(&["project", "add", project.to_str().unwrap()]);

    for (flags, folder) in [
        (&[][..], "github-tokens"),
        (&["--api"][..], "github-api-tokens"),
    ] {
        let mut arguments = vec!["project", "token", "set", "app"];
        arguments.extend(flags);
        let output = torlek(&home, &arguments, &format!("{GITHUB_TOKEN}\n"), &[]);

        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        let file = config_dir(&home).join(folder).join("app");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!("{GITHUB_TOKEN}\n")
        );
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&config_dir(&home).join(folder)), 0o700);
        assert_no_value(&output);
    }
    assert!(
        home.cockpit_ok(&["project", "list"]).contains("app"),
        "the project is still registered"
    );
}

#[test]
fn project_token_show_and_clear_never_print_the_token() {
    let home = home_with_claude();
    let _daemon = home.start();
    let project = home.project("app");
    home.cockpit_ok(&["project", "add", project.to_str().unwrap()]);
    let set = torlek(
        &home,
        &["project", "token", "set", "app"],
        &format!("{GITHUB_TOKEN}\n"),
        &[],
    );
    assert!(set.status.success(), "{}", text(&set.stderr));
    let file = config_dir(&home).join("github-tokens/app");

    let shown = torlek(&home, &["project", "token", "show", "app"], "", &[]);
    let cleared = torlek(&home, &["project", "token", "clear", "app"], "", &[]);

    assert_eq!(
        text(&shown.stdout),
        format!(
            "{file}\tmode 0600\nusable: launches read {file}\n",
            file = file.display()
        )
    );
    assert_eq!(
        text(&cleared.stdout),
        format!("removed {}\n", file.display())
    );
    assert!(!file.exists());
    assert_no_value(&shown);
    assert_no_value(&cleared);
}

#[test]
fn project_token_for_an_unregistered_project_is_refused() {
    let home = home_with_claude();
    let _daemon = home.start();

    let output = torlek(
        &home,
        &["project", "token", "set", "app"],
        &format!("{GITHUB_TOKEN}\n"),
        &[],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("no project is named \"app\""),
        "{}",
        text(&output.stderr)
    );
    assert!(!config_dir(&home).join("github-tokens").exists());
}

#[test]
fn a_value_given_as_an_argument_is_a_usage_error() {
    let home = TestHome::new();

    let output = torlek(
        &home,
        &["provider", "credential", "set", "claude", CREDENTIAL],
        "",
        &[],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(!claude_dir(&home).exists());
}

#[test]
fn inside_a_session_every_credential_verb_is_refused_and_writes_nothing() {
    let home = home_with_claude();
    let _daemon = home.start();
    let project = home.project("app");
    home.cockpit_ok(&["project", "add", project.to_str().unwrap()]);

    for verb in [
        &[
            "provider",
            "credential",
            "set",
            "claude",
            "--variable",
            "ANTHROPIC_API_KEY",
        ][..],
        &["provider", "credential", "show", "claude"],
        &["provider", "credential", "clear", "claude"],
        &["project", "token", "set", "app"],
        &["project", "token", "set", "app", "--api"],
        &["project", "token", "show", "app"],
        &["project", "token", "clear", "app"],
    ] {
        let output = torlek(
            &home,
            verb,
            &format!("{CREDENTIAL}\n"),
            &SESSION_ENVIRONMENT,
        );

        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{verb:?}: {stderr}");
        assert!(
            stderr.contains("this command is the owner's alone"),
            "{verb:?}: {stderr}"
        );
        assert_no_value(&output);
    }
    for folder in ["provider-credentials", "github-tokens", "github-api-tokens"] {
        assert!(
            !config_dir(&home).join(folder).exists(),
            "{folder} was written"
        );
    }
}

/// On a terminal the prompt turns echo off before it shows, so what the owner types never
/// reaches the screen, and turns it back on after. util-linux `script` stands in for the owner's terminal.
#[cfg(target_os = "linux")]
#[test]
fn on_a_terminal_set_prompts_without_echoing_the_value() {
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::Duration;

    let home = home_with_claude();
    let _daemon = home.start();
    let command_line = format!(
        "{} provider credential set claude --variable ANTHROPIC_API_KEY && stty -a",
        env!("CARGO_BIN_EXE_torlek")
    );
    let mut child = home
        .base_command(Path::new("script"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args([
            "--quiet",
            "--return",
            "--command",
            &command_line,
            "/dev/null",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .expect("util-linux `script` runs");
    let mut stdout = child.stdout.take().unwrap();
    let (chunks, screen) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(read) = stdout.read(&mut buffer) {
            if read == 0 || chunks.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut shown = String::new();
    while !shown.contains("(not shown): ") {
        let chunk = screen
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no prompt on the terminal: {shown:?}"));
        shown.push_str(&String::from_utf8_lossy(&chunk));
    }

    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(format!("{CREDENTIAL}\r").as_bytes())
        .unwrap();
    let status = child.wait().unwrap();
    drop(stdin);
    while let Ok(chunk) = screen.recv_timeout(Duration::from_secs(5)) {
        shown.push_str(&String::from_utf8_lossy(&chunk));
    }

    assert!(status.success(), "{shown}");
    assert!(shown.contains("saved claude's credential"), "{shown}");
    assert!(!shown.contains(CREDENTIAL), "the value was echoed: {shown}");
    let settings_after = shown.split("saved claude's credential").nth(1).unwrap();
    assert!(
        settings_after
            .split_whitespace()
            .any(|setting| setting.trim_end_matches(';') == "echo"),
        "echo is not back on after the prompt: {settings_after}"
    );
    assert_eq!(
        std::fs::read_to_string(claude_dir(&home).join("ANTHROPIC_API_KEY")).unwrap(),
        format!("{CREDENTIAL}\n")
    );
}

#[test]
fn a_removed_projects_token_can_still_be_cleared() {
    let home = home_with_claude();
    let _daemon = home.start();
    let project = home.project("app");
    home.cockpit_ok(&["project", "add", project.to_str().unwrap()]);
    let set = torlek(
        &home,
        &["project", "token", "set", "app"],
        &format!("{GITHUB_TOKEN}\n"),
        &[],
    );
    assert!(set.status.success(), "{}", text(&set.stderr));
    home.cockpit_ok(&["project", "remove", "app"]);
    let file = config_dir(&home).join("github-tokens/app");

    let cleared = torlek(&home, &["project", "token", "clear", "app"], "", &[]);

    assert_eq!(cleared.status.code(), Some(0), "{}", text(&cleared.stderr));
    assert_eq!(
        text(&cleared.stdout),
        format!("removed {}\n", file.display())
    );
    assert!(!file.exists());
}
