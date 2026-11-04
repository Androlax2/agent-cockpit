//! Issue #410: the coordinator grants (`observe-sessions`, `message-sessions`,
//! `start-sessions`), the start terms and their hourly cap, and the audit of their use, on a
//! real state database.
mod common;

use cockpit_authorization::{
    Actor, Audit, AuditedAction, Basis, Error, Outcome, Permission, Refusal, Scope, StartAllowance,
    StartTerms, active_coordinator_grants, active_grants, audit_entry, audit_of, evaluate,
    grant_start_sessions, latest_audit_on, latest_audits_by_target, record_audit, revoke,
    start_allowance, started_sessions, stored_grant,
};
use cockpit_turso::Database;
use common::{grant, open, session};

const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

async fn allowance(database: &Database, holder: &str, now: i64) -> StartAllowance {
    allowance_with_active(database, holder, now, 0).await
}

async fn allowance_with_active(
    database: &Database,
    holder: &str,
    now: i64,
    active: u32,
) -> StartAllowance {
    let read = database.read().await.unwrap();
    start_allowance(&read, &session(holder), now, active)
        .await
        .unwrap()
}

async fn give_start_terms(database: &Database, holder: &str, terms: StartTerms) {
    let write = database.write().await.unwrap();
    grant_start_sessions(&write, &session(holder), terms)
        .await
        .unwrap();
    write.commit().await.unwrap();
}

