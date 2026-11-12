use super::*;

async fn pending(home: &TestHome) -> Vec<cockpit_protocol::PendingCheckView> {
    match home
        .connect()
        .await
        .request(
            Command::CheckPending {
                project: "app".into(),
                after: None,
            },
            GENEROUS,
        )
        .await
        .unwrap()
    {
        Reply::CheckPending {
            pending,
            next: None,
        } => pending,
        other => panic!("expected one pending page: {other:?}"),
    }
}
fn recover(row: &cockpit_protocol::PendingCheckView) -> Command {
    Command::CheckResolvePending {
        project: row.project.clone(),
        run: row.run.clone(),
        revision: row.revision,
        database_epoch: row.database_epoch.clone(),
    }
}

#[tokio::test]
async fn pending_recovery_refuses_agents_and_active_work_then_survives_daemon_crash() {
    let verification = started().await;
    std::fs::write(verification.root.join(".torlek/config.toml"), "[[checks]]\nname='wait'\ncommand='echo started; i=0; while [ ! -f release ] && [ $i -lt 500 ]; do i=$((i+1)); sleep 0.02; done; echo ended'\n").unwrap();
    let run = match verification
        .as_owner(verification.owner_run("wait"))
        .await
        .unwrap()
    {
        Reply::CheckRunStarted { run } => run,
        other => panic!("{other:?}"),
    };
    let rows = pending(&verification.home).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.run, run);
    assert!(row.active);
    assert!(row.collector_active);
    assert_eq!(
        refused(verification.as_agent(recover(row)).await).code,
        ErrorCode::NotAuthorized
    );
    assert_eq!(
        refused(verification.as_owner(recover(row)).await).code,
        ErrorCode::Conflict
    );
    let mut stale = row.clone();
    stale.database_epoch = "not-current".into();
    assert_eq!(
        refused(verification.as_owner(recover(&stale)).await).code,
        ErrorCode::StaleRevision
    );
    stale = row.clone();
    stale.revision += 1;
    assert_eq!(
        refused(verification.as_owner(recover(&stale)).await).code,
        ErrorCode::StaleRevision
    );
    let Verification {
        _daemon,
        home,
        root,
        token: _,
        _cwd,
    } = verification;
    _daemon.kill();
    std::fs::write(root.join("release"), "go").unwrap();
    let daemon = home.start();
    let rows = pending(&home).await;
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].collector_active);
    let deadline = Instant::now() + GENEROUS;
    let reply = loop {
        match home
            .connect()
            .await
            .request(recover(&rows[0]), GENEROUS)
            .await
        {
            Ok(reply) => break reply,
            Err(ClientError::Daemon(error))
                if error.code == ErrorCode::Conflict && Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            other => panic!("recovery: {other:?}"),
        }
    };
    let Reply::CheckReceipt {
        receipt: Some(receipt),
    } = reply
    else {
        panic!("expected recovery receipt")
    };
    assert_eq!(receipt.outcome, CheckOutcomeView::Unknown);
    assert_eq!(receipt.exit_code, None);
    assert_eq!(
        home.connect()
            .await
            .request(recover(&rows[0]), GENEROUS)
            .await
            .unwrap(),
        Reply::CheckReceipt {
            receipt: Some(receipt.clone())
        }
    );
    assert!(pending(&home).await.is_empty());
    drop(daemon);
    let db = home.open_state().await;
    let read = db.read().await.unwrap();
    let facts = cockpit_processes::output_facts(
        &read,
        cockpit_processes::ExecutionId(row_execution(&rows)),
    )
    .await
    .unwrap();
    assert_eq!(facts.project.as_deref(), Some("app"));
    assert!(!facts.pending_result);
    let stdout = home
        .state_home()
        .join(common::BUILD_PROFILE_DIR)
        .join("executions")
        .join(&rows[0].origin_epoch)
        .join(rows[0].execution.to_string())
        .join("stdout");
    assert!(std::fs::read_to_string(stdout).unwrap().contains("started"));
}
fn row_execution(rows: &[cockpit_protocol::PendingCheckView]) -> i64 {
    rows[0].execution
}

