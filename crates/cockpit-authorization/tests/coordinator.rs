//! Issue #410: the coordinator grants (`observe-sessions`, `message-sessions`,
//! `start-sessions`), the start terms and their hourly cap, and the audit of their use, on a
//! real state database.
mod common;

use cockpit_authorization::{
    Actor, Audit, AuditedAction, Basis, Error, Outcome, Permission, Refusal, Scope, StartAllowance,
    StartTerms, active_coordinator_grants, active_grants, audit_of, evaluate, grant_start_sessions,
    latest_audit_on, latest_audits_by_target, record_audit, revoke, start_allowance, stored_grant,
};
use cockpit_turso::Database;
use common::{grant, open, session};

const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

async fn allowance(database: &Database, holder: &str, now: i64) -> StartAllowance {
    let read = database.read().await.unwrap();
    start_allowance(&read, &session(holder), now).await.unwrap()
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

#[tokio::test]
async fn starts_beyond_the_hourly_cap_are_refused_until_the_oldest_leaves_the_hour() {
    let database = open("start-cap").await;
    give_start_terms(
        &database,
        "coordinator",
        StartTerms {
            per_hour: 2,
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
            needs_confirmation: true,
        },
    )
    .await;

    assert!(matches!(refused, Err(Error::InvalidStartTerms { .. })));
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
async fn only_coordinator_grants_are_audited() {
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