/// Records a start by `holder` at `at`, under the grant its allowance named.
async fn record_start(database: &Database, holder: &str, at: i64) {
    let StartAllowance::Allowed {
        grant, revision, ..
    } = allowance(database, holder, at).await
    else {
        panic!("{holder} may not start at {at}");
    };
    let write = database.write().await.unwrap();
    record_audit(
        &write,
        &Audit {
            actor: session(holder),
            permission: Permission::StartSessions,
            grant,
            revision,
            action: AuditedAction::Start,
            target: None,
            detail: None,
            at,
        },
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
}

#[tokio::test]
async fn no_other_grant_implies_a_coordinator_grant() {
    let database = open("coordinator-not-implied").await;
    let holder = session("coordinator");
    for permission in Permission::ALL {
        if permission.is_owner_only() || permission.is_coordinator() {
            continue;
        }
        grant(&database, &holder, permission, &Scope::Owner).await;
    }
    let read = database.read().await.unwrap();

    for permission in Permission::COORDINATOR {
        let decision = evaluate(
            &read,
            &Actor::Session(holder.clone()),
            permission,
            &Scope::Session(session("worker")),
        )
        .await
        .unwrap();
        assert!(
            matches!(decision.outcome, Outcome::Refused(Refusal::NoGrant { .. })),
            "{permission} was implied: {decision:?}"
        );
    }
}

#[tokio::test]
async fn each_coordinator_grant_allows_only_its_own_permission() {
    let database = open("coordinator-one-each").await;
    let holder = session("coordinator");
    let observe = grant(
        &database,
        &holder,
        Permission::ObserveSessions,
        &Scope::Owner,
    )
    .await;
    let read = database.read().await.unwrap();
    let decide = |permission| {
        let read = &read;
        let holder = holder.clone();
        async move {
            evaluate(
                read,
                &Actor::Session(holder),
                permission,
                &Scope::Session(session("worker")),
            )
            .await
            .unwrap()
            .outcome
        }
    };

    assert_eq!(
        decide(Permission::ObserveSessions).await,
        Outcome::Allowed(Basis::Grant(observe.id))
    );
    assert!(matches!(
        decide(Permission::MessageSessions).await,
        Outcome::Refused(_)
    ));
    assert!(matches!(
        decide(Permission::StartSessions).await,
        Outcome::Refused(_)
    ));
}

#[tokio::test]
async fn a_session_without_start_sessions_is_refused_naming_the_grant() {
    let database = open("start-no-grant").await;

    assert_eq!(
        allowance(&database, "coordinator", NOW).await,
        StartAllowance::Refused(Refusal::NoGrant {
            permission: Permission::StartSessions,
            scope: Scope::Owner,
        })
    );
}

#[tokio::test]
async fn a_plain_start_sessions_grant_confirms_every_start() {
    let database = open("start-default-terms").await;
    let given = grant(
        &database,
        &session("coordinator"),
        Permission::StartSessions,
        &Scope::Owner,
    )
    .await;

    assert_eq!(given.start_terms, Some(StartTerms::DEFAULT));
    assert!(matches!(
        allowance(&database, "coordinator", NOW).await,
        StartAllowance::Allowed {
            needs_confirmation: true,
            ..
        }
    ));
}

/// #739: the terms a grant given without any are eight sessions at once and twenty starts an
/// hour, each confirmed.
#[test]
fn the_default_terms_are_eight_at_once_and_twenty_an_hour_confirmed() {
    assert_eq!(
        StartTerms::DEFAULT,
        StartTerms {
            per_hour: 20,
            max_active: 8,
            needs_confirmation: true,
        }
    );
}

#[tokio::test]
async fn starts_beyond_the_hourly_cap_are_refused_until_the_oldest_leaves_the_hour() {
    let database = open("start-cap").await;
    give_start_terms(
        &database,
        "coordinator",
        StartTerms {
            per_hour: 2,
            max_active: StartTerms::DEFAULT_MAX_ACTIVE,
            needs_confirmation: false,
        },
    )
    .await;
    record_start(&database, "coordinator", NOW).await;
    record_start(&database, "coordinator", NOW + 10).await;

    assert_eq!(
        allowance(&database, "coordinator", NOW + 20).await,
        StartAllowance::Capped {
            per_hour: 2,
            retry_at: NOW + HOUR,
        }
    );
    assert!(matches!(
        allowance(&database, "coordinator", NOW + HOUR).await,
        StartAllowance::Allowed {
            needs_confirmation: false,
            ..
        }
    ));
}

#[tokio::test]
async fn the_cap_counts_one_holders_starts_only() {
    let database = open("start-cap-per-holder").await;
    let terms = StartTerms {
        per_hour: 1,
        max_active: StartTerms::DEFAULT_MAX_ACTIVE,
        needs_confirmation: true,
    };
    give_start_terms(&database, "first", terms).await;
    give_start_terms(&database, "second", terms).await;
    record_start(&database, "first", NOW).await;

    assert!(matches!(
        allowance(&database, "second", NOW + 1).await,
        StartAllowance::Allowed { .. }
    ));
}

#[tokio::test]
async fn granting_other_start_terms_replaces_the_grant_and_keeps_the_count() {
    let database = open("start-regrant").await;
    give_start_terms(
        &database,
        "coordinator",
        StartTerms {
            per_hour: 1,
            max_active: StartTerms::DEFAULT_MAX_ACTIVE,
            needs_confirmation: true,
        },
    )
    .await;
    record_start(&database, "coordinator", NOW).await;
    give_start_terms(
        &database,
        "coordinator",
        StartTerms {
            per_hour: 1,
            max_active: StartTerms::DEFAULT_MAX_ACTIVE,
            needs_confirmation: false,
        },
    )
    .await;
    let read = database.read().await.unwrap();

    let held = active_grants(&read, &session("coordinator")).await.unwrap();
    assert_eq!(held.len(), 1, "one start-sessions grant stays: {held:?}");
    assert_eq!(
        held[0].start_terms.map(|terms| terms.needs_confirmation),
        Some(false)
    );
    assert!(
        matches!(
            allowance(&database, "coordinator", NOW + 1).await,
            StartAllowance::Capped { .. }
        ),
        "granting again must not reset the hour's count"
    );
}

#[tokio::test]
async fn a_cap_of_zero_is_refused() {
    let database = open("start-zero-cap").await;
    let write = database.write().await.unwrap();

    let refused = grant_start_sessions(
        &write,
        &session("coordinator"),
        StartTerms {
            per_hour: 0,
            max_active: StartTerms::DEFAULT_MAX_ACTIVE,
            needs_confirmation: true,
        },
    )
    .await;

    assert!(matches!(refused, Err(Error::InvalidStartTerms { .. })));
}

/// #739: the terms for the tests of `max_active`, with an hourly cap they never reach.
const TWO_AT_ONCE: StartTerms = StartTerms {
    per_hour: 100,
    max_active: 2,
    needs_confirmation: true,
};

#[tokio::test]
async fn a_start_at_max_active_is_refused_with_the_active_count() {
    let database = open("start-at-capacity").await;
    give_start_terms(&database, "coordinator", TWO_AT_ONCE).await;

    assert_eq!(
        allowance_with_active(&database, "coordinator", NOW, 2).await,
        StartAllowance::AtCapacity {
            max_active: 2,
            active: 2,
        }
    );
}

#[tokio::test]
async fn a_start_below_max_active_is_allowed() {
    let database = open("start-below-capacity").await;
    give_start_terms(&database, "coordinator", TWO_AT_ONCE).await;

    assert!(matches!(
        allowance_with_active(&database, "coordinator", NOW, 1).await,
        StartAllowance::Allowed { .. }
    ));
}

#[tokio::test]
async fn the_hourly_cap_still_applies_below_max_active() {
    let database = open("start-cap-below-capacity").await;
    give_start_terms(
        &database,
        "coordinator",
        StartTerms {
            per_hour: 1,
            ..TWO_AT_ONCE
        },
    )
    .await;
    record_start(&database, "coordinator", NOW).await;

    assert_eq!(
        allowance_with_active(&database, "coordinator", NOW + 1, 0).await,
        StartAllowance::Capped {
            per_hour: 1,
            retry_at: NOW + HOUR,
        }
    );
}

#[tokio::test]
async fn a_max_active_of_zero_is_refused() {
    let database = open("start-zero-at-once").await;
    let write = database.write().await.unwrap();

    let refused = grant_start_sessions(
        &write,
        &session("coordinator"),
        StartTerms {
            max_active: 0,
            ..TWO_AT_ONCE
        },
    )
    .await;

    assert!(matches!(refused, Err(Error::InvalidStartTerms { .. })));
}

#[tokio::test]
async fn started_sessions_names_each_session_the_holder_started_once() {
    let database = open("started-sessions").await;
    give_start_terms(&database, "coordinator", StartTerms::DEFAULT).await;
    give_start_terms(&database, "other", StartTerms::DEFAULT).await;
    let write = database.write().await.unwrap();
    for (actor, action, target) in [
        ("coordinator", AuditedAction::Started, "worker-b"),
        ("coordinator", AuditedAction::Started, "worker-a"),
        ("coordinator", AuditedAction::Started, "worker-a"),
        ("coordinator", AuditedAction::Message, "worker-c"),
        ("other", AuditedAction::Started, "worker-d"),
    ] {
        let permission = match action {
            AuditedAction::Message => Permission::MessageSessions,
            _ => Permission::StartSessions,
        };
        record_audit(
            &write,
            &Audit {
                actor: session(actor),
                permission,
                grant: stored_start_grant(&write, actor).await,
                revision: cockpit_authorization::current_revision(&write)
                    .await
                    .unwrap(),
                action,
                target: Some(session(target)),
                detail: None,
                at: NOW,
            },
        )
        .await
        .unwrap();
    }
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let started = started_sessions(&read, &session("coordinator"))
        .await
        .unwrap();

    assert_eq!(started, [session("worker-a"), session("worker-b")]);
}

async fn stored_start_grant(
    connection: &cockpit_turso::sql::Connection,
    holder: &str,
) -> cockpit_authorization::GrantId {
    cockpit_authorization::active_grant(
        connection,
        &session(holder),
        Permission::StartSessions,
        &Scope::Owner,
    )
    .await
    .unwrap()
    .expect("the holder has start-sessions")
    .id
}

#[tokio::test]
async fn a_revoked_start_grant_refuses_the_next_start() {
    let database = open("start-revoked").await;
    let given = grant(
        &database,
        &session("coordinator"),
        Permission::StartSessions,
        &Scope::Owner,
    )
    .await;
    let write = database.write().await.unwrap();
    revoke(&write, given.id).await.unwrap();
    write.commit().await.unwrap();

    assert!(matches!(
        allowance(&database, "coordinator", NOW).await,
        StartAllowance::Refused(_)
    ));
}

#[tokio::test]
async fn the_audit_keeps_both_sessions_of_each_action() {
    let database = open("audit-both-sessions").await;
    let given = grant(
        &database,
        &session("coordinator"),
        Permission::MessageSessions,
        &Scope::Owner,
    )
    .await;
    let message = Audit {
        actor: session("coordinator"),
        permission: Permission::MessageSessions,
        grant: given.id,
        revision: given.granted_at,
        action: AuditedAction::Message,
        target: Some(session("worker")),
        detail: None,
        at: NOW,
    };
    let write = database.write().await.unwrap();
    let id = record_audit(&write, &message).await.unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let by_actor = audit_of(&read, &session("coordinator"), 10).await.unwrap();
    let by_target = audit_of(&read, &session("worker"), 10).await.unwrap();
    let latest = latest_audit_on(&read, &session("worker"), AuditedAction::Message)
        .await
        .unwrap()
        .expect("the message is the worker's latest");

    assert_eq!(by_actor.len(), 1);
    assert_eq!(by_actor[0].audit, message);
    assert_eq!(by_target, by_actor);
    assert_eq!(latest.id, id);
}

#[tokio::test]
async fn a_grant_that_is_neither_a_coordinators_nor_change_state_is_not_audited() {
    let database = open("audit-coordinator-only").await;
    let given = grant(
        &database,
        &session("coordinator"),
        Permission::RunCommands,
        &Scope::Owner,
    )
    .await;
    let write = database.write().await.unwrap();

    let refused = record_audit(
        &write,
        &Audit {
            actor: session("coordinator"),
            permission: Permission::RunCommands,
            grant: given.id,
            revision: given.granted_at,
            action: AuditedAction::Message,
            target: Some(session("worker")),
            detail: None,
            at: NOW,
        },
    )
    .await;

    assert!(matches!(refused, Err(Error::NotAudited { .. })));
}

#[tokio::test]
async fn the_active_coordinator_grants_leave_out_other_and_revoked_grants() {
    let database = open("active-coordinator-grants").await;
    let observe = grant(
        &database,
        &session("b-coordinator"),
        Permission::ObserveSessions,
        &Scope::Owner,
    )
    .await;
    let message = grant(
        &database,
        &session("a-coordinator"),
        Permission::MessageSessions,
        &Scope::Owner,
    )
    .await;
    grant(
        &database,
        &session("a-coordinator"),
        Permission::RunCommands,
        &Scope::Owner,
    )
    .await;
    let revoked = grant(
        &database,
        &session("a-coordinator"),
        Permission::ObserveSessions,
        &Scope::Owner,
    )
    .await;
    let write = database.write().await.unwrap();
    revoke(&write, revoked.id).await.unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let active = active_coordinator_grants(&read).await.unwrap();

    assert_eq!(active, vec![message, observe]);
}

#[tokio::test]
async fn the_latest_audit_by_target_is_one_entry_per_target() {
    let database = open("latest-audits-by-target").await;
    let given = grant(
        &database,
        &session("coordinator"),
        Permission::MessageSessions,
        &Scope::Owner,
    )
    .await;
    let message_to = |target: &str, at: i64| Audit {
        actor: session("coordinator"),
        permission: Permission::MessageSessions,
        grant: given.id,
        revision: given.granted_at,
        action: AuditedAction::Message,
        target: Some(session(target)),
        detail: Some(format!("at {at}")),
        at,
    };
    let write = database.write().await.unwrap();
    for audit in [
        message_to("worker-1", NOW),
        message_to("worker-2", NOW + 1),
        message_to("worker-1", NOW + 2),
    ] {
        record_audit(&write, &audit).await.unwrap();
    }
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let latest = latest_audits_by_target(&read, AuditedAction::Message)
        .await
        .unwrap();

    let mut latest: Vec<_> = latest.into_iter().map(|entry| entry.audit).collect();
    latest.sort_by_key(|audit| audit.at);
    assert_eq!(
        latest,
        vec![
            message_to("worker-2", NOW + 1),
            message_to("worker-1", NOW + 2)
        ]
    );
}

#[tokio::test]
async fn a_stored_grant_is_read_back_after_its_revocation_and_none_when_never_stored() {
    let database = open("stored-grant").await;
    let given = grant(
        &database,
        &session("coordinator"),
        Permission::StartSessions,
        &Scope::Owner,
    )
    .await;
    let write = database.write().await.unwrap();
    revoke(&write, given.id).await.unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let revoked = stored_grant(&read, given.id.value()).await.unwrap();
    let never_stored = stored_grant(&read, given.id.value() + 1).await.unwrap();

    assert_eq!(revoked, Some(given));
    assert_eq!(never_stored, None);
}

#[tokio::test]
async fn an_audit_entry_is_read_back_by_its_id() {
    let database = open("audit-entry").await;
    let given = grant(
        &database,
        &session("coordinator"),
        Permission::MessageSessions,
        &Scope::Owner,
    )
    .await;
    let message = Audit {
        actor: session("coordinator"),
        permission: Permission::MessageSessions,
        grant: given.id,
        revision: given.granted_at,
        action: AuditedAction::Message,
        target: Some(session("worker")),
        detail: Some("rebase".to_owned()),
        at: NOW,
    };
    let write = database.write().await.unwrap();
    let id = record_audit(&write, &message).await.unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let found = audit_entry(&read, id).await.unwrap();
    let missing = audit_entry(&read, id + 1).await.unwrap();

    assert_eq!(found.map(|entry| entry.audit), Some(message));
    assert_eq!(missing, None);
}

/// #657: the owner's decision on a start a coordinator asked for is audited beside the ask.
#[tokio::test]
async fn the_owners_decisions_on_a_start_are_audited() {
    let database = open("decision-audit").await;
    give_start_terms(&database, "coordinator", StartTerms::DEFAULT).await;
    let StartAllowance::Allowed {
        grant, revision, ..
    } = allowance(&database, "coordinator", NOW).await
    else {
        panic!("the coordinator may start");
    };
    let write = database.write().await.unwrap();
    for action in [AuditedAction::Approved, AuditedAction::Declined] {
        record_audit(
            &write,
            &Audit {
                actor: session("coordinator"),
                permission: Permission::StartSessions,
                grant,
                revision,
                action,
                target: None,
                detail: Some("approval request 3, by the owner through the TUI".to_owned()),
                at: NOW,
            },
        )
        .await
        .unwrap();
    }
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let actions: Vec<AuditedAction> = audit_of(&read, &session("coordinator"), 10)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.audit.action)
        .collect();

    assert_eq!(actions, [AuditedAction::Declined, AuditedAction::Approved]);
}

