//! Durable maintenance exclusion; fixtures never register or inspect a real checkout.
#![cfg(target_os = "linux")]
mod common;
use cockpit_projects::{
    Git, PruneEvidence, PrunePolicy, claim, create_parallel_clone, prepare_target_prune,
    preview_target_prune, require_execution_start_allowed, reserve_target_prune, sweep,
};
use common::{Scratch, holder, register};

#[tokio::test]
#[cfg(target_os = "linux")]
async fn cleanup_receipt_cannot_checkpoint_a_reused_operation_in_another_database_epoch() {
    use std::sync::Arc;
    let first = Scratch::new();
    let second = Scratch::new();
    let mut old_receipt = None;
    let mut old_status = None;
    for scratch in [&first, &second] {
        let database = Arc::new(scratch.open().await);
        let (project, _) = register(&database, &scratch.repository("app")).await;
        let clone = create_parallel_clone(
            &database,
            &Git::from_path(),
            project.name.as_str(),
            "feature",
        )
        .await
        .unwrap();
        std::fs::create_dir(clone.path.join("target")).unwrap();
        std::fs::write(clone.path.join("target/output"), b"keep").unwrap();
        std::fs::write(clone.path.join(".git/info/exclude"), "/target/\n").unwrap();
        let evidence = PruneEvidence {
            ended_sessions: Some("fixture-ended-session".into()),
            busy: None,
        };
        let policy = PrunePolicy::default();
        let preview =
            preview_target_prune(&database, &Git::from_path(), clone.id, &policy, &evidence)
                .await
                .unwrap();
        let write = database.write().await.unwrap();
        let plan = prepare_target_prune(&write, database.epoch(), &preview, &evidence)
            .await
            .unwrap();
        reserve_target_prune(
            &write,
            database.epoch(),
            plan.operation,
            plan.revision,
            &preview,
            &evidence,
            false,
        )
        .await
        .unwrap();
        let step = cockpit_projects::begin_prune_step(
            &write,
            database.epoch(),
            plan.operation,
            &policy,
            &evidence,
        )
        .await
        .unwrap();
        let before = cockpit_projects::prune_status(&write, plan.operation)
            .await
            .unwrap();
        write.commit().await.unwrap();
        if let Some(receipt) = old_receipt.take() {
            let old: cockpit_projects::PruneStatus = old_status.take().unwrap();
            assert_eq!(
                (old.operation, old.revision),
                (plan.operation, plan.revision)
            );
            assert_ne!(old.epoch, plan.epoch);
            let write = database.write().await.unwrap();
            assert!(
                cockpit_projects::complete_prune_step(&write, database.epoch(), receipt)
                    .await
                    .is_err(),
                "accepted a receipt from another database epoch"
            );
            assert_eq!(
                cockpit_projects::prune_status(&write, plan.operation)
                    .await
                    .unwrap(),
                before
            );
            write.rollback().await.unwrap();
            assert_eq!(
                std::fs::read(clone.path.join("target/output")).unwrap(),
                b"keep"
            );
            drop(step);
        } else {
            old_status = Some(plan);
            old_receipt = Some(
                step.run(Arc::clone(&database), Git::from_path())
                    .await
                    .unwrap(),
            );
        }
    }
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn automatic_prune_requires_complete_eligible_owner_policy_and_manual_confirm_stays_available()
 {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (project, _) = register(&database, &scratch.repository("app")).await;
    let clone = create_parallel_clone(
        &database,
        &Git::from_path(),
        project.name.as_str(),
        "feature",
    )
    .await
    .unwrap();
    std::fs::create_dir(clone.path.join("target")).unwrap();
    std::fs::write(clone.path.join("target/output"), b"12345678").unwrap();
    std::fs::write(clone.path.join(".git/info/exclude"), "/target/\n").unwrap();
    let ended = PruneEvidence {
        ended_sessions: Some("observed-fixture-termination".into()),
        busy: None,
    };
    for (automatic, projects, threshold, expected) in [
        (false, vec!["app".into()], 8, false),
        (true, vec![], 8, false),
        (true, vec!["other".into()], 8, false),
        (true, vec!["app".into()], 7, false),
        (true, vec!["app".into()], 8, true),
    ] {
        let policy = PrunePolicy {
            automatic,
            projects,
            confirm_above_bytes: threshold,
            ..Default::default()
        };
        let preview = preview_target_prune(&database, &Git::from_path(), clone.id, &policy, &ended)
            .await
            .unwrap();
        assert_eq!(preview.confirmation_required, !expected);
        let write = database.write().await.unwrap();
        let plan = prepare_target_prune(&write, database.epoch(), &preview, &ended)
            .await
            .unwrap();
        let result = reserve_target_prune(
            &write,
            database.epoch(),
            plan.operation,
            plan.revision,
            &preview,
            &ended,
            true,
        )
        .await;
        assert_eq!(result.is_ok(), expected, "{result:?}");
        if !expected {
            require_execution_start_allowed(&write).await.unwrap();
            reserve_target_prune(
                &write,
                database.epoch(),
                plan.operation,
                plan.revision,
                &preview,
                &ended,
                false,
            )
            .await
            .unwrap();
        }
        write.rollback().await.unwrap();
    }
    for case in [
        "unknown-end",
        "occupied",
        "hardlink",
        "symlink",
        "source-state",
        "nonignored",
        "tracked",
    ] {
        let evidence = if case == "unknown-end" {
            PruneEvidence::default()
        } else {
            ended.clone()
        };
        let mut held = false;
        match case {
            "occupied" => {
                let write = database.write().await.unwrap();
                claim(&write, clone.id, &holder("live-holder"))
                    .await
                    .unwrap();
                write.commit().await.unwrap();
                held = true;
            }
            "hardlink" => std::fs::hard_link(
                clone.path.join("target/output"),
                clone.path.join("target/linked"),
            )
            .unwrap(),
            "symlink" => {
                std::os::unix::fs::symlink(scratch.path(), clone.path.join("target/linked"))
                    .unwrap()
            }
            "source-state" => std::fs::create_dir(clone.path.join("target/.git")).unwrap(),
            "nonignored" => std::fs::write(clone.path.join(".git/info/exclude"), "").unwrap(),
            "tracked" => {
                common::git(&clone.path, &["add", "--force", "target/output"]);
            }
            _ => (),
        }
        let preview = preview_target_prune(
            &database,
            &Git::from_path(),
            clone.id,
            &PrunePolicy::default(),
            &evidence,
        )
        .await
        .unwrap();
        assert!(preview.kept_reason.is_some(), "{case}: {preview:?}");
        let write = database.write().await.unwrap();
        assert!(
            prepare_target_prune(&write, database.epoch(), &preview, &evidence)
                .await
                .is_err(),
            "{case}"
        );
        require_execution_start_allowed(&write).await.unwrap();
        write.rollback().await.unwrap();
        if held {
            let write = database.write().await.unwrap();
            cockpit_projects::release(&write, clone.id, &holder("live-holder"))
                .await
                .unwrap();
            write.commit().await.unwrap();
        }
        if matches!(case, "hardlink" | "symlink") {
            std::fs::remove_file(clone.path.join("target/linked")).unwrap();
        }
        if case == "source-state" {
            std::fs::remove_dir(clone.path.join("target/.git")).unwrap();
        }
        if case == "nonignored" {
            std::fs::write(clone.path.join(".git/info/exclude"), "/target/\n").unwrap();
        }
    }
    assert_eq!(
        std::fs::read(clone.path.join("target/output")).unwrap(),
        b"12345678"
    );
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn prune_guard_survives_restart_and_blocks_claim_and_whole_clone_sweep() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (project, _) = register(&database, &scratch.repository("app")).await;
    let clone = create_parallel_clone(
        &database,
        &Git::from_path(),
        project.name.as_str(),
        "feature",
    )
    .await
    .unwrap();
    std::fs::create_dir(clone.path.join("target")).unwrap();
    std::fs::write(clone.path.join("target/output"), b"12345678").unwrap();
    std::fs::write(clone.path.join(".git/info/exclude"), "/target/\n").unwrap();
    let evidence = PruneEvidence {
        ended_sessions: Some("fixture-ended-session-revision-1".to_owned()),
        busy: None,
    };
    let preview = preview_target_prune(
        &database,
        &Git::from_path(),
        clone.id,
        &PrunePolicy::default(),
        &evidence,
    )
    .await
    .unwrap();
    assert!(preview.kept_reason.is_none(), "{preview:?}");
    let write = database.write().await.unwrap();
    let plan = prepare_target_prune(&write, database.epoch(), &preview, &evidence)
        .await
        .unwrap();
    reserve_target_prune(
        &write,
        database.epoch(),
        plan.operation,
        plan.revision,
        &preview,
        &evidence,
        false,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    drop(database);
    let database = scratch.open().await;
    let write = database.write().await.unwrap();
    assert!(require_execution_start_allowed(&write).await.is_err());
    assert!(
        claim(&write, clone.id, &holder("later-session"))
            .await
            .is_err(),
        "a reservation must exclude a new claimant"
    );
    write.rollback().await.unwrap();
    let report = sweep(&database, &Git::from_path(), false).await.unwrap();
    assert!(report.removed.is_empty(), "{report:?}");
    assert!(clone.path.join("target/output").is_file());
    assert!(clone.path.join("README").is_file());
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn damaged_cleanup_cursor_or_byte_receipt_cannot_acquire_a_guard() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (project, _) = register(&database, &scratch.repository("app")).await;
    let clone = create_parallel_clone(
        &database,
        &Git::from_path(),
        project.name.as_str(),
        "feature",
    )
    .await
    .unwrap();
    std::fs::create_dir(clone.path.join("target")).unwrap();
    std::fs::write(clone.path.join("target/output"), b"12345678").unwrap();
    std::fs::write(clone.path.join(".git/info/exclude"), "/target/\n").unwrap();
    let evidence = PruneEvidence {
        ended_sessions: Some("fixture-ended-session-revision-1".into()),
        busy: None,
    };
    let preview = preview_target_prune(
        &database,
        &Git::from_path(),
        clone.id,
        &PrunePolicy::default(),
        &evidence,
    )
    .await
    .unwrap();
    for damage in [
        "cursor = 100001",
        "removed_bytes = 9",
        "absent_bytes = 9",
        "apparent_bytes = 9",
        "target_identity = 'd:replacement'",
    ] {
        let write = database.write().await.unwrap();
        let plan = prepare_target_prune(&write, database.epoch(), &preview, &evidence)
            .await
            .unwrap();
        // Deliberately damage this fixture's projects-owned record, as recovery can see
        // an inconsistent snapshot. The regular consume API must reject it before guards.
        write
            .execute(
                &format!("UPDATE project_target_prunes SET {damage} WHERE id = ?1"),
                [plan.operation],
            )
            .await
            .unwrap();
        assert!(
            reserve_target_prune(
                &write,
                database.epoch(),
                plan.operation,
                plan.revision,
                &preview,
                &evidence,
                false
            )
            .await
            .is_err(),
            "accepted {damage}"
        );
        require_execution_start_allowed(&write).await.unwrap();
        write.rollback().await.unwrap();
    }
    assert_eq!(
        std::fs::read(clone.path.join("target/output")).unwrap(),
        b"12345678"
    );

    for damage in [
        "phase='finishing'",
        "phase='deleting',cursor=1",
        "phase='paused',worker_active=1,resume_phase='reserved'",
    ] {
        let write = database.write().await.unwrap();
        let plan = prepare_target_prune(&write, database.epoch(), &preview, &evidence)
            .await
            .unwrap();
        reserve_target_prune(
            &write,
            database.epoch(),
            plan.operation,
            plan.revision,
            &preview,
            &evidence,
            false,
        )
        .await
        .unwrap();
        write
            .execute(
                &format!("UPDATE project_target_prunes SET {damage} WHERE id = ?1"),
                [plan.operation],
            )
            .await
            .unwrap();
        assert!(
            cockpit_projects::begin_prune_step(
                &write,
                database.epoch(),
                plan.operation,
                &PrunePolicy::default(),
                &evidence
            )
            .await
            .is_err(),
            "accepted {damage}"
        );
        write.rollback().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(target_os = "linux")]
async fn cancelled_cleanup_future_keeps_database_exclusive_until_blocking_step_returns() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let scratch = Scratch::new();
    let database = Arc::new(scratch.open().await);
    let (project, _) = register(&database, &scratch.repository("app")).await;
    let clone = create_parallel_clone(
        &database,
        &Git::from_path(),
        project.name.as_str(),
        "feature",
    )
    .await
    .unwrap();
    std::fs::create_dir(clone.path.join("target")).unwrap();
    std::fs::write(clone.path.join("target/output"), b"123").unwrap();
    std::fs::write(clone.path.join(".git/info/exclude"), "/target/\n").unwrap();
    let evidence = PruneEvidence {
        ended_sessions: Some("fixture-ended-session-revision-1".into()),
        busy: None,
    };
    let policy = PrunePolicy::default();
    let preview = preview_target_prune(&database, &Git::from_path(), clone.id, &policy, &evidence)
        .await
        .unwrap();
    let (operation, step) = {
        let write = database.write().await.unwrap();
        let plan = prepare_target_prune(&write, database.epoch(), &preview, &evidence)
            .await
            .unwrap();
        reserve_target_prune(
            &write,
            database.epoch(),
            plan.operation,
            plan.revision,
            &preview,
            &evidence,
            false,
        )
        .await
        .unwrap();
        let step = cockpit_projects::begin_prune_step(
            &write,
            database.epoch(),
            plan.operation,
            &policy,
            &evidence,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        (plan.operation, step)
    };
    let wrapper = scratch.path().join("git-latch");
    let entered = scratch.path().join("entered");
    let resume = scratch.path().join("resume");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
    std::fs::write(&wrapper, format!("#!/bin/sh\nexport HOME={} GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1\n: > {}\nn=0\nwhile [ ! -f {} ]; do n=$((n+1)); [ \"$n\" -lt 1000 ] || exit 125; /usr/bin/sleep 0.01; done\nexec /usr/bin/git \"$@\"\n",
        quote(&scratch.path().join("git-home")), quote(&entered), quote(&resume))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let running = tokio::spawn(step.run(Arc::clone(&database), Git::with_program(wrapper)));
    drop(database);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !entered.exists() {
        assert!(
            Instant::now() < deadline,
            "cleanup never entered its blocking step"
        );
        tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(10)))
            .await
            .unwrap();
    }
    running.abort();
    match running.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("cleanup future completed while its blocking step was held"),
    }
    assert!(
        cockpit_turso::Database::open(
            &scratch.path().join("state.db"),
            &[cockpit_projects::MIGRATIONS]
        )
        .await
        .is_err(),
        "another daemon could recover a still-running filesystem step"
    );
    std::fs::write(&resume, "").unwrap();
    let reopened = loop {
        match cockpit_turso::Database::open(
            &scratch.path().join("state.db"),
            &[cockpit_projects::MIGRATIONS],
        )
        .await
        {
            Ok(database) => break database,
            Err(_) if Instant::now() < deadline => {
                tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(10)))
                    .await
                    .unwrap();
            }
            Err(error) => panic!("database stayed owned after step returned: {error}"),
        }
    };
    let write = reopened.write().await.unwrap();
    cockpit_projects::recover_target_prunes(&write)
        .await
        .unwrap();
    let status = cockpit_projects::prune_status(&write, operation)
        .await
        .unwrap();
    assert_eq!(status.phase, "paused");
    require_execution_start_allowed(&write).await.unwrap();
    assert!(
        cockpit_projects::require_checkout_available(&write, clone.id)
            .await
            .is_err()
    );
    write.commit().await.unwrap();
    assert_eq!(
        std::fs::read(clone.path.join("target/output")).unwrap(),
        b"123"
    );
}
