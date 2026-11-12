use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn session_start_restart_and_end_supply_current_output_ownership_and_protection() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let id = cockpit_sessions::SessionId::parse(&started.id).unwrap();
    let old = {
        let read = database.read().await.unwrap();
        let session = cockpit_sessions::session(&read, &id).await.unwrap();
        let facts = cockpit_processes::output_facts(&read, session.launch.unwrap().execution)
            .await
            .unwrap();
        assert_eq!(facts.mode, cockpit_processes::OutputMode::Hosted);
        assert_eq!(facts.session.as_deref(), Some(started.id.as_str()));
        assert!(facts.project.is_some());
        assert!(
            cockpit_sessions::protects_output(&read, &facts)
                .await
                .unwrap()
        );
        facts
    };
    // The latest captured file run is independent of newer hosted/PTY launches.
    let supervisor = cockpit_processes::Supervisor::new(
        home.path().join("output-fixture"),
        cockpit_processes::LaunchContainment::ProcessGroup,
        cockpit_processes::SystemdTools::from_path(),
    );
    let mut file_runs = Vec::new();
    for _ in 0..2 {
        let write = database.write().await.unwrap();
        let execution = supervisor.reserve(&write, database.epoch()).await.unwrap();
        cockpit_processes::attribute_output(
            &write,
            execution,
            database.epoch(),
            old.project.as_deref(),
            Some(&started.id),
            cockpit_processes::OutputMode::Files,
            false,
        )
        .await
        .unwrap();
        let guard = cockpit_processes::guard_output_in(&write, Arc::clone(&database), execution)
            .await
            .unwrap();
        write.commit().await.unwrap();
        let spec = cockpit_processes::LaunchSpec {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "printf retained".into()],
            working_dir: root.clone(),
            environment: vec![],
            terminal: None,
        };
        supervisor
            .launch_reserved_with_output(execution, &spec, guard)
            .await
            .unwrap()
            .exit
            .await
            .unwrap()
            .unwrap();
        supervisor.observe(&database, execution).await.unwrap();
        let facts = cockpit_processes::output_facts(&database.read().await.unwrap(), execution)
            .await
            .unwrap();
        assert!(!facts.protected());
        file_runs.push(facts);
    }
    {
        let read = database.read().await.unwrap();
        assert!(
            !cockpit_sessions::protects_output(&read, &file_runs[0])
                .await
                .unwrap()
        );
        assert!(
            cockpit_sessions::protects_output(&read, &file_runs[1])
                .await
                .unwrap()
        );
    }
    session_reply(
        &client,
        Command::SessionRestart {
            session: started.id.clone(),
        },
    )
    .await;
    {
        let read = database.read().await.unwrap();
        let session = cockpit_sessions::session(&read, &id).await.unwrap();
        let new = cockpit_processes::output_facts(&read, session.launch.unwrap().execution)
            .await
            .unwrap();
        assert_ne!(new.execution, old.execution);
        assert_eq!(new.origin_epoch.as_deref(), Some(database.epoch()));
        assert!(
            cockpit_sessions::protects_output(&read, &new)
                .await
                .unwrap()
        );
        assert!(
            !cockpit_sessions::protects_output(&read, &old)
                .await
                .unwrap()
        );
    }
    session_reply(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;
    {
        let read = database.read().await.unwrap();
        assert!(
            cockpit_sessions::protects_output(&read, &file_runs[1])
                .await
                .unwrap(),
            "asleep retains its latest file output after hosted replacement"
        );
    }
    session_reply(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;
    let read = database.read().await.unwrap();
    let session = cockpit_sessions::session(&read, &id).await.unwrap();
    assert!(
        !cockpit_sessions::protects_output(&read, &file_runs[1])
            .await
            .unwrap()
    );
    let new = cockpit_processes::output_facts(&read, session.launch.unwrap().execution)
        .await
        .unwrap();
    assert!(
        !cockpit_sessions::protects_output(&read, &new)
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_recovery_waits_for_a_cancelled_readers_actual_blocking_work() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let supervisor = cockpit_processes::Supervisor::new(
        home.path().join("state/cockpit/executions"),
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
    let pending = cockpit_verification::reserve_result(
        &write,
        database.epoch(),
        execution.0,
        cockpit_verification::NewReceipt {
            ran: None,
            project: "app".into(),
            run_id: run.clone(),
            check: cockpit_verification::CheckDefinition {
                name: "fixture".into(),
                command: "printf protected".into(),
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
    let guard = cockpit_processes::guard_output_in(&write, Arc::clone(&database), execution)
        .await
        .unwrap();
    write.commit().await.unwrap();
    let spec = cockpit_processes::LaunchSpec {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), "printf protected".into()],
        working_dir: root,
        environment: vec![],
        terminal: None,
    };
    let launched = supervisor
        .launch_reserved_with_output(execution, &spec, guard.clone())
        .await
        .unwrap();
    launched.exit.await.unwrap().unwrap();
    let before = cockpit_processes::execution(&database, execution)
        .await
        .unwrap();
    let stale = Command::CheckResolvePending {
        project: "app".into(),
        run: run.clone(),
        revision: pending.revision + 1,
        database_epoch: database.epoch().to_owned(),
    };
    assert_eq!(
        refusal(request(&client, stale).await).code,
        ErrorCode::StaleRevision
    );
    assert_eq!(
        cockpit_processes::execution(&database, execution)
            .await
            .unwrap()
            .phase,
        before.phase,
        "stale recovery must not observe/mutate containment"
    );
    supervisor.observe(&database, execution).await.unwrap();
    assert!(
        cockpit_processes::observed_ended(&database.read().await.unwrap(), &[execution])
            .await
            .unwrap()
    );
    let path = supervisor.output_path(&database, execution, cockpit_processes::OutputKind::Stdout);
    let (release, wait) = std::sync::mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let task = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _ = entered.send(());
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"protected");
        let _ = finished.send(());
    });
    ready.await.unwrap();
    task.abort();
    let recover = Command::CheckResolvePending {
        project: "app".into(),
        run,
        revision: pending.revision,
        database_epoch: database.epoch().to_owned(),
    };
    assert_eq!(
        refusal(request(&client, recover.clone()).await).code,
        ErrorCode::Conflict
    );
    release.send(()).unwrap();
    done.await.unwrap();
    let deadline = Instant::now() + GENEROUS;
    loop {
        match request(&client, recover.clone()).await {
            Ok(Reply::CheckReceipt {
                receipt: Some(receipt),
            }) => {
                assert_eq!(receipt.outcome, cockpit_protocol::CheckOutcomeView::Unknown);
                break;
            }
            Err(_) if Instant::now() < deadline => tokio::task::yield_now().await,
            other => panic!("{other:?}"),
        }
    }
}