#[tokio::test]
async fn a_proven_never_started_check_keeps_its_original_launch_refusal() {
    let verification = started().await;
    std::fs::write(
        verification.root.join(".torlek/config.toml"),
        "[[checks]]\nname='missing'\ncommand='true'\nworking_directory='missing-directory'\n",
    )
    .unwrap();
    assert_eq!(
        refused(
            verification
                .as_owner(verification.owner_run("missing"))
                .await
        )
        .code,
        ErrorCode::Refused
    );
    assert!(pending(&verification.home).await.is_empty());
}

#[tokio::test]
async fn reservation_before_launch_survives_restore_and_needs_current_epoch_recovery() {
    let verification = started().await;
    let Verification {
        _daemon,
        home,
        root: _,
        token: _,
        _cwd,
    } = verification;
    _daemon.kill();
    let database = home.open_state().await;
    let supervisor = cockpit_processes::Supervisor::new(
        home.state_home()
            .join(common::BUILD_PROFILE_DIR)
            .join("executions"),
        cockpit_processes::LaunchContainment::ProcessGroup,
        cockpit_processes::SystemdTools::from_path(),
    );
    let write = database.write().await.unwrap();
    let execution = supervisor.reserve(&write, database.epoch()).await.unwrap();
    cockpit_processes::attribute_output(
        &write,
        execution,
        database.epoch(),
        Some("app"),
        None,
        cockpit_processes::OutputMode::Files,
        true,
    )
    .await
    .unwrap();
    let run = format!("execution-{}-{}", database.epoch(), execution.0);
    let now = common::unix_now();
    let original = cockpit_verification::reserve_result(
        &write,
        database.epoch(),
        execution.0,
        cockpit_verification::NewReceipt {
            ran: None,
            project: "app".into(),
            run_id: run.clone(),
            check: cockpit_verification::CheckDefinition {
                name: "fixture".into(),
                command: "true".into(),
                working_directory: ".".into(),
                environment: "native".into(),
            },
            source: cockpit_verification::Source {
                head: "fixture".into(),
                base: None,
                dirty: None,
            },
            artifacts: vec![],
            outcome: cockpit_verification::Outcome::Unknown,
            exit_code: None,
            output: String::new(),
            output_references: vec![],
            started_at: now,
            finished_at: now,
        },
    )
    .await
    .unwrap();
    // An unrelated pending reservation must remain protected after recovering this one.
    let unrelated = supervisor.reserve(&write, database.epoch()).await.unwrap();
    cockpit_processes::attribute_output(
        &write,
        unrelated,
        database.epoch(),
        Some("other"),
        None,
        cockpit_processes::OutputMode::Files,
        true,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    let backup = home.home().join("backup.db");
    database.backup_to(&backup).await.unwrap();
    let old_epoch = database.epoch().to_owned();
    drop(database);
    let db_path = home
        .state_home()
        .join(common::BUILD_PROFILE_DIR)
        .join("state.db");
    cockpit_turso::restore(&backup, &db_path).await.unwrap();
    let daemon = home.start();
    let rows = pending(&home).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].origin_epoch, old_epoch);
    assert_ne!(rows[0].database_epoch, old_epoch);
    let mut stale = rows[0].clone();
    stale.database_epoch = old_epoch;
    assert_eq!(
        refused(
            home.connect()
                .await
                .request(recover(&stale), GENEROUS)
                .await
        )
        .code,
        ErrorCode::StaleRevision
    );
    let reply = home
        .connect()
        .await
        .request(recover(&rows[0]), GENEROUS)
        .await
        .unwrap();
    assert!(
        matches!(reply,Reply::CheckReceipt {receipt:Some(ref r)} if r.outcome==CheckOutcomeView::Unknown)
    );
    drop(daemon);
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    assert!(
        cockpit_processes::output_facts(&write, unrelated)
            .await
            .unwrap()
            .pending_result
    );
    assert!(
        cockpit_verification::seal_result(
            &write,
            &original,
            original.receipt.clone(),
            0,
            now,
            false
        )
        .await
        .is_err(),
        "pre-restore callback cannot revive recovered intent"
    );
}

#[tokio::test]
async fn fast_results_never_report_an_orphan_while_the_collector_commits() {
    let verification = started().await;
    for _ in 0..32 {
        let receipt = verification.run("lint").await;
        assert_eq!(receipt.outcome, CheckOutcomeView::Passed);
    }
}
