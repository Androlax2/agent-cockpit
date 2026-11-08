//! #59, #858: the owner lets one project's sessions use a login account with
//! `torlek account grant`, decided like a network grant (#674): when and through which client
//! are recorded, and `revoke` takes it back. Through the real daemon and CLI binaries.

mod candidate;
mod real_daemon;

use real_daemon::Home;

const LISTED: &str = "[accounts]\ncodex = [\"work\"]\nopencode = [\"work\"]\n";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A home whose owner lists `work` for Codex and OpenCode, with the project `cockpit`.
async fn started() -> (Home, real_daemon::Daemon) {
    let home = Home::new();
    home.write_owner_config(LISTED);
    let daemon = home.start();
    let client = home.connect().await;
    home.project(&client, "cockpit").await;
    (home, daemon)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_names_the_project_the_login_and_who_granted_it_through_which_client() {
    let (home, _daemon) = started().await;

    let output = home
        .cockpit(&["account", "grant", "cockpit", "codex", "work"], &[])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("project cockpit uses the codex provider's account work"),
        "{stdout}"
    );
    assert!(
        stdout.contains("granted by the owner through the CLI at"),
        "{stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_active_grants_are_listed() {
    let (home, _daemon) = started().await;
    home.cockpit(&["account", "grant", "cockpit", "codex", "work"], &[])
        .await;

    let output = home.cockpit(&["account", "grants"], &[]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "grant\t1\tcockpit\tcodex\twork\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_grant_is_no_longer_listed() {
    let (home, _daemon) = started().await;
    home.cockpit(&["account", "grant", "cockpit", "codex", "work"], &[])
        .await;

    let revoked = home.cockpit(&["account", "revoke", "1"], &[]).await;

    assert!(revoked.status.success(), "{}", text(&revoked.stderr));
    assert!(text(&revoked.stdout).contains("revoked by the owner through the CLI at"));
    let listed = home.cockpit(&["account", "grants", "cockpit"], &[]).await;
    assert_eq!(text(&listed.stdout), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn granting_a_login_the_project_holds_already_is_refused_naming_the_grant() {
    let (home, _daemon) = started().await;
    home.cockpit(&["account", "grant", "cockpit", "codex", "work"], &[])
        .await;

    let again = home
        .cockpit(&["account", "grant", "cockpit", "codex", "work"], &[])
        .await;

    assert!(!again.status.success());
    let stderr = text(&again.stderr);
    assert!(stderr.contains("through grant 1"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_the_owner_does_not_list_is_refused_naming_the_setting() {
    let (home, _daemon) = started().await;

    let output = home
        .cockpit(&["account", "grant", "cockpit", "codex", "spare"], &[])
        .await;

    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("accounts.codex"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_that_is_not_registered_is_refused() {
    let (home, _daemon) = started().await;

    let output = home
        .cockpit(&["account", "grant", "website", "codex", "work"], &[])
        .await;

    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("website"), "{stderr}");
    let listed = home.cockpit(&["account", "grants"], &[]).await;
    assert_eq!(text(&listed.stdout), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_with_no_login_to_keep_is_refused_naming_it() {
    let (home, _daemon) = started().await;

    let output = home
        .cockpit(&["account", "grant", "cockpit", "opencode", "work"], &[])
        .await;

    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("the opencode adapter has no sign-in a login account can keep"),
        "{stderr}"
    );
}
