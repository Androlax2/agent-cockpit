//! #1387, part 3: an "always" approval covers only its exact command line, directory, card,
//! checkout and files with their hashes, on a real state database. A changed file or argument
//! asks again; a request whose files cannot all be told is never approved always.

mod common;

use cockpit_approvals::{
    AlwaysApproval, DecidedThrough, Error, Finding, Project, Reach, Refusal, RootFile, RootRuns,
    RunAsk, RunRequest, RunState, always_approvals, approve_always, approve_by_always, approve_run,
    ask_run, pinning_incomplete, record_checks, revoke_always,
};
use common::{Fixture, NOW, session};

const AFTER_GRACE: i64 = NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS;
const SCRIPT_HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const CHANGED_HASH: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> Refusal {
    match result {
        Err(Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// What one ask and its checks name.
#[derive(Clone)]
struct Asked {
    command: &'static str,
    project: Option<&'static str>,
    hosts: Vec<String>,
    pinned: Vec<RootFile>,
    root_runs: RootRuns,
    findings: Vec<Finding>,
}

fn script(fixture: &Fixture, sha256: &str) -> Vec<RootFile> {
    vec![RootFile {
        path: fixture.cwd().join("x.sh"),
        sha256: Some(sha256.to_owned()),
    }]
}

fn known() -> RootRuns {
    RootRuns {
        editable: Vec::new(),
        cannot_tell: false,
    }
}

/// `sh ./x.sh` in project `app`, its script pinned at [`SCRIPT_HASH`].
fn build(fixture: &Fixture) -> Asked {
    Asked {
        command: "sh ./x.sh",
        project: Some("app"),
        hosts: Vec::new(),
        pinned: script(fixture, SCRIPT_HASH),
        root_runs: known(),
        findings: Vec::new(),
    }
}

/// Files `asked` from session `s1` and records its checks, as the daemon does.
async fn filed(fixture: &Fixture, asked: &Asked) -> RunRequest {
    let asker = session("s1");
    fixture.grant(&asker).await;
    let write = fixture.database.write().await.unwrap();
    let ask = RunAsk {
        command: asked.command.to_owned(),
        cwd: fixture.cwd(),
        reason: "Builds the project.".to_owned(),
        project: asked.project.map(|name| Project {
            name: name.to_owned(),
            root: fixture.cwd(),
        }),
        reach: Reach {
            writes: Vec::new(),
            hosts: asked.hosts.clone(),
        },
    };
    let run = ask_run(&write, &asker, ask, NOW).await.unwrap();
    let run = record_checks(
        &write,
        run.id,
        run.revision,
        &asked.root_runs,
        &asked.findings,
        &asked.pinned,
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    run
}

/// The owner approves `run` always, through the TUI.
async fn approved_always(fixture: &Fixture, run: &RunRequest) -> Result<AlwaysApproval, Error> {
    let write = fixture.database.write().await.unwrap();
    let (_, approved) = approve_run(
        &write,
        Some(DecidedThrough::Tui),
        run.id,
        run.revision,
        &run.digest(),
        AFTER_GRACE,
    )
    .await
    .unwrap();
    let always = approve_always(&write, &approved, Some(DecidedThrough::Tui), AFTER_GRACE).await?;
    write.commit().await.unwrap();
    Ok(always)
}

/// Files `asked` and tries the "always" approvals on it, as the daemon does at each ask.
async fn asked_again(fixture: &Fixture, asked: &Asked) -> RunRequest {
    let run = filed(fixture, asked).await;
    let write = fixture.database.write().await.unwrap();
    let approved = approve_by_always(&write, &run, AFTER_GRACE).await.unwrap();
    write.commit().await.unwrap();
    match approved {
        Some((approved, run)) => {
            assert_eq!(approved.command, run.command);
            run
        }
        None => run,
    }
}

#[tokio::test]
async fn a_later_ask_of_the_same_command_and_files_is_approved_without_asking() {
    let fixture = Fixture::new().await;
    let asked = build(&fixture);
    let always = approved_always(&fixture, &filed(&fixture, &asked).await)
        .await
        .unwrap();

    let again = asked_again(&fixture, &asked).await;

    assert_eq!(again.state, RunState::Approved);
    let decision = again.decision.expect("approved");
    assert_eq!(decision.always, Some(always.id));
    assert_eq!(decision.through, None, "the owner did not decide it");
    assert_eq!(always.files, asked.pinned);
}

#[tokio::test]
async fn an_ask_that_differs_in_anything_the_always_approval_names_asks_again() {
    let fixture = Fixture::new().await;
    let asked = build(&fixture);
    approved_always(&fixture, &filed(&fixture, &asked).await)
        .await
        .unwrap();
    let variants = [
        (
            "a changed script",
            Asked {
                pinned: script(&fixture, CHANGED_HASH),
                ..build(&fixture)
            },
        ),
        (
            "another argument",
            Asked {
                command: "sh ./x.sh --release",
                ..build(&fixture)
            },
        ),
        (
            "another host on the card",
            Asked {
                hosts: vec!["example.com".to_owned()],
                ..build(&fixture)
            },
        ),
        (
            "another project",
            Asked {
                project: Some("other"),
                ..build(&fixture)
            },
        ),
        (
            "a file it did not run before",
            Asked {
                pinned: [
                    script(&fixture, SCRIPT_HASH),
                    vec![RootFile {
                        path: fixture.cwd().join("y.sh"),
                        sha256: None,
                    }],
                ]
                .concat(),
                ..build(&fixture)
            },
        ),
    ];

    for (what, variant) in &variants {
        let again = asked_again(&fixture, variant).await;
        assert_eq!(again.state, RunState::Asking, "{what}");
        assert_eq!(again.decision, None, "{what}");
    }
}

#[tokio::test]
async fn an_ask_whose_files_could_not_be_pinned_is_not_approved_by_an_always_without_files() {
    let fixture = Fixture::new().await;
    let without_files = Asked {
        command: "cargo build",
        pinned: Vec::new(),
        ..build(&fixture)
    };
    approved_always(&fixture, &filed(&fixture, &without_files).await)
        .await
        .unwrap();

    let timed_out = Asked {
        findings: vec![pinning_incomplete(false)],
        ..without_files
    };
    let again = asked_again(&fixture, &timed_out).await;

    assert_eq!(again.state, RunState::Asking);
}

#[tokio::test]
async fn a_request_that_cannot_be_narrowed_is_never_approved_always() {
    let fixture = Fixture::new().await;
    let cases = [
        (
            "some files could not be pinned",
            Asked {
                findings: vec![pinning_incomplete(true)],
                ..build(&fixture)
            },
            "Torlek cannot tell every file it runs, so a changed file would not ask again",
        ),
        (
            "what sudo runs cannot be told",
            Asked {
                root_runs: RootRuns {
                    editable: Vec::new(),
                    cannot_tell: true,
                },
                ..build(&fixture)
            },
            "Torlek cannot tell every file it runs, so a changed file would not ask again",
        ),
        (
            "no project",
            Asked {
                project: None,
                ..build(&fixture)
            },
            "it was not asked from a registered project's checkout",
        ),
    ];

    for (what, asked, why) in cases {
        let run = filed(&fixture, &asked).await;
        assert_eq!(
            refusal(approved_always(&fixture, &run).await),
            Refusal::NotAlways {
                why: why.to_owned()
            },
            "{what}"
        );
        let read = fixture.database.read().await.unwrap();
        let stored = cockpit_approvals::run_request(&read, run.id, NOW)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, RunState::Asking, "{what}: nothing approved");
    }
    let read = fixture.database.read().await.unwrap();
    assert_eq!(always_approvals(&read).await.unwrap(), Vec::new());
}

#[tokio::test]
async fn a_revoked_always_approval_approves_nothing_more() {
    let fixture = Fixture::new().await;
    let asked = build(&fixture);
    let always = approved_always(&fixture, &filed(&fixture, &asked).await)
        .await
        .unwrap();
    let write = fixture.database.write().await.unwrap();

    let revoked = revoke_always(&write, always.id, Some(DecidedThrough::Cli), AFTER_GRACE)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(revoked.revoked.map(|act| act.at), Some(AFTER_GRACE));
    assert_eq!(asked_again(&fixture, &asked).await.state, RunState::Asking);
    let read = fixture.database.read().await.unwrap();
    assert_eq!(always_approvals(&read).await.unwrap(), Vec::new());
}

#[tokio::test]
async fn revoking_an_always_approval_twice_is_refused() {
    let fixture = Fixture::new().await;
    let always = approved_always(&fixture, &filed(&fixture, &build(&fixture)).await)
        .await
        .unwrap();
    let write = fixture.database.write().await.unwrap();
    revoke_always(&write, always.id, None, AFTER_GRACE)
        .await
        .unwrap();

    assert_eq!(
        refusal(revoke_always(&write, always.id, None, AFTER_GRACE).await),
        Refusal::AlwaysRevoked
    );
    assert_eq!(
        refusal(revoke_always(&write, always.id + 1, None, AFTER_GRACE).await),
        Refusal::NotFound {
            kind: "always approval",
            id: always.id + 1
        }
    );
}

#[tokio::test]
async fn approving_always_after_the_script_changed_replaces_the_approval_before() {
    let fixture = Fixture::new().await;
    let first = approved_always(&fixture, &filed(&fixture, &build(&fixture)).await)
        .await
        .unwrap();
    let changed = Asked {
        pinned: script(&fixture, CHANGED_HASH),
        ..build(&fixture)
    };

    let second = approved_always(&fixture, &filed(&fixture, &changed).await)
        .await
        .unwrap();

    let read = fixture.database.read().await.unwrap();
    assert_eq!(always_approvals(&read).await.unwrap(), vec![second.clone()]);
    drop(read);
    assert_ne!(first.id, second.id);
    assert_eq!(
        asked_again(&fixture, &build(&fixture)).await.state,
        RunState::Asking,
        "the old script's approval was replaced"
    );
    assert_eq!(
        asked_again(&fixture, &changed).await.state,
        RunState::Approved
    );
    assert_eq!(
        second.files.first().map(|file| file.path.clone()),
        Some(fixture.cwd().join("x.sh"))
    );
}
