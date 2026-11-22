//! #60: the owner reads a project's open issues beside its todos with `torlek todo issues`,
//! through the forge contract and the project's API token, for reads only (#857 as amended on
//! 2026-11-22). Through the real daemon and CLI with a fake `gh`; nothing reaches GitHub.

mod candidate;
mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use real_daemon::Home;

/// A fine-grained token's shape; never a real one.
const API_TOKEN: &str = "github_pat_11FAKEAPI0fakefakefake";
const PUSH_TOKEN: &str = "github_pat_11FAKEPUSH0fakefakefake";

/// Two open issues as `gh issue list --json=number,title,state,labels,author,url` prints them;
/// the second's title carries a terminal sequence and a line break.
const LISTED: &str = r#"[{"author":{"login":"octo"},"labels":[{"name":"greenfield"},{"name":"alpha"}],"number":12,"state":"OPEN","title":"Read issues beside todos","url":"https://github.com/owner/cockpit/issues/12"},{"author":{"login":"ghost"},"labels":[],"number":7,"state":"OPEN","title":"\u001b[31mred\nfake\tline\u202emoc.evil","url":"https://github.com/owner/cockpit/issues/7"}]"#;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

/// A fake `gh` first on the daemon's `PATH`: it logs each call's arguments and `GH_TOKEN`,
/// answers `--version` as `gh` 2.104.0 and `issue list` with [`LISTED`].
fn fake_gh(home: &Home) -> PathBuf {
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = home.path().join("gh.log");
    let listed = home.path().join("gh-issue-list.json");
    std::fs::write(&listed, LISTED).unwrap();
    let script = bin.join("gh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s token=%s\\n' \"$*\" \"${{GH_TOKEN-}}\" >> {log}\n\
             case \"$1\" in\n  --version) printf 'gh version 2.104.0 (2026-11-12)\\n' ;;\n  \
             issue) cat {listed} ;;\n  *) exit 97 ;;\nesac\n",
            log = quoted(&log),
            listed = quoted(&listed),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    log
}

/// A home with the project `cockpit`, whose origin is `origin`, and a fake `gh`; with the
/// fake's call log.
async fn started(origin: &str) -> (Home, real_daemon::Daemon, PathBuf) {
    let home = Home::new();
    let log = fake_gh(&home);
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "cockpit").await;
    let added = std::process::Command::new("git")
        .current_dir(&root)
        .args(["remote", "add", "origin", origin])
        .output()
        .unwrap();
    assert!(added.status.success(), "{}", text(&added.stderr));
    (home, daemon, log)
}

fn gh_calls(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_lists_a_projects_open_issues_beside_its_todos() {
    let (home, _daemon, _) = started("https://github.com/owner/cockpit.git").await;
    home.write_github_api_token("cockpit", API_TOKEN, 0o600);

    let output = home.cockpit(&["todo", "issues", "cockpit"], &[]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let listed = text(&output.stdout);
    assert_eq!(
        listed.lines().next(),
        Some(
            "#12\tRead issues beside todos\tgreenfield,alpha\thttps://github.com/owner/cockpit/issues/12"
        )
    );
    assert_eq!(listed.lines().count(), 2, "{listed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn issues_are_read_with_the_projects_api_token_and_never_its_other_token() {
    let (home, _daemon, log) = started("https://github.com/owner/cockpit.git").await;
    home.write_github_api_token("cockpit", API_TOKEN, 0o600);
    home.write_github_token("cockpit", PUSH_TOKEN, 0o600);

    let output = home.cockpit(&["todo", "issues", "cockpit"], &[]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let calls = gh_calls(&log);
    let read = calls
        .lines()
        .find(|call| call.starts_with("issue list "))
        .unwrap_or_else(|| panic!("no issue list in:\n{calls}"));
    assert!(read.contains("--repo=owner/cockpit"), "{read}");
    assert!(read.contains("--state=open"), "{read}");
    assert!(read.ends_with(&format!("token={API_TOKEN}")), "{read}");
    assert!(!calls.contains(PUSH_TOKEN), "{calls}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_title_from_the_forge_cannot_fake_a_line_a_terminal_sequence_or_its_reading_order() {
    let (home, _daemon, _) = started("https://github.com/owner/cockpit.git").await;
    home.write_github_api_token("cockpit", API_TOKEN, 0o600);

    let output = home.cockpit(&["todo", "issues", "cockpit"], &[]).await;

    let listed = text(&output.stdout);
    assert!(!listed.contains(['\u{1b}', '\u{202e}']), "{listed:?}");
    assert_eq!(
        listed.lines().nth(1),
        Some(
            "#7\t\\u{1b}[31mred\\nfake\\tline\\u{202e}moc.evil\t-\thttps://github.com/owner/cockpit/issues/7"
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_without_an_api_token_has_no_issues_read() {
    let (home, _daemon, log) = started("https://github.com/owner/cockpit.git").await;
    home.write_github_token("cockpit", PUSH_TOKEN, 0o600);

    let output = home.cockpit(&["todo", "issues", "cockpit"], &[]).await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(refused.contains("cockpit has no API token"), "{refused}");
    assert!(
        refused.contains("`torlek project token set cockpit --api`"),
        "{refused}"
    );
    assert_eq!(gh_calls(&log), "", "gh is never run");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ssh_origin_has_its_issues_refused_naming_ssh() {
    let (home, _daemon, log) = started("git@github.com:owner/cockpit.git").await;
    home.write_github_api_token("cockpit", API_TOKEN, 0o600);

    let output = home.cockpit(&["todo", "issues", "cockpit"], &[]).await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(refused.contains("SSH"), "{refused}");
    assert!(!gh_calls(&log).contains("issue list"), "{}", gh_calls(&log));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_project_is_not_found() {
    let (home, _daemon, _) = started("https://github.com/owner/cockpit.git").await;

    let output = home.cockpit(&["todo", "issues", "elsewhere"], &[]).await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(
        refused.contains("no project is named \"elsewhere\""),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_origin_naming_a_credential_is_refused_without_repeating_it() {
    let (home, _daemon, log) =
        started("https://x-access-token:SECRET@github.com/owner/cockpit.git").await;
    home.write_github_api_token("cockpit", API_TOKEN, 0o600);

    let output = home.cockpit(&["todo", "issues", "cockpit"], &[]).await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(refused.contains("names a user or a password"), "{refused}");
    assert!(!refused.contains("SECRET"), "{refused}");
    assert!(!gh_calls(&log).contains("issue list"), "{}", gh_calls(&log));
}

/// `gh --version` is the adapter's probe: it runs at the first read, not at each.
#[tokio::test(flavor = "multi_thread")]
async fn the_forge_program_is_probed_once_across_reads() {
    let (home, _daemon, log) = started("https://github.com/owner/cockpit.git").await;
    home.write_github_api_token("cockpit", API_TOKEN, 0o600);

    for _ in 0..2 {
        let output = home.cockpit(&["todo", "issues", "cockpit"], &[]).await;
        assert!(output.status.success(), "{}", text(&output.stderr));
    }

    let calls = gh_calls(&log);
    assert_eq!(
        calls
            .lines()
            .filter(|call| call.starts_with("--version"))
            .count(),
        1,
        "{calls}"
    );
    assert_eq!(
        calls
            .lines()
            .filter(|call| call.starts_with("issue list"))
            .count(),
        2,
        "{calls}"
    );
}
