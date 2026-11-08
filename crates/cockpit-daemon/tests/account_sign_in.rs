//! #59, #858: the owner signs a login account in once with `torlek account sign-in`, which
//! runs the provider's own sign-in in the owner's terminal with a fresh Torlek-owned folder as
//! its home, then moves only the login file into the account's login folder, unread.
//! Through the real daemon and CLI binaries and a stand-in Codex whose `login` writes a fake
//! login and a file beside it.

mod candidate;
mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use real_daemon::Home;

const WORK: &str = "[accounts]\ncodex = [\"work\"]\n";
const FAKE_LOGIN: &str = "fixture-login-not-a-credential";
/// A variable of the owner's that a provider would prefer to its sign-in.
const OWNER_KEY: (&str, &str) = ("OPENAI_API_KEY", "sk-fixture-owner-key");

/// Replaces the stand-in Codex: it still answers `--version` for the daemon's probe, and its
/// `login` logs its arguments and environment, then writes `writes` under its home.
fn codex_signing_in(home: &Home, writes: &str) {
    let program = home.path().join("bin/codex");
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in\n\
         --version) echo 'codex-cli 0.166.0' ;;\n\
         login)\n\
           printf '%s\\n' \"$@\" > '{arguments}'\n\
           env > '{environment}'\n\
           mkdir -p \"$HOME/.codex\"\n\
           printf 'model = \"fixture\"\\n' > \"$HOME/.codex/config.toml\"\n\
           {writes}\n\
           ;;\n\
         esac\n",
        arguments = sign_in_arguments(home).display(),
        environment = sign_in_environment(home).display(),
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn writes_login() -> String {
    format!("printf '{FAKE_LOGIN}' > \"$HOME/.codex/auth.json\"")
}

fn sign_in_arguments(home: &Home) -> PathBuf {
    home.path().join("sign-in.arguments")
}

fn sign_in_environment(home: &Home) -> PathBuf {
    home.path().join("sign-in.environment")
}

fn logins_of(home: &Home, provider: &str, name: &str) -> PathBuf {
    home.state_dir()
        .join("account-logins")
        .join(provider)
        .join(name)
}

fn entries(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_moves_only_the_login_file_into_its_folder() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, &writes_login());
    let _daemon = home.start();

    let output = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let folder = logins_of(&home, "codex", "work");
    assert_eq!(entries(&folder), ["login"]);
    assert_eq!(entries(&folder.join("login")), ["auth.json"]);
    let login = folder.join("login/auth.json");
    assert_eq!(std::fs::read_to_string(&login).unwrap(), FAKE_LOGIN);
    let mode = std::fs::metadata(&login).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert!(text(&output.stdout).contains(&folder.join("login").display().to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sign_in_runs_in_a_fresh_home_without_the_owners_variables() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, &writes_login());
    let _daemon = home.start();

    let output = home
        .cockpit(
            &["account", "sign-in", "codex", "work"],
            &[OWNER_KEY, ("CODEX_HOME", "/owner/codex")],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let environment = std::fs::read_to_string(sign_in_environment(&home)).unwrap();
    let sign_in_home = logins_of(&home, "codex", "work").join("sign-in");
    assert!(
        environment
            .lines()
            .any(|line| line == format!("HOME={}", sign_in_home.display())),
        "{environment}"
    );
    assert!(!environment.contains(OWNER_KEY.0), "{environment}");
    assert!(!environment.contains("CODEX_HOME"), "{environment}");
    let arguments = std::fs::read_to_string(sign_in_arguments(&home)).unwrap();
    assert_eq!(
        arguments.lines().collect::<Vec<_>>(),
        ["login", "-c", "cli_auth_credentials_store=\"file\""]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sign_in_leaves_the_owners_own_codex_folder_alone() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, &writes_login());
    let _daemon = home.start();

    let output = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(!home.path().join(".codex").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_the_owner_does_not_list_is_refused_naming_the_setting() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, &writes_login());
    let _daemon = home.start();

    let output = home
        .cockpit(&["account", "sign-in", "codex", "spare"], &[])
        .await;

    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("accounts.codex"), "{stderr}");
    assert!(!sign_in_arguments(&home).exists(), "the sign-in ran");
    assert!(!logins_of(&home, "codex", "spare").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_that_leaves_no_login_moves_nothing_and_names_the_file() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, "true");
    let _daemon = home.start();

    let output = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;

    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("~/.codex/auth.json"), "{stderr}");
    assert!(!logins_of(&home, "codex", "work").join("login").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_that_fails_moves_nothing() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, &format!("{}\n exit 3", writes_login()));
    let _daemon = home.start();

    let output = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;

    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("status 3"));
    assert!(!logins_of(&home, "codex", "work").join("login").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_that_fails_leaves_no_login_in_its_home() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, &format!("{}\n exit 3", writes_login()));
    let _daemon = home.start();

    let output = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;

    assert!(!output.status.success());
    assert!(!logins_of(&home, "codex", "work").join("sign-in").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_swapped_for_a_link_is_refused_and_nothing_is_kept() {
    let home = Home::new();
    home.write_owner_config(WORK);
    let outside = home.path().join("outside-secret");
    std::fs::write(&outside, "not a login").unwrap();
    codex_signing_in(
        &home,
        &format!("ln -s '{}' \"$HOME/.codex/auth.json\"", outside.display()),
    );
    let _daemon = home.start();

    let output = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;

    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("other than a plain file"));
    assert!(!logins_of(&home, "codex", "work").join("login").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_again_replaces_the_login() {
    let home = Home::new();
    home.write_owner_config(WORK);
    codex_signing_in(&home, "printf 'first' > \"$HOME/.codex/auth.json\"");
    let _daemon = home.start();
    let first = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;
    assert!(first.status.success(), "{}", text(&first.stderr));
    codex_signing_in(&home, &writes_login());

    let second = home
        .cockpit(&["account", "sign-in", "codex", "work"], &[])
        .await;

    assert!(second.status.success(), "{}", text(&second.stderr));
    let login = logins_of(&home, "codex", "work").join("login/auth.json");
    assert_eq!(std::fs::read_to_string(login).unwrap(), FAKE_LOGIN);
}
