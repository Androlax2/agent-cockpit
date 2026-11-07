//! Owner recovery never interprets abandonment as permission to touch retained files.
use super::*;

fn configured_home() -> Home {
    let home = Home::new();
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
    )
    .unwrap();
    home
}

fn abandon(status: &cockpit_protocol::StoragePruneStatusView) -> Command {
    Command::StoragePruneAbandon {
        operation: status.operation,
        revision: status.revision,
        epoch: status.database_epoch.clone(),
    }
}

async fn status(client: &Connection, operation: i64) -> cockpit_protocol::StoragePruneStatusView {
    let Reply::StoragePruneStatus { status } =
        request(client, Command::StoragePruneStatus { operation })
            .await
            .unwrap()
    else {
        panic!()
    };
    *status
}

async fn reserve(home: &Home, branch: &str) -> (i64, cockpit_projects::PrunePreview) {
    let (plan, preview) = prepared_cleanup_in(home, branch).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    cockpit_projects::reserve_target_prune(
        &write,
        database.epoch(),
        plan.operation,
        plan.revision,
        &preview,
        &cockpit_projects::PruneEvidence::default(),
        false,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    (plan.operation, preview)
}

// Only this test's isolated, controlled clone tree; includes empty directories and Git files.
fn contents(root: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(
        root: &Path,
        path: &Path,
        result: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>,
    ) {
        if path.is_dir() {
            result.insert(path.strip_prefix(root).unwrap().to_owned(), None);
            for child in std::fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), result);
            }
        } else {
            result.insert(
                path.strip_prefix(root).unwrap().to_owned(),
                Some(std::fs::read(path).unwrap()),
            );
        }
    }
    let mut result = std::collections::BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_abandon_preserves_files_other_guards_and_terminal_history() {
    let home = configured_home();
    let daemon = home.start(BASELINE, settings()).await;
    let owner = connect(&daemon).await;
    home.project(&owner).await;
    let (first, preview) = reserve(&home, "first").await;
    cleanup_step(&home, first, true).await; // create quarantine
    cleanup_step(&home, first, true).await; // detach target
    let database = home.database.lock().unwrap().clone().unwrap();
    {
        let write = database.write().await.unwrap();
        cockpit_projects::pause_target_prune(&write, first, "fixture safe checkpoint")
            .await
            .unwrap();
        write.commit().await.unwrap();
    }
    let (second, other) = reserve(&home, "second").await;
    let before = status(&owner, first).await;
    let first_files = contents(&preview.checkout.path);
    let other_files = contents(&other.checkout.path);
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    assert_eq!(
        refusal(request(&agent, abandon(&before)).await).code,
        ErrorCode::NotAuthorized
    );
    for (revision, epoch) in [
        (before.revision + 1, before.database_epoch.clone()),
        (before.revision, "stale-epoch".into()),
    ] {
        assert_eq!(
            refusal(
                request(
                    &owner,
                    Command::StoragePruneAbandon {
                        operation: first,
                        revision,
                        epoch
                    }
                )
                .await
            )
            .code,
            ErrorCode::StaleRevision
        );
        assert_eq!(status(&owner, first).await, before);
    }
    let Reply::StoragePruneStatus { status: abandoned } =
        request(&owner, abandon(&before)).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(abandoned.phase, "abandoned");
    assert_eq!(abandoned.target, before.target);
    assert_eq!(abandoned.quarantine, before.quarantine);
    assert_eq!(
        abandoned.removed_apparent_bytes,
        before.removed_apparent_bytes
    );
    assert!(abandoned.reason.as_deref().unwrap().contains("retained"));
    assert_eq!(contents(&preview.checkout.path), first_files);
    assert_eq!(contents(&other.checkout.path), other_files);
    {
        let read = database.read().await.unwrap();
        cockpit_projects::require_checkout_available(&read, preview.checkout.id)
            .await
            .unwrap();
        assert!(
            cockpit_projects::require_checkout_available(&read, other.checkout.id)
                .await
                .is_err()
        );
        assert!(
            cockpit_projects::require_execution_start_allowed(&read)
                .await
                .unwrap_err()
                .to_string()
                .contains(&second.to_string())
        );
    }
    let replay = request(&owner, abandon(&before)).await.unwrap();
    assert_eq!(
        replay,
        Reply::StoragePruneStatus {
            status: abandoned.clone()
        }
    );
    let replay = request(
        &owner,
        Command::StoragePruneApply {
            plan: first,
            revision: before.revision,
            epoch: before.epoch.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        replay,
        Reply::StoragePruneStatus {
            status: abandoned.clone()
        }
    );
    drop(agent);
    drop(owner);
    drop(database);
    home.stop(daemon);
    let daemon = home.start(BASELINE, settings()).await;
    let owner = connect(&daemon).await;
    assert_eq!(status(&owner, first).await, *abandoned);
    assert_eq!(status(&owner, second).await.phase, "paused");
    assert_eq!(contents(&preview.checkout.path), first_files);
    assert_eq!(contents(&other.checkout.path), other_files);
    // Explicit fixture creation, never automatic restoration of the retained old target.
    std::fs::create_dir(preview.checkout.path.join("target")).unwrap();
    std::fs::write(preview.checkout.path.join("target/new"), b"new").unwrap();
    let Reply::StoragePrunePreview { preview: fresh } = request(
        &owner,
        Command::StoragePrunePreview {
            checkout: preview.checkout.id.0,
            prepare: true,
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_ne!(fresh.plan.unwrap().operation, first);
    let started = new_session(&owner, &preview.checkout.path, None).await;
    assert_eq!(started.state, SessionStateView::Running);
    let refused = refusal(
        request(
            &owner,
            Command::SessionNew {
                dir: other.checkout.path.display().to_string(),
                agent: Some("fake".into()),
                prompt: None,
                clone: None,
                model: None,
                effort: None,
                compaction_threshold: None,
            },
        )
        .await,
    );
    assert!(refused.message.contains(&second.to_string()), "{refused:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_abandon_after_restart_fences_old_receipt_and_next_step() {
    for complete_step in [false, true] {
        let home = configured_home();
        let daemon = home.start(BASELINE, settings()).await;
        let owner = connect(&daemon).await;
        home.project(&owner).await;
        let (operation, preview) = reserve(&home, "cleanup").await;
        let database = home.database.lock().unwrap().clone().unwrap();
        let policy = cockpit_projects::PrunePolicy {
            base_ref: Some("refs/heads/main".into()),
            ..Default::default()
        };
        let step = {
            let write = database.write().await.unwrap();
            let step = cockpit_projects::begin_prune_step(
                &write,
                database.epoch(),
                operation,
                &policy,
                &Default::default(),
            )
            .await
            .unwrap();
            write.commit().await.unwrap();
            step
        };
        let (receipt, delayed) = if complete_step {
            (
                Some(
                    step.run(Arc::clone(&database), cockpit_projects::Git::from_path())
                        .await
                        .unwrap(),
                ),
                None,
            )
        } else {
            (None, Some(step))
        };
        let before = status(&owner, operation).await;
        let error = refusal(request(&owner, abandon(&before)).await);
        assert_eq!(error.code, ErrorCode::Refused);
        assert!(error.message.contains("active"), "{error:?}");
        let files = contents(&preview.checkout.path);
        drop(owner);
        drop(database);
        home.stop(daemon);
        let daemon = home.start(BASELINE, settings()).await;
        let owner = connect(&daemon).await;
        let paused = status(&owner, operation).await;
        assert_eq!(paused.phase, "paused");
        request(&owner, abandon(&paused)).await.unwrap();
        let abandoned = status(&owner, operation).await;
        assert_eq!(abandoned.phase, "abandoned");
        let database = home.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        if let Some(receipt) = receipt {
            assert!(
                cockpit_projects::complete_prune_step(&write, database.epoch(), receipt)
                    .await
                    .is_err()
            );
        }
        assert!(
            cockpit_projects::begin_prune_step(
                &write,
                database.epoch(),
                operation,
                &policy,
                &Default::default()
            )
            .await
            .is_err()
        );
        cockpit_projects::recover_target_prunes(&write)
            .await
            .unwrap();
        write.commit().await.unwrap();
        assert_eq!(status(&owner, operation).await, abandoned);
        if let Some(delayed) = delayed {
            assert!(
                delayed
                    .run(Arc::clone(&database), cockpit_projects::Git::from_path())
                    .await
                    .is_err(),
                "old reserved callback performed filesystem work after abandonment"
            );
        }
        assert_eq!(contents(&preview.checkout.path), files);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_abandon_restored_plan_requires_current_displayed_database_epoch() {
    let home = configured_home();
    let daemon = home.start(BASELINE, settings()).await;
    let owner = connect(&daemon).await;
    home.project(&owner).await;
    let (operation, preview) = reserve(&home, "cleanup").await;
    let old = status(&owner, operation).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let backup = home.path().join("reserved-backup.db");
    database.backup_to(&backup).await.unwrap();
    let files = contents(&preview.checkout.path);
    drop(owner);
    drop(database);
    home.stop(daemon);
    let deadline = Instant::now() + GENEROUS;
    loop {
        match cockpit_turso::restore(&backup, &home.path().join("state/cockpit/state.db")).await {
            Ok(()) => break,
            Err(cockpit_turso::Error::AlreadyOpen { .. }) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => panic!("{error}"),
        }
    }
    let daemon = home.start(BASELINE, settings()).await;
    let owner = connect(&daemon).await;
    let current = status(&owner, operation).await;
    assert_eq!(old.epoch, current.epoch);
    assert_ne!(old.database_epoch, current.database_epoch);
    assert_eq!(
        refusal(request(&owner, abandon(&old)).await).code,
        ErrorCode::StaleRevision
    );
    assert_eq!(status(&owner, operation).await, current);
    request(&owner, abandon(&current)).await.unwrap();
    assert_eq!(status(&owner, operation).await.phase, "abandoned");
    assert_eq!(contents(&preview.checkout.path), files);
    let database = home.database.lock().unwrap().clone().unwrap();
    let read = database.read().await.unwrap();
    cockpit_projects::require_execution_start_allowed(&read)
        .await
        .unwrap();
    cockpit_projects::require_checkout_available(&read, preview.checkout.id)
        .await
        .unwrap();
}
