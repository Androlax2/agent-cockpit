//! #767: ending a session sweeps the clone it occupied, keeps one holding work with its
//! `target/` when it has unpushed commits, records the outcome in `torlek storage`, and the
//! owner's setting turns it off.
use super::*;
use cockpit_protocol::{EndSweepView, PruneVerdictView};

/// Starts a session in a new clone on `branch` of a fresh project, with a built `target/`
/// Git ignores.
async fn session_in_clone(
    home: &Home,
    client: &Connection,
    branch: &str,
) -> (SessionView, PathBuf) {
    let root = home.project(client).await;
    let session = start_in(client, &root, Some(branch)).await;
    home.launches(1).await;
    let clone = PathBuf::from(&session.working_dir);
    std::fs::create_dir(clone.join("target")).unwrap();
    std::fs::write(clone.join("target/output"), b"fixture").unwrap();
    std::fs::write(clone.join(".git/info/exclude"), "/target/\n").unwrap();
    (session, clone)
}

async fn end(client: &Connection, session: &SessionView) {
    session_reply(
        client,
        Command::SessionEnd {
            session: session.id.clone(),
        },
    )
    .await;
}

async fn end_sweeps(client: &Connection) -> Vec<EndSweepView> {
    match request(client, Command::StorageReport).await {
        Ok(Reply::Storage { end_sweeps, .. }) => end_sweeps,
        other => panic!("{other:?}"),
    }
}

/// The record of the clone's sweep, once the daemon made it.
async fn swept(client: &Connection) -> EndSweepView {
    let deadline = Instant::now() + GENEROUS;
    loop {
        if let [sweep] = end_sweeps(client).await.as_slice() {
            return sweep.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the ended session's clone was not swept"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_whose_clone_is_clean_and_pushed_removes_the_clone() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let (session, clone) = session_in_clone(&home, &client, "work").await;

    end(&client, &session).await;
    let sweep = swept(&client).await;

    assert_eq!(sweep.kept, None, "{sweep:?}");
    assert_eq!(sweep.session, session.id);
    assert_eq!(sweep.checkout, session.checkout);
    assert_eq!(sweep.path, clone.display().to_string());
    assert!(!clone.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_whose_clone_has_unpushed_commits_keeps_the_clone_and_its_target() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let (session, clone) = session_in_clone(&home, &client, "work").await;
    std::fs::write(clone.join("draft"), "work\n").unwrap();
    git(&clone, &["add", "draft"]);
    git(&clone, &["commit", "--quiet", "-m", "local work"]);

    end(&client, &session).await;
    let sweep = swept(&client).await;

    let reason = sweep.kept.as_deref().unwrap_or_default();
    assert!(reason.starts_with("unpushed commits"), "{sweep:?}");
    assert!(reason.contains("not merged into origin/HEAD"), "{sweep:?}");
    assert_eq!(sweep.target, None, "the #758 rule keeps target/ too");
    assert!(clone.join("draft").exists());
    assert_eq!(
        std::fs::read(clone.join("target/output")).unwrap(),
        b"fixture"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_whose_clone_has_uncommitted_changes_keeps_it_and_says_what_became_of_target()
 {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let (session, clone) = session_in_clone(&home, &client, "work").await;
    std::fs::write(clone.join("draft"), "work\n").unwrap();

    end(&client, &session).await;
    let sweep = swept(&client).await;

    assert_eq!(sweep.kept.as_deref(), Some("uncommitted changes"));
    // The default prune policy is not automatic, so the owner confirms the prune.
    let target = sweep.target.as_deref().unwrap_or_default();
    assert!(target.starts_with("kept: "), "{sweep:?}");
    assert!(clone.join("draft").exists());
    assert_eq!(
        std::fs::read(clone.join("target/output")).unwrap(),
        b"fixture"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_setting_off_ending_a_session_removes_nothing() {
    let home = Home::new();
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[storage]\nsweep_on_session_end = false\n",
    )
    .unwrap();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let (session, clone) = session_in_clone(&home, &client, "work").await;

    end(&client, &session).await;
    // Many of the test daemon's 100 ms passes.
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(end_sweeps(&client).await, Vec::new());
    assert!(clone.join("target/output").exists());
}

/// #883: session `running` works in its own clone of the same project while `ended`, in
/// another clone, ends.
async fn one_running_one_ended(
    home: &Home,
    client: &Connection,
) -> (SessionView, SessionView, PathBuf) {
    let root = home.project(client).await;
    let running = start_in(client, &root, Some("busy")).await;
    let ended = start_in(client, &root, Some("done")).await;
    home.launches(2).await;
    let clone = PathBuf::from(&ended.working_dir);
    std::fs::create_dir(clone.join("target")).unwrap();
    std::fs::write(clone.join("target/output"), b"fixture").unwrap();
    std::fs::write(clone.join(".git/info/exclude"), "/target/\n").unwrap();
    (running, ended, clone)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_running_in_another_clone_does_not_keep_an_ended_sessions_clean_clone() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let (running, ended, clone) = one_running_one_ended(&home, &client).await;

    end(&client, &ended).await;
    let sweep = swept(&client).await;

    assert_eq!(sweep.kept, None, "{sweep:?}");
    assert_eq!(sweep.session, ended.id);
    assert!(!clone.exists());
    assert!(PathBuf::from(&running.working_dir).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_running_in_another_clone_is_not_the_reason_an_ended_clones_target_is_kept() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let (running, ended, clone) = one_running_one_ended(&home, &client).await;
    std::fs::write(clone.join("draft"), "work\n").unwrap();

    end(&client, &ended).await;
    let sweep = swept(&client).await;

    assert_eq!(sweep.kept.as_deref(), Some("uncommitted changes"));
    let target = sweep.target.as_deref().unwrap_or_default();
    assert!(!target.contains(running.id.as_str()), "{sweep:?}");
    // The default prune policy is not automatic, so the owner confirms the prune.
    assert!(target.starts_with("kept: "), "{sweep:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_storage_report_names_a_running_session_only_on_its_own_clone() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let (running, ended, clone) = one_running_one_ended(&home, &client).await;
    let running_clone = PathBuf::from(&running.working_dir);
    std::fs::create_dir(running_clone.join("target")).unwrap();
    std::fs::write(running_clone.join("target/output"), b"fixture").unwrap();
    std::fs::write(running_clone.join(".git/info/exclude"), "/target/\n").unwrap();
    std::fs::write(clone.join("draft"), "work\n").unwrap();
    end(&client, &ended).await;
    swept(&client).await;

    let clones = match request(&client, Command::StorageReport).await {
        Ok(Reply::Storage { clones, .. }) => clones,
        other => panic!("{other:?}"),
    };

    let verdict = |checkout: i64| {
        let clone = clones
            .iter()
            .find(|clone| clone.checkout == checkout)
            .unwrap_or_else(|| panic!("checkout {checkout} in {clones:?}"));
        clone.prune.clone()
    };
    assert!(
        matches!(verdict(running.checkout), Some(PruneVerdictView::Kept { reason }) if reason.contains(running.id.as_str())),
        "{clones:?}"
    );
    assert_eq!(
        verdict(ended.checkout),
        Some(PruneVerdictView::Eligible {
            confirmation_required: true
        }),
        "{clones:?}"
    );
}
