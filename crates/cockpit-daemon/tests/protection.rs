//! #695: `torlek project protect` sets up the default branch's protection once, a second run
//! changes nothing, the token the owner types is used for that call and never stored, a forge
//! adapter that cannot do it answers typed, and `torlek doctor --project DIR` reports the
//! branch's state either way. Through the real daemon and CLI with a fake `gh` that keeps the
//! repository's rules in a file; nothing reaches GitHub.

mod candidate;
mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use real_daemon::Home;

/// A fine-grained token's shape; never a real one.
const API_TOKEN: &str = "github_pat_11FAKEAPI0fakefakefake";
/// The token the owner types for protect alone: the fake accepts a ruleset write with it only.
const ADMIN_TOKEN: &str = "github_pat_11FAKEADMIN0fakefakefake";

/// The branch's rules once Torlek's ruleset is on it, as `rules/branches/main` lists them.
const PROTECTED_RULES: &str = r#"[{"type":"deletion","ruleset_id":42},{"type":"non_fast_forward","ruleset_id":42},{"type":"pull_request","parameters":{"required_approving_review_count":0},"ruleset_id":42},{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"ci"}]},"ruleset_id":42}]"#;
const RULESETS: &str = r#"[{"id":42,"name":"torlek-default-branch","source_type":"Repository"}]"#;
const WRITTEN: &str = r#"{"id":42,"name":"torlek-default-branch","enforcement":"active"}"#;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

/// The folder the fake `gh` keeps its log and the repository's state in.
fn forge_dir(home: &Home) -> PathBuf {
    home.path().join("fake-forge")
}

/// A fake `gh` first on the daemon's `PATH`: it logs each call's arguments and `GH_TOKEN` and
/// answers the protection read and write of `owner/cockpit`. A ruleset write with
/// [`ADMIN_TOKEN`] marks the branch protected; one with another token is refused as GitHub
/// refuses a token without administration. With `can_run` false its `--version` fails, as a
/// `gh` that cannot run does.
fn fake_gh(home: &Home, can_run: bool) {
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let state = forge_dir(home);
    std::fs::create_dir_all(&state).unwrap();
    for (name, contents) in [
        ("rules.json", PROTECTED_RULES),
        ("rulesets.json", RULESETS),
        ("written.json", WRITTEN),
        ("admin-token", ADMIN_TOKEN),
    ] {
        std::fs::write(state.join(name), contents).unwrap();
    }
    let version = if can_run {
        "printf 'gh version 2.104.0 (2026-11-12)\\n'"
    } else {
        "echo 'gh: broken' >&2; exit 1"
    };
    let script = format!(
        r#"#!/bin/sh
state={state}
printf '%s token=%s\n' "$*" "${{GH_TOKEN-}}" >> "$state/gh.log"
protected() {{ [ -e "$state/protected" ]; }}
case "$*" in
  "--version") {version} ;;
  "api repos/owner/cockpit --jq=.default_branch") printf 'main\n' ;;
  "api repos/owner/cockpit/rules/branches/main?per_page=100")
    if protected; then cat "$state/rules.json"; else printf '[]\n'; fi ;;
  "api repos/owner/cockpit/branches/main --jq=.protected")
    if protected; then echo true; else echo false; fi ;;
  "api repos/owner/cockpit/rulesets?includes_parents=false&per_page=100")
    if protected; then cat "$state/rulesets.json"; else printf '[]\n'; fi ;;
  "api repos/owner/cockpit/rulesets/42") cat "$state/written-body.json" ;;
  "api --method=POST repos/owner/cockpit/rulesets --input=-")
    if [ "${{GH_TOKEN-}}" != "$(cat "$state/admin-token")" ]; then
      echo 'gh: Resource not accessible by personal access token (HTTP 403)' >&2; exit 1
    fi
    cat > "$state/written-body.json"; touch "$state/protected"; cat "$state/written.json" ;;
  *) exit 97 ;;
esac
"#,
        state = quoted(&state),
    );
    let program = bin.join("gh");
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A home with the project `cockpit` on `https://github.com/owner/cockpit.git`, its API token
/// and a fake `gh`; with the project's root.
async fn started(can_run: bool) -> (Home, real_daemon::Daemon, PathBuf) {
    let home = Home::new();
    fake_gh(&home, can_run);
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "cockpit").await;
    let added = std::process::Command::new("git")
        .current_dir(&root)
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/cockpit.git",
        ])
        .output()
        .unwrap();
    assert!(added.status.success(), "{}", text(&added.stderr));
    home.write_github_api_token("cockpit", API_TOKEN, 0o600);
    (home, daemon, root)
}

async fn protect(home: &Home, token: &str) -> std::process::Output {
    home.cockpit_with_stdin(
        &["project", "protect", "cockpit", "--check", "ci"],
        &format!("{token}\n"),
    )
    .await
}

fn gh_calls(home: &Home) -> String {
    std::fs::read_to_string(forge_dir(home).join("gh.log")).unwrap_or_default()
}

fn ruleset_writes(home: &Home) -> Vec<String> {
    gh_calls(home)
        .lines()
        .filter(|call| call.contains("--method="))
        .map(str::to_owned)
        .collect()
}