/// #657 rebuilt the audit table to widen its actions; entries written before keep every column.
#[tokio::test]
async fn entries_audited_before_the_decision_actions_survive_the_rebuild() {
    const BEFORE_DECISIONS: cockpit_turso::MigrationSet = cockpit_turso::MigrationSet {
        owner: "authorization",
        migrations: cockpit_authorization::MIGRATIONS.migrations.split_at(2).0,
    };
    let path = common::database_path("decision-audit-rebuild");
    let before = Database::open(&path, &[BEFORE_DECISIONS]).await.unwrap();
    let write = before.write().await.unwrap();
    write
        .execute(
            "INSERT INTO authorization_audit
                (id, actor, permission, grant_id, revision, action, target, detail, at)
             VALUES (5, 'coordinator', 'start-sessions', 2, 3, 'start', NULL,
                'approval request 3', ?1)",
            [NOW],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(before);

    let after = Database::open(&path, &[cockpit_authorization::MIGRATIONS])
        .await
        .unwrap();
    let read = after.read().await.unwrap();

    let entry = audit_entry(&read, 5).await.unwrap().unwrap();
    assert_eq!(entry.audit.action, AuditedAction::Start);
    assert_eq!(entry.audit.detail.as_deref(), Some("approval request 3"));
    assert_eq!(entry.audit.at, NOW);
}

/// #739: a `start-sessions` grant stored before `max_active` existed reads with the default,
/// and keeps its own hourly cap and confirmation.
#[tokio::test]
async fn a_start_grant_stored_before_max_active_reads_with_the_default() {
    const BEFORE_MAX_ACTIVE: cockpit_turso::MigrationSet = cockpit_turso::MigrationSet {
        owner: "authorization",
        migrations: cockpit_authorization::MIGRATIONS.migrations.split_at(3).0,
    };
    let path = common::database_path("max-active-migration");
    let before = Database::open(&path, &[BEFORE_MAX_ACTIVE]).await.unwrap();
    let write = before.write().await.unwrap();
    write
        .execute(
            "INSERT INTO authorization_grants
                (id, holder, permission, scope_kind, scope_id, granted_at, starts_per_hour,
                 starts_unconfirmed)
             VALUES (7, 'coordinator', 'start-sessions', 'owner', NULL, 1, 10, 1),
                    (8, 'coordinator', 'observe-sessions', 'owner', NULL, 2, NULL, 0)",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(before);

    let after = Database::open(&path, &[cockpit_authorization::MIGRATIONS])
        .await
        .unwrap();
    let read = after.read().await.unwrap();

    let held = active_grants(&read, &session("coordinator")).await.unwrap();
    let terms: Vec<Option<StartTerms>> = held.iter().map(|grant| grant.start_terms).collect();
    assert_eq!(
        terms,
        [
            Some(StartTerms {
                per_hour: 10,
                max_active: StartTerms::DEFAULT_MAX_ACTIVE,
                needs_confirmation: false,
            }),
            None,
        ]
    );
}

#[tokio::test]
async fn end_sessions_is_a_coordinator_grant_no_other_grant_implies() {
    let database = open("end-sessions-grant").await;
    let holder = session("coordinator");
    for other in [
        Permission::ObserveSessions,
        Permission::MessageSessions,
        Permission::StartSessions,
        Permission::ChangeState,
    ] {
        grant(&database, &holder, other, &Scope::Owner).await;
    }
    let read = database.read().await.unwrap();
    let actor = Actor::Session(holder.clone());

    let refused = evaluate(
        &read,
        &actor,
        Permission::EndSessions,
        &Scope::Session(session("worker")),
    )
    .await
    .unwrap();

    assert_eq!(
        refused.outcome,
        Outcome::Refused(Refusal::NoGrant {
            permission: Permission::EndSessions,
            scope: Scope::Session(session("worker")),
        })
    );
    assert!(Permission::EndSessions.is_coordinator());
    assert_eq!(
        "end-sessions".parse::<Permission>().ok(),
        Some(Permission::EndSessions)
    );
}

#[tokio::test]
async fn an_end_is_audited_with_the_ended_session_and_its_item() {
    let database = open("end-audited").await;
    let holder = session("coordinator");
    let given = grant(&database, &holder, Permission::EndSessions, &Scope::Owner).await;
    let write = database.write().await.unwrap();
    let audited = Audit {
        actor: holder.clone(),
        permission: Permission::EndSessions,
        grant: given.id,
        revision: given.granted_at,
        action: AuditedAction::Ended,
        target: Some(session("worker")),
        detail: Some("item 0a1b2c3d".to_owned()),
        at: NOW,
    };
    let id = record_audit(&write, &audited).await.unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    let entries = audit_of(&read, &session("worker"), 10).await.unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, id);
    assert_eq!(entries[0].audit, audited);
    assert_eq!(
        latest_audit_on(&read, &session("worker"), AuditedAction::Ended)
            .await
            .unwrap()
            .map(|entry| entry.id),
        Some(id)
    );
}

#[tokio::test]
async fn entries_audited_before_the_ended_action_survive_the_rebuild() {
    const BEFORE_ENDS: cockpit_turso::MigrationSet = cockpit_turso::MigrationSet {
        owner: "authorization",
        migrations: cockpit_authorization::MIGRATIONS.migrations.split_at(4).0,
    };
    let path = common::database_path("ended-audit-rebuild");
    let before = Database::open(&path, &[BEFORE_ENDS]).await.unwrap();
    let write = before.write().await.unwrap();
    write
        .execute(
            "INSERT INTO authorization_audit
                (id, actor, permission, grant_id, revision, action, target, detail, at)
             VALUES (7, 'coordinator', 'start-sessions', 2, 3, 'approved', NULL,
                'approval request 3, by the owner', ?1)",
            [NOW],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(before);

    let after = Database::open(&path, &[cockpit_authorization::MIGRATIONS])
        .await
        .unwrap();
    let read = after.read().await.unwrap();

    let entry = audit_entry(&read, 7).await.unwrap().unwrap();
    assert_eq!(entry.audit.action, AuditedAction::Approved);
    assert_eq!(
        entry.audit.detail.as_deref(),
        Some("approval request 3, by the owner")
    );
    assert_eq!(entry.audit.at, NOW);
}

/// #714: a session's edit of a task item it filed is audited under its `change-state` grant,
/// with no target and the item in the detail.
#[tokio::test]
async fn an_edit_of_an_own_item_is_audited_under_change_state() {
    let database = open("edit-audited").await;
    let editor = session("worker");
    let given = grant(
        &database,
        &editor,
        Permission::ChangeState,
        &Scope::Session(editor.clone()),
    )
    .await;
    let write = database.write().await.unwrap();
    let audited = Audit {
        actor: editor.clone(),
        permission: Permission::ChangeState,
        grant: given.id,
        revision: given.granted_at,
        action: AuditedAction::Edited,
        target: None,
        detail: Some("item 36: title \"#711?\" -> \"#713\"".to_owned()),
        at: NOW,
    };
    let id = record_audit(&write, &audited).await.unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    let entries = audit_of(&read, &editor, 10).await.unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, id);
    assert_eq!(entries[0].audit, audited);
}

#[tokio::test]
async fn entries_audited_before_the_edited_action_survive_the_rebuild() {
    const BEFORE_EDITS: cockpit_turso::MigrationSet = cockpit_turso::MigrationSet {
        owner: "authorization",
        migrations: cockpit_authorization::MIGRATIONS.migrations.split_at(5).0,
    };
    let path = common::database_path("edited-audit-rebuild");
    let before = Database::open(&path, &[BEFORE_EDITS]).await.unwrap();
    let write = before.write().await.unwrap();
    write
        .execute(
            "INSERT INTO authorization_audit
                (id, actor, permission, grant_id, revision, action, target, detail, at)
             VALUES (9, 'coordinator', 'end-sessions', 2, 3, 'ended', 'worker',
                'item 0a1b2c3d', ?1)",
            [NOW],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(before);

    let after = Database::open(&path, &[cockpit_authorization::MIGRATIONS])
        .await
        .unwrap();
    let read = after.read().await.unwrap();

    let entry = audit_entry(&read, 9).await.unwrap().unwrap();
    assert_eq!(entry.audit.action, AuditedAction::Ended);
    assert_eq!(entry.audit.target, Some(session("worker")));
    assert_eq!(entry.audit.detail.as_deref(), Some("item 0a1b2c3d"));
    assert_eq!(entry.audit.at, NOW);
}
