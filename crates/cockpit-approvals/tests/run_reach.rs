//! What a confined run's card names beyond its checkout (#1387): the folders it may also
//! write and the hosts it may reach. The owner approves them with the command, so the
//! digest binds them; the record keeps the limits the run hit.

mod common;

use std::path::PathBuf;

use cockpit_approvals::{
    Error, Output, Reach, Refusal, RunAsk, RunResult, action_digest, approve_run, ask_run,
    ask_run_unless_pending, finish_run, resolve_write, run_request, start_run,
};
use cockpit_processes::ExecutionId;
use common::{Fixture, NOW, session};

const AFTER_GRACE: i64 = NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS;

fn reach(writes: &[&str], hosts: &[&str]) -> Reach {
    Reach {
        writes: writes.iter().map(PathBuf::from).collect(),
        hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
    }
}

fn reaching(fixture: &Fixture, reach: Reach) -> RunAsk {
    RunAsk {
        reach,
        ..fixture.ask("make install")
    }
}

async fn asked(fixture: &Fixture, ask: RunAsk) -> cockpit_approvals::RunRequest {
    fixture.grant(&session("s1")).await;
    let write = fixture.database.write().await.unwrap();
    let run = ask_run(&write, &session("s1"), ask, NOW).await.unwrap();
    write.commit().await.unwrap();
    run
}

#[tokio::test]
async fn a_request_keeps_the_folders_and_hosts_its_card_names() {
    let fixture = Fixture::new().await;
    let named = reach(
        &["/home/owner/.cache/build"],
        &["example.com", "127.0.0.2:8080"],
    );

    let run = asked(&fixture, reaching(&fixture, named.clone())).await;

    let read = fixture.database.read().await.unwrap();
    let stored = run_request(&read, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(stored.reach, named);
}

#[tokio::test]
async fn the_digest_binds_what_the_card_names() {
    let fixture = Fixture::new().await;
    let plain = asked(&fixture, fixture.ask("make install")).await;
    let reaching_hosts = asked(&fixture, reaching(&fixture, reach(&[], &["example.com"]))).await;
    let reaching_writes = asked(&fixture, reaching(&fixture, reach(&["/srv/out"], &[]))).await;

    assert_eq!(
        plain.digest(),
        action_digest(&fixture.cwd(), "make install")
    );
    assert_ne!(reaching_hosts.digest(), plain.digest());
    assert_ne!(reaching_writes.digest(), plain.digest());
    assert_ne!(reaching_writes.digest(), reaching_hosts.digest());
}

#[tokio::test]
async fn an_approval_names_the_card_it_was_shown() {
    let fixture = Fixture::new().await;
    let run = asked(&fixture, reaching(&fixture, reach(&[], &["example.com"]))).await;
    let write = fixture.database.write().await.unwrap();

    let without_the_card = approve_run(
        &write,
        None,
        run.id,
        1,
        &action_digest(&fixture.cwd(), "make install"),
        AFTER_GRACE,
    )
    .await;
    let (approved, _) = approve_run(&write, None, run.id, 1, &run.digest(), AFTER_GRACE)
        .await
        .unwrap();

    assert!(
        matches!(
            without_the_card,
            Err(Error::Refused(Refusal::DigestMismatch))
        ),
        "{without_the_card:?}"
    );
    assert_eq!(approved.reach, reach(&[], &["example.com"]));
}

#[tokio::test]
async fn the_same_command_naming_other_hosts_is_asked_again() {
    let fixture = Fixture::new().await;
    fixture.grant(&session("s1")).await;
    let write = fixture.database.write().await.unwrap();
    let first = ask_run_unless_pending(
        &write,
        &session("s1"),
        reaching(&fixture, reach(&[], &["example.com"])),
        NOW,
    )
    .await
    .unwrap();

    let second = ask_run_unless_pending(
        &write,
        &session("s1"),
        reaching(&fixture, reach(&[], &["example.org"])),
        NOW,
    )
    .await
    .unwrap();

    assert!(!second.already_pending);
    assert_ne!(second.run.id, first.run.id);
}

#[tokio::test]
async fn a_card_that_names_no_usable_folder_or_host_names_the_field() {
    let fixture = Fixture::new().await;
    fixture.grant(&session("s1")).await;
    let many_hosts: Vec<String> = (0..17).map(|index| format!("h{index}.example")).collect();
    let many_hosts: Vec<&str> = many_hosts.iter().map(String::as_str).collect();
    let cases = [
        ("writes", reach(&["relative/out"], &[])),
        ("writes", reach(&["/srv/out\nmore"], &[])),
        ("hosts", reach(&[], &[""])),
        ("hosts", reach(&[], &["example.com\nexample.org"])),
        ("hosts", reach(&[], &many_hosts)),
    ];
    for (expected_field, named) in cases {
        let write = fixture.database.write().await.unwrap();
        match ask_run(&write, &session("s1"), reaching(&fixture, named), NOW).await {
            Err(Error::Refused(Refusal::InvalidRequest { field, .. })) => {
                assert_eq!(field, expected_field)
            }
            other => panic!("expected an invalid {expected_field}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_finished_run_keeps_the_limits_it_hit() {
    let fixture = Fixture::new().await;
    let run = asked(&fixture, fixture.ask("cat ~/.ssh/id_ed25519")).await;
    let write = fixture.database.write().await.unwrap();
    let (approved, _) = approve_run(&write, None, run.id, 1, &run.digest(), AFTER_GRACE)
        .await
        .unwrap();
    start_run(&write, &approved, ExecutionId(7)).await.unwrap();
    let hits = vec![
        "tried to read /home/owner/.ssh/id_ed25519, which confined runs cannot see".to_owned(),
    ];
    let result = RunResult {
        exit_code: 1,
        output: Output {
            shown: "Permission denied".to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
        },
        output_retired: false,
        stopped_by_owner: false,
        sudo_ran_unasked: 0,
        typed_secret_redacted: 0,
        limit_hits: hits.clone(),
    };

    finish_run(&write, &approved, &result, NOW).await.unwrap();

    let stored = run_request(&write, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(stored.result.unwrap().limit_hits, hits);
}

#[test]
fn a_folder_to_write_is_resolved_where_it_leads() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("out");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();

    let resolved = resolve_write(dir.path().join("link").to_str().unwrap()).unwrap();

    assert_eq!(resolved, real.canonicalize().unwrap());
}

#[test]
fn a_folder_to_write_that_is_missing_names_the_folder() {
    let missing = "/nonexistent/torlek-test-out";

    match resolve_write(missing) {
        Err(Refusal::InvalidRequest { field, reason }) => {
            assert_eq!(field, "writes");
            assert!(reason.starts_with(missing), "{reason}");
        }
        other => panic!("expected an invalid write, got {other:?}"),
    }
}