/// `doctor`'s `protection` line and the `next:` line after it, if any.
async fn doctor_protection(home: &Home, root: &Path) -> String {
    let output = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;
    let report = text(&output.stdout);
    let lines: Vec<&str> = report.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.contains(" protection: "))
        .unwrap_or_else(|| panic!("no protection line in:\n{report}\n{}", text(&output.stderr)));
    let mut found = lines[at].to_owned();
    if let Some(next) = lines
        .get(at + 1)
        .filter(|line| line.trim_start().starts_with("next:"))
    {
        found.push('\n');
        found.push_str(next);
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn protect_creates_the_rule_once_and_a_second_run_changes_nothing() {
    let (home, _daemon, _) = started(true).await;

    let first = protect(&home, ADMIN_TOKEN).await;
    let second = protect(&home, ADMIN_TOKEN).await;

    assert!(first.status.success(), "{}", text(&first.stderr));
    assert!(
        text(&first.stdout).contains("cockpit main: protected: Torlek's rule was created"),
        "{}",
        text(&first.stdout)
    );
    assert!(second.status.success(), "{}", text(&second.stderr));
    assert!(
        text(&second.stdout).contains("cockpit main: already protected: nothing was changed"),
        "{}",
        text(&second.stdout)
    );
    assert_eq!(ruleset_writes(&home).len(), 1, "{}", gh_calls(&home));
}

#[tokio::test(flavor = "multi_thread")]
async fn protect_writes_with_the_typed_token_and_the_owners_checks() {
    let (home, _daemon, _) = started(true).await;

    let output = protect(&home, ADMIN_TOKEN).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let writes = ruleset_writes(&home);
    assert!(
        writes[0].ends_with(&format!("token={ADMIN_TOKEN}")),
        "{writes:?}"
    );
    let body = std::fs::read_to_string(forge_dir(&home).join("written-body.json")).unwrap();
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        body["rules"][3]["parameters"]["required_status_checks"],
        serde_json::json!([{ "context": "ci" }])
    );
    assert_eq!(body["bypass_actors"], serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_typed_token_is_stored_nowhere() {
    let (home, _daemon, _) = started(true).await;

    let output = protect(&home, ADMIN_TOKEN).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let holding = files_holding(&home.path(), ADMIN_TOKEN.as_bytes(), &forge_dir(&home));
    assert_eq!(holding, Vec::<PathBuf>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_without_the_right_to_change_settings_is_refused_naming_what_it_needs() {
    let (home, _daemon, _) = started(true).await;

    let output = protect(&home, API_TOKEN).await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(
        refused.contains("cockpit's default branch was not protected"),
        "{refused}"
    );
    assert!(refused.contains("HTTP 403"), "{refused}");
    assert!(refused.contains("administration write"), "{refused}");
    assert!(!refused.contains(API_TOKEN), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forge_adapter_that_cannot_protect_answers_typed() {
    let (home, _daemon, _) = started(false).await;

    let output = protect(&home, ADMIN_TOKEN).await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(
        refused.contains("does not report the `protection-bootstrap` capability"),
        "{refused}"
    );
    assert!(ruleset_writes(&home).is_empty(), "{}", gh_calls(&home));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_project_is_refused_before_any_token_is_read() {
    let (home, _daemon, _) = started(true).await;

    let output = home
        .cockpit_with_stdin(
            &["project", "protect", "typo", "--check", "ci"],
            ADMIN_TOKEN,
        )
        .await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(refused.contains("no project is named typo"), "{refused}");
    assert!(!refused.contains("not shown"), "{refused}");
    assert_eq!(gh_calls(&home), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn protect_without_a_check_is_refused_before_any_token_is_read() {
    let (home, _daemon, _) = started(true).await;

    let output = home
        .cockpit_with_stdin(&["project", "protect", "cockpit"], ADMIN_TOKEN)
        .await;

    assert!(!output.status.success());
    let refused = text(&output.stderr);
    assert!(refused.contains("--check NAME"), "{refused}");
    assert!(!gh_calls(&home).contains("api"), "{}", gh_calls(&home));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_reports_an_unprotected_default_branch_with_the_command_that_fixes_it() {
    let (home, _daemon, root) = started(true).await;

    let line = doctor_protection(&home, &root).await;

    assert!(line.starts_with("needs-authorization"), "{line}");
    assert!(
        line.contains(
            "protection: main is not protected: no change request required, no check \
             required, force pushes allowed, deletion allowed"
        ),
        "{line}"
    );
    assert!(
        line.contains("next: `torlek project protect cockpit --check NAME`"),
        "{line}"
    );
    let reads = gh_calls(&home);
    assert!(
        reads
            .lines()
            .filter(|call| call.starts_with("api "))
            .all(|call| call.ends_with(&format!("token={API_TOKEN}"))),
        "{reads}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_reports_a_protected_default_branch_as_available() {
    let (home, _daemon, root) = started(true).await;
    let output = protect(&home, ADMIN_TOKEN).await;
    assert!(output.status.success(), "{}", text(&output.stderr));

    let line = doctor_protection(&home, &root).await;

    assert!(line.starts_with("available"), "{line}");
    assert!(
        line.contains("protection: main takes a change request with ci passing"),
        "{line}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_reports_an_adapter_that_cannot_read_protection_without_blocking() {
    let (home, _daemon, root) = started(false).await;

    let line = doctor_protection(&home, &root).await;

    assert!(line.starts_with("unknown"), "{line}");
    assert!(
        line.contains("does not report the `protection-read` capability"),
        "{line}"
    );
}

/// Every regular file under `dir`, outside `skipped`, whose bytes hold `needle`.
fn files_holding(dir: &Path, needle: &[u8], skipped: &Path) -> Vec<PathBuf> {
    let mut holding = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return holding;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.starts_with(skipped) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            holding.extend(files_holding(&path, needle, skipped));
        } else if kind.is_file()
            && std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
        {
            holding.push(path);
        }
    }
    holding
}
