//! Real session routes must observe durable cleanup exclusion before launch preparation.
use super::*;
#[path = "target_prune/abandon.rs"]
mod abandon;

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_changed_manifest_local_oid_policy_and_current_busy_evidence_refuse_apply() {
    for change in [
        "manifest",
        "local-base",
        "local-tip",
        "policy",
        "queue",
        "foreground",
    ] {
        let home = Home::new();
        let config = home.path().join("home/.config/torlek");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
        )
        .unwrap();
        let mut daemon = home.start(BASELINE, settings()).await;
        let mut client = connect(&daemon).await;
        let root = home.project(&client).await;
        let (plan, preview) = prepared_cleanup(&home).await;
        match change {
            "manifest" => {
                std::fs::write(preview.checkout.path.join("target/new"), b"keep").unwrap()
            }
            "local-base" => {
                git(
                    &root,
                    &["commit", "--quiet", "--allow-empty", "-m", "changed-base"],
                );
            }
            "local-tip" => {
                git(
                    &preview.checkout.path,
                    &["commit", "--quiet", "--allow-empty", "-m", "changed-tip"],
                );
            }
            "policy" => {
                drop(client);
                home.stop(daemon);
                std::fs::write(config.join("config.toml"), "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\ntarget_prune_confirm_above_mib = 2\n").unwrap();
                daemon = home.start(BASELINE, settings()).await;
                client = connect(&daemon).await;
            }
            "queue" => {
                request(&client, Command::ResourceQueue { paused: true })
                    .await
                    .unwrap();
            }
            "foreground" => {
                request(
                    &client,
                    Command::ResourceProfile {
                        profile: cockpit_protocol::ProfileView::Foreground,
                    },
                )
                .await
                .unwrap();
            }
            _ => unreachable!(),
        }
        let error = refusal(
            request(
                &client,
                Command::StoragePruneApply {
                    plan: plan.operation,
                    revision: plan.revision,
                    epoch: plan.epoch,
                },
            )
            .await,
        );
        assert!(
            matches!(error.code, ErrorCode::Refused | ErrorCode::StaleRevision),
            "{change}: {error:?}"
        );
        assert_eq!(
            std::fs::read(preview.checkout.path.join("target/output")).unwrap(),
            b"fixture"
        );
        let database = home.database.lock().unwrap().clone().unwrap();
        let read = database.read().await.unwrap();
        cockpit_projects::require_execution_start_allowed(&read)
            .await
            .unwrap();
        assert_eq!(
            cockpit_projects::prune_status(&read, plan.operation)
                .await
                .unwrap()
                .phase,
            "prepared"
        );
        drop(daemon);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_active_bulk_step_keeps_control_responsive_and_refuses_duplicate_writer() {
    use std::os::unix::fs::PermissionsExt;
    for stop_daemon in [false, true] {
        let home = Home::new();
        let config = home.path().join("home/.config/torlek");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
        )
        .unwrap();
        let wrapper = home.path().join("git-latch");
        let previewed = home.path().join("previewed");
        let entered = home.path().join("entered");
        let resume = home.path().join("resume");
        let quote = |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
        std::fs::write(&wrapper, format!("#!/bin/sh\nexport HOME={} GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1\ncase \"$*\" in\n*ls-files*)\n if [ -f {} ]; then\n  : > {}\n  n=0\n  while [ ! -f {} ]; do n=$((n+1)); [ \"$n\" -lt 1000 ] || exit 125; /usr/bin/sleep 0.01; done\n else : > {}; fi;;\nesac\nexec /usr/bin/git \"$@\"\n",
        quote(&home.path().join("isolated-git-home")), quote(&previewed), quote(&entered), quote(&resume), quote(&previewed))).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let daemon = home
            .start_wired(|| {
                let mut wiring = home.wiring(BASELINE, settings());
                wiring.git = cockpit_projects::Git::with_program(wrapper.clone());
                wiring
            })
            .await;
        let client = connect(&daemon).await;
        let root = home.project(&client).await;
        let (plan, preview) = prepared_cleanup(&home).await;
        let command = Command::StoragePruneApply {
            plan: plan.operation,
            revision: plan.revision,
            epoch: plan.epoch,
        };
        let duplicate = command.clone();
        let pending = tokio::spawn(async move { request(&client, command).await });
        let deadline = Instant::now() + GENEROUS;
        while !entered.exists() {
            assert!(
                Instant::now() < deadline,
                "cleanup never entered its reserved blocking step"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let control = connect(&daemon).await;
        let reply = tokio::time::timeout(
            Duration::from_secs(2),
            request(
                &control,
                Command::StoragePruneStatus {
                    operation: plan.operation,
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let Reply::StoragePruneStatus { status } = reply else {
            panic!("{reply:?}")
        };
        assert_eq!(status.phase, "reserved");
        assert_eq!(status.removed_apparent_bytes, 0);
        let error = refusal(
            request(
                &control,
                Command::StoragePruneAbandon {
                    operation: status.operation,
                    revision: status.revision,
                    epoch: status.database_epoch.clone(),
                },
            )
            .await,
        );
        assert_eq!(error.code, ErrorCode::Refused);
        assert!(error.message.contains("active"), "{error:?}");
        let duplicate = request(&control, duplicate).await.unwrap();
        let Reply::StoragePruneStatus { status } = duplicate else {
            panic!("{duplicate:?}")
        };
        assert_eq!(status.phase, "reserved");
        assert!(
            refusal(
                request(
                    &control,
                    Command::SessionNew {
                        dir: root.display().to_string(),
                        agent: Some("fake".into()),
                        prompt: None,
                        clone: None
                    }
                )
                .await
            )
            .message
            .contains(&plan.operation.to_string())
        );
        assert_eq!(
            std::fs::read(preview.checkout.path.join("target/output")).unwrap(),
            b"fixture"
        );
        if stop_daemon {
            let database = home.database.lock().unwrap().clone().unwrap();
            home.stop(daemon);
            std::fs::write(&resume, "").unwrap();
            let _reply = pending.await.unwrap();
            let deadline = Instant::now() + GENEROUS;
            loop {
                let read = database.read().await.unwrap();
                let status = cockpit_projects::prune_status(&read, plan.operation)
                    .await
                    .unwrap();
                if status.phase == "paused" {
                    assert!(status.reason.unwrap().contains("stopping"));
                    cockpit_projects::require_execution_start_allowed(&read)
                        .await
                        .unwrap();
                    assert!(
                        cockpit_projects::require_checkout_available(&read, preview.checkout.id)
                            .await
                            .is_err()
                    );
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "shutdown did not pause after the joined slice: {status:?}"
                );
                drop(read);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                std::fs::read(preview.checkout.path.join("target/output")).unwrap(),
                b"fixture"
            );
        } else {
            std::fs::write(&resume, "").unwrap();
            let Reply::StoragePruneStatus { status } = pending.await.unwrap().unwrap() else {
                panic!()
            };
            assert_eq!(status.phase, "complete");
            assert_eq!(status.removed_apparent_bytes, 7);
        }
        assert!(home.logged_launches().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_restored_database_refuses_old_intent_for_reused_plan_number() {
    let home = Home::new();
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
    )
    .unwrap();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    home.project(&client).await;
    let (_, preview) = prepared_cleanup(&home).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let backup = home.path().join("before-plan.db");
    let write = database.write().await.unwrap();
    write
        .execute(
            "DELETE FROM project_target_prunes WHERE phase='prepared'",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    database.backup_to(&backup).await.unwrap();
    let prepare = || Command::StoragePrunePreview {
        checkout: preview.checkout.id.0,
        prepare: true,
    };
    let Reply::StoragePrunePreview { preview: old } = request(&client, prepare()).await.unwrap()
    else {
        panic!()
    };
    let old = old.plan.unwrap();
    drop(database);
    drop(client);
    home.stop(daemon);
    let deadline = Instant::now() + GENEROUS;
    loop {
        match cockpit_turso::restore(&backup, &home.path().join("state/cockpit/state.db")).await {
            Ok(()) => break,
            Err(cockpit_turso::Error::AlreadyOpen { .. }) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => panic!("restore failed: {error}"),
        }
    }
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let Reply::StoragePrunePreview { preview: current } =
        request(&client, prepare()).await.unwrap()
    else {
        panic!()
    };
    let current = current.plan.unwrap();
    assert_eq!(
        (old.operation, old.revision),
        (current.operation, current.revision)
    );
    assert_ne!(old.epoch, current.epoch);
    let refused = refusal(
        request(
            &client,
            Command::StoragePruneApply {
                plan: old.operation,
                revision: old.revision,
                epoch: old.epoch,
            },
        )
        .await,
    );
    assert_eq!(refused.code, ErrorCode::StaleRevision);
    assert_eq!(
        std::fs::read(preview.checkout.path.join("target/output")).unwrap(),
        b"fixture"
    );
    let Reply::StoragePruneStatus { status } = request(
        &client,
        Command::StoragePruneStatus {
            operation: current.operation,
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(status.phase, "prepared");
    assert_eq!(status.epoch, current.epoch);
}

async fn prepared_cleanup(
    home: &Home,
) -> (
    cockpit_projects::PruneStatus,
    cockpit_projects::PrunePreview,
) {
    prepared_cleanup_in(home, "cleanup").await
}

async fn prepared_cleanup_in(
    home: &Home,
    branch: &str,
) -> (
    cockpit_projects::PruneStatus,
    cockpit_projects::PrunePreview,
) {
    use cockpit_projects::{
        Git, PruneEvidence, PrunePolicy, create_parallel_clone, prepare_target_prune,
        preview_target_prune,
    };
    let database = home.database.lock().unwrap().clone().unwrap();
    let clone = create_parallel_clone(&database, &Git::from_path(), "app", branch)
        .await
        .unwrap();
    std::fs::create_dir(clone.path.join("target")).unwrap();
    std::fs::write(clone.path.join("target/output"), b"fixture").unwrap();
    std::fs::write(clone.path.join(".git/info/exclude"), "/target/\n").unwrap();
    let policy = PrunePolicy {
        base_ref: Some("refs/heads/main".into()),
        ..PrunePolicy::default()
    };
    let evidence = PruneEvidence::default();
    let preview = preview_target_prune(&database, &Git::from_path(), clone.id, &policy, &evidence)
        .await
        .unwrap();
    let write = database.write().await.unwrap();
    let plan = prepare_target_prune(&write, database.epoch(), &preview, &evidence)
        .await
        .unwrap();
    write.commit().await.unwrap();
    (plan, preview)
}

async fn reserve_cleanup(home: &Home) -> (i64, PathBuf) {
    let (plan, preview) = prepared_cleanup(home).await;
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
    (plan.operation, preview.checkout.path)
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_guard_refuses_unrelated_session_start_before_any_launch_intent() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let (operation, clone) = reserve_cleanup(&home).await;
    let result = request(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".into()),
            prompt: None,
            clone: None,
        },
    )
    .await;
    let error = refusal(result);
    assert!(error.message.contains(&operation.to_string()), "{error:?}");
    assert!(sessions(&client, true).await.is_empty());
    assert!(home.logged_launches().is_empty());
    assert_eq!(
        std::fs::read(clone.join("target/output")).unwrap(),
        b"fixture"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_guard_refuses_check_after_its_source_reads() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    std::fs::create_dir(root.join(".torlek")).unwrap();
    std::fs::write(
        root.join(".torlek/config.toml"),
        "[[checks]]\nname = 'fixture'\ncommand = 'echo should-not-run'\n",
    )
    .unwrap();
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!("[projects]\ntrusted = [{:?}]\n", root.to_str().unwrap()),
    )
    .unwrap();
    let (operation, _) = reserve_cleanup(&home).await;
    let result = request(
        &client,
        Command::CheckRun {
            project: "app".into(),
            check: "fixture".into(),
            checkout: None,
        },
    )
    .await;
    let error = refusal(result);
    assert!(error.message.contains(&operation.to_string()), "{error:?}");
    let database = home.database.lock().unwrap().clone().unwrap();
    assert!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_guard_refuses_owner_approval_without_consuming_intent() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    // Seed a legitimate pending request using the capability API, without launching an
    // agent. The production owner socket is the entrypoint under test.
    let run = pending_run(&database, root).await;
    let (operation, clone) = reserve_cleanup(&home).await;
    let result = request(
        &client,
        Command::ApprovalApprove {
            request: run.id.0,
            revision: run.revision,
            digest: run.digest(),
        },
    )
    .await;
    let error = refusal(result);
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains(&operation.to_string()), "{error:?}");
    let read = database.read().await.unwrap();
    let current = cockpit_approvals::shown_run(&read, run.id, unix_now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.state, cockpit_approvals::RunState::Asking);
    assert_eq!(current.revision, run.revision);
    assert_eq!(current.digest(), run.digest());
    assert!(
        cockpit_processes::occupying_executions_in(&read)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read(clone.join("target/output")).unwrap(),
        b"fixture"
    );
}

async fn pending_run(
    database: &cockpit_turso::Database,
    root: PathBuf,
) -> cockpit_approvals::RunRequest {
    use cockpit_authorization::{Permission, Scope, SessionId};
    let write = database.write().await.unwrap();
    let session = SessionId::new("prune-fixture").unwrap();
    cockpit_authorization::grant(
        &write,
        &session,
        Permission::RunCommands,
        &Scope::Session(session.clone()),
    )
    .await
    .unwrap();
    let request = cockpit_approvals::ask_run(
        &write,
        &session,
        cockpit_approvals::RunAsk {
            command: "printf should-not-run".into(),
            cwd: root,
            reason: "fixture".into(),
            project: None,
        },
        unix_now() - cockpit_approvals::APPROVAL_GRACE_SECONDS - 1,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    request
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_prepare_refuses_committed_approval_before_runner_reserves() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let (plan, preview) = prepared_cleanup(&home).await;
    let run = pending_run(&database, root).await;
    // Stop precisely at the real capability's committed approved-intent boundary.
    // No Runner dispatch: there is deliberately no occupying process to discover.
    {
        let write = database.write().await.unwrap();
        cockpit_approvals::approve_run(&write, run.id, run.revision, &run.digest(), unix_now())
            .await
            .unwrap();
        write.commit().await.unwrap();
    }
    assert!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap()
            .is_empty()
    );
    let result = request(
        &client,
        Command::StoragePrunePreview {
            checkout: preview.checkout.id.0,
            prepare: true,
        },
    )
    .await;
    let error = refusal(result);
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(
        error
            .message
            .contains(&format!("run request {} is approved", run.id.0)),
        "{error:?}"
    );
    let read = database.read().await.unwrap();
    cockpit_projects::require_execution_start_allowed(&read)
        .await
        .unwrap();
    assert_eq!(
        cockpit_projects::prune_status(&read, plan.operation)
            .await
            .unwrap()
            .phase,
        "prepared"
    );
    assert_eq!(
        std::fs::read(preview.checkout.path.join("target/output")).unwrap(),
        b"fixture"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_guard_refuses_wake_and_resume_without_new_launch_or_revision() {
    for ended in [false, true] {
        let home = Home::new();
        let daemon = home.start(BASELINE, settings()).await;
        let client = connect(&daemon).await;
        let root = home.project(&client).await;
        let started = new_session(&client, &root, None).await;
        home.launches(1).await;
        let stopped = session_reply(
            &client,
            if ended {
                Command::SessionEnd {
                    session: started.id.clone(),
                }
            } else {
                Command::SessionSleep {
                    session: started.id.clone(),
                }
            },
        )
        .await;
        let (operation, _) = reserve_cleanup(&home).await;
        let result = request(
            &client,
            if ended {
                Command::SessionResume {
                    session: started.id.clone(),
                }
            } else {
                Command::SessionWake {
                    session: started.id.clone(),
                }
            },
        )
        .await;
        let error = refusal(result);
        assert!(error.message.contains(&operation.to_string()), "{error:?}");
        let current = sessions(&client, true)
            .await
            .into_iter()
            .find(|s| s.id == started.id)
            .unwrap();
        assert_eq!(current.state, stopped.state);
        assert_eq!(current.revision, stopped.revision);
        assert_eq!(home.still_logged(1).await.len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_recovery_releases_global_guard_but_keeps_candidate_excluded() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;
    // Deliberately seed a recovery conflict through the domain API. Production cleanup
    // evidence refuses this running session; startup must also fail closed on the guard.
    let (operation, clone) = reserve_cleanup(&home).await;
    drop(client);
    home.stop(daemon);
    home.lose_every_agent();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let current = sessions(&client, true)
        .await
        .into_iter()
        .find(|s| s.id == started.id)
        .unwrap();
    // Startup has exclusive database ownership, hence no prior blocking step can remain.
    // It releases only the global guard before restoring unrelated sessions.
    assert_eq!(current.state, SessionStateView::Running);
    assert_eq!(home.still_logged(2).await.len(), 2);
    let error = refusal(
        request(
            &client,
            Command::SessionNew {
                dir: clone.display().to_string(),
                agent: Some("fake".into()),
                prompt: None,
                clone: None,
            },
        )
        .await,
    );
    assert!(error.message.contains(&operation.to_string()), "{error:?}");
    let result = request(&client, Command::StoragePruneStatus { operation })
        .await
        .unwrap();
    let Reply::StoragePruneStatus { status } = result else {
        panic!("{result:?}")
    };
    assert_eq!(status.phase, "paused");
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_confirmed_owner_plan_removes_only_its_target_and_is_idempotent() {
    let home = Home::new();
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
    )
    .unwrap();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    home.project(&client).await;
    let (plan, preview) = prepared_cleanup(&home).await;
    let apply = || Command::StoragePruneApply {
        plan: plan.operation,
        revision: plan.revision,
        epoch: plan.epoch.clone(),
    };
    let result = request(&client, apply()).await;
    assert!(
        matches!(result, Ok(Reply::StoragePruneStatus { .. })),
        "{result:?}"
    );
    let deadline = Instant::now() + GENEROUS;
    loop {
        let result = request(
            &client,
            Command::StoragePruneStatus {
                operation: plan.operation,
            },
        )
        .await
        .unwrap();
        let Reply::StoragePruneStatus { status } = result else {
            panic!("{result:?}")
        };
        if status.phase == "complete" {
            assert_eq!(status.apparent_bytes, 7);
            assert_eq!(status.removed_apparent_bytes, 7);
            assert_eq!(status.absent_unconfirmed_bytes, 0);
            break;
        }
        assert!(Instant::now() < deadline, "{status:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!preview.checkout.path.join("target").exists());
    assert!(preview.checkout.path.join(".git").is_dir());
    assert_eq!(
        std::fs::read(preview.checkout.path.join("src/README")).unwrap(),
        b"fixture\n"
    );
    std::fs::create_dir(preview.checkout.path.join("target")).unwrap();
    std::fs::write(preview.checkout.path.join("target/replacement"), b"keep").unwrap();
    assert!(matches!(
        request(&client, apply()).await,
        Ok(Reply::StoragePruneStatus { .. })
    ));
    assert_eq!(
        std::fs::read(preview.checkout.path.join("target/replacement")).unwrap(),
        b"keep"
    );
    let database = home.database.lock().unwrap().clone().unwrap();
    let read = database.read().await.unwrap();
    cockpit_projects::require_execution_start_allowed(&read)
        .await
        .unwrap();
    cockpit_projects::require_checkout_available(&read, preview.checkout.id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn paused_target_prune_keeps_candidate_checks_and_approvals_out_but_allows_unrelated_work() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let (operation, clone) = reserve_cleanup(&home).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    // Seed the safe-paused boundary: no worker exists, the global guard is released,
    // but this candidate's durable guard must still protect every production start route.
    {
        let write = database.write().await.unwrap();
        write
            .execute(
                "DELETE FROM project_target_prune_guards WHERE key = 'global'",
                (),
            )
            .await
            .unwrap();
        write.commit().await.unwrap();
    }
    std::fs::create_dir(clone.join(".torlek")).unwrap();
    std::fs::write(
        clone.join(".torlek/config.toml"),
        "[[checks]]\nname = 'fixture'\ncommand = 'printf should-not-run'\n",
    )
    .unwrap();
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!("[projects]\ntrusted = [{:?}]\n", root.to_str().unwrap()),
    )
    .unwrap();
    let error = refusal(
        request(
            &client,
            Command::CheckRun {
                project: "app".into(),
                check: "fixture".into(),
                checkout: Some(clone.display().to_string()),
            },
        )
        .await,
    );
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains(&operation.to_string()), "{error:?}");
    let run = pending_run(&database, clone.clone()).await;
    let error = refusal(
        request(
            &client,
            Command::ApprovalApprove {
                request: run.id.0,
                revision: run.revision,
                digest: run.digest(),
            },
        )
        .await,
    );
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains(&operation.to_string()), "{error:?}");
    let current = cockpit_approvals::shown_run(&database.read().await.unwrap(), run.id, unix_now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.state, cockpit_approvals::RunState::Asking);
    assert_eq!(
        std::fs::read(clone.join("target/output")).unwrap(),
        b"fixture"
    );
    let started = new_session(&client, &root, None).await;
    assert_eq!(started.state, SessionStateView::Running);
    assert_eq!(home.launches(1).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_target_prune_consumes_equality_at_configured_interval_and_confirms_above_threshold()
 {
    for extra in [0, 1] {
        let home = Home::new();
        let daemon = home.start(BASELINE, settings()).await;
        let client = connect(&daemon).await;
        home.project(&client).await;
        let (_, preview) = prepared_cleanup(&home).await;
        let bytes = 1024 * 1024 + extra;
        std::fs::OpenOptions::new()
            .write(true)
            .open(preview.checkout.path.join("target/output"))
            .unwrap()
            .set_len(bytes)
            .unwrap();
        drop(client);
        home.stop(daemon);
        let config = home.path().join("home/.config/torlek");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.toml"), "[storage]\ntarget_prune_automatic = true\ntarget_prune_projects = ['app']\ntarget_prune_confirm_above_mib = 1\ntarget_prune_interval_minutes = 1\ntarget_prune_base_ref = 'refs/heads/main'\n").unwrap();
        let daemon = home.start(BASELINE, settings()).await;
        let client = connect(&daemon).await;
        if extra == 0 {
            let deadline = Instant::now() + GENEROUS;
            while preview.checkout.path.join("target").exists() {
                assert!(
                    Instant::now() < deadline,
                    "automatic policy never pruned equality"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let database = home.database.lock().unwrap().clone().unwrap();
            let deadline = Instant::now() + GENEROUS;
            loop {
                let read = database.read().await.unwrap();
                let mut rows = read.query("SELECT id FROM project_target_prunes WHERE checkout_id=?1 AND phase='complete'", [preview.checkout.id.0]).await.unwrap();
                if let Some(row) = rows.next().await.unwrap() {
                    let status = cockpit_projects::prune_status(&read, row.get(0).unwrap())
                        .await
                        .unwrap();
                    assert_eq!(status.apparent_bytes, bytes);
                    assert_eq!(status.removed_apparent_bytes, bytes);
                    break;
                }
                drop(rows);
                drop(read);
                assert!(
                    Instant::now() < deadline,
                    "automatic removal lacks a completion receipt"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            // A newly created disposable target waits for the configured next minute;
            // the production timer, not a fixture-only rescan shortcut, must consume it.
            std::fs::create_dir(preview.checkout.path.join("target")).unwrap();
            std::fs::write(preview.checkout.path.join("target/next"), b"next").unwrap();
            let recreated = Instant::now();
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(
                std::fs::read(preview.checkout.path.join("target/next")).unwrap(),
                b"next"
            );
            let deadline = recreated + Duration::from_secs(70);
            while preview.checkout.path.join("target").exists() {
                assert!(
                    Instant::now() < deadline,
                    "configured automatic interval never revisited target"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert!(
                recreated.elapsed() >= Duration::from_secs(50),
                "cleanup bypassed the configured interval"
            );
        } else {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let reply = request(
                &client,
                Command::StoragePrunePreview {
                    checkout: preview.checkout.id.0,
                    prepare: false,
                },
            )
            .await
            .unwrap();
            let Reply::StoragePrunePreview { preview: report } = reply else {
                panic!("{reply:?}")
            };
            assert!(report.confirmation_required);
            assert_eq!(
                std::fs::metadata(preview.checkout.path.join("target/output"))
                    .unwrap()
                    .len(),
                bytes
            );
            let reply = request(
                &client,
                Command::StoragePrunePreview {
                    checkout: preview.checkout.id.0,
                    prepare: true,
                },
            )
            .await
            .unwrap();
            let Reply::StoragePrunePreview { preview: report } = reply else {
                panic!("{reply:?}")
            };
            let plan = report.plan.unwrap();
            let reply = request(
                &client,
                Command::StoragePruneApply {
                    plan: plan.operation,
                    revision: plan.revision,
                    epoch: plan.epoch,
                },
            )
            .await
            .unwrap();
            let Reply::StoragePruneStatus { status } = reply else {
                panic!("{reply:?}")
            };
            assert_eq!(status.phase, "complete");
            assert_eq!(status.removed_apparent_bytes, bytes);
        }
        assert_eq!(
            std::fs::read(preview.checkout.path.join("src/README")).unwrap(),
            b"fixture\n"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_target_prune_commands_are_refused_without_replacing_a_plan_or_touching_files() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let owner = connect(&daemon).await;
    home.project(&owner).await;
    let (plan, preview) = prepared_cleanup(&home).await;
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    for command in [
        Command::StoragePrunePreview {
            checkout: preview.checkout.id.0,
            prepare: false,
        },
        Command::StoragePrunePreview {
            checkout: preview.checkout.id.0,
            prepare: true,
        },
        Command::StoragePruneApply {
            plan: plan.operation,
            revision: plan.revision,
            epoch: plan.epoch.clone(),
        },
        Command::StoragePruneStatus {
            operation: plan.operation,
        },
    ] {
        assert_eq!(
            refusal(request(&agent, command).await).code,
            ErrorCode::NotAuthorized
        );
    }
    let database = home.database.lock().unwrap().clone().unwrap();
    assert_eq!(
        cockpit_projects::prune_status(&database.read().await.unwrap(), plan.operation)
            .await
            .unwrap(),
        plan
    );
    assert_eq!(
        std::fs::read(preview.checkout.path.join("target/output")).unwrap(),
        b"fixture"
    );
}

async fn cleanup_step(home: &Home, operation: i64, commit_receipt: bool) {
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
            &cockpit_projects::PruneEvidence::default(),
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        step
    };
    let receipt = step
        .run(Arc::clone(&database), cockpit_projects::Git::from_path())
        .await
        .unwrap();
    if commit_receipt {
        let write = database.write().await.unwrap();
        cockpit_projects::complete_prune_step(&write, database.epoch(), receipt)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_daemon_restart_recovers_reserved_detached_and_unreceipted_unlinks() {
    // Real durable boundaries: reserved; detached before receipt; unlinked before receipt.
    for completed_steps in [0, 1, 2] {
        let home = Home::new();
        let config = home.path().join("home/.config/torlek");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
        )
        .unwrap();
        let daemon = home.start(BASELINE, settings()).await;
        let client = connect(&daemon).await;
        home.project(&client).await;
        let (operation, clone) = reserve_cleanup(&home).await;
        for _ in 0..completed_steps {
            cleanup_step(&home, operation, true).await;
        }
        if completed_steps != 0 {
            cleanup_step(&home, operation, false).await;
        }
        drop(client);
        home.stop(daemon);
        let daemon = home.start(BASELINE, settings()).await;
        let client = connect(&daemon).await;
        let before = request(&client, Command::StoragePruneStatus { operation })
            .await
            .unwrap();
        let Reply::StoragePruneStatus { status } = before else {
            panic!("{before:?}")
        };
        assert_eq!(status.phase, "paused");
        assert_eq!(status.removed_apparent_bytes, 0);
        let result = request(
            &client,
            Command::StoragePruneApply {
                plan: operation,
                revision: status.revision,
                epoch: status.epoch.clone(),
            },
        )
        .await
        .unwrap();
        let Reply::StoragePruneStatus { status } = result else {
            panic!("{result:?}")
        };
        assert_eq!(status.phase, "complete", "{status:?}");
        if completed_steps == 2 {
            assert_eq!(
                (
                    status.removed_apparent_bytes,
                    status.absent_unconfirmed_bytes
                ),
                (0, 7)
            );
        } else {
            assert_eq!(
                (
                    status.removed_apparent_bytes,
                    status.absent_unconfirmed_bytes
                ),
                (7, 0)
            );
        }
        assert!(!clone.join("target").exists());
        assert_eq!(
            std::fs::read(clone.join("src/README")).unwrap(),
            b"fixture\n"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn target_prune_guard_committed_during_check_git_await_is_checked_before_reservation() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let wrapper = home.path().join("git-latch");
    let armed = home.path().join("armed");
    let claimed = home.path().join("claimed");
    let entered = home.path().join("entered");
    let resume = home.path().join("resume");
    let quote = |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
    let script = format!(
        "#!/bin/sh\nexport HOME={} GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1\ncase \"$*\" in\n*--get-regexp*)\n if [ -f {} ] && /usr/bin/mkdir {} 2>/dev/null; then\n  : > {}\n  n=0\n  while [ ! -f {} ]; do n=$((n+1)); [ \"$n\" -lt 1000 ] || exit 125; /usr/bin/sleep 0.01; done\n fi;;\nesac\nexec /usr/bin/git \"$@\"\n",
        quote(&home.path().join("isolated-git-home")),
        quote(&armed),
        quote(&claimed),
        quote(&entered),
        quote(&resume)
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = home
        .start_wired(|| {
            let mut wiring = home.wiring(BASELINE, settings());
            wiring.git = cockpit_projects::Git::with_program(wrapper.clone());
            wiring
        })
        .await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    std::fs::create_dir(root.join(".torlek")).unwrap();
    std::fs::write(
        root.join(".torlek/config.toml"),
        "[[checks]]\nname = 'fixture'\ncommand = 'printf should-not-run'\n",
    )
    .unwrap();
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!("[projects]\ntrusted = [{:?}]\n", root.to_str().unwrap()),
    )
    .unwrap();
    std::fs::write(&armed, "").unwrap();
    let pending = tokio::spawn(async move {
        request(
            &client,
            Command::CheckRun {
                project: "app".into(),
                check: "fixture".into(),
                checkout: None,
            },
        )
        .await
    });
    let deadline = Instant::now() + GENEROUS;
    while !entered.exists() {
        assert!(
            Instant::now() < deadline,
            "the real check did not reach its Git await"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The request already passed source/permission setup, but cannot use an earlier
    // no-maintenance snapshot after this independently committed reservation.
    let (operation, clone) = reserve_cleanup(&home).await;
    std::fs::write(&resume, "").unwrap();
    let error = refusal(pending.await.unwrap());
    assert!(error.message.contains(&operation.to_string()), "{error:?}");
    let database = home.database.lock().unwrap().clone().unwrap();
    assert!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read(clone.join("target/output")).unwrap(),
        b"fixture"
    );
}
