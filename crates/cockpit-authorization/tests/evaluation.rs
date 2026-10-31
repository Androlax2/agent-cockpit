//! Granting, evaluating and revoking on a real state database.
mod common;

use cockpit_authorization::{
    Actor, Basis, Error, Outcome, Permission, Refusal, Scope, active_grants, evaluate, revoke,
};
use cockpit_turso::Database;
use common::{grant, open, session, task};

async fn decide(
    database: &Database,
    actor: &Actor,
    permission: Permission,
    scope: &Scope,
) -> Outcome {
    let read = database.read().await.unwrap();
    evaluate(&read, actor, permission, scope)
        .await
        .unwrap()
        .outcome
}

#[tokio::test]
async fn a_session_with_a_matching_grant_is_allowed_naming_that_grant() {
    let database = open("matching-grant").await;
    let builder = session("builder");
    let grant = grant(
        &database,
        &builder,
        Permission::ChangeState,
        &Scope::Task(task("t1")),
    )
    .await;

    let outcome = decide(
        &database,
        &Actor::Session(builder),
        Permission::ChangeState,
        &Scope::Task(task("t1")),
    )
    .await;

    assert_eq!(outcome, Outcome::Allowed(Basis::Grant(grant.id)));
}

#[tokio::test]
async fn a_session_without_a_grant_is_refused_naming_the_grant_that_would_unblock_it() {
    let database = open("no-grant").await;

    let outcome = decide(
        &database,
        &Actor::Session(session("builder")),
        Permission::ReadPaneContents,
        &Scope::Session(session("reviewer")),
    )
    .await;

    assert_eq!(
        outcome,
        Outcome::Refused(Refusal::NoGrant {
            permission: Permission::ReadPaneContents,
            scope: Scope::Session(session("reviewer")),
        })
    );
}

#[tokio::test]
async fn a_task_grant_does_not_cover_another_task() {
    let database = open("other-task").await;
    let builder = session("builder");
    grant(
        &database,
        &builder,
        Permission::ChangeState,
        &Scope::Task(task("t1")),
    )
    .await;

    let outcome = decide(
        &database,
        &Actor::Session(builder),
        Permission::ChangeState,
        &Scope::Task(task("t2")),
    )
    .await;

    assert!(
        matches!(outcome, Outcome::Refused(Refusal::NoGrant { .. })),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_grant_of_one_permission_does_not_cover_another() {
    let database = open("other-permission").await;
    let builder = session("builder");
    grant(
        &database,
        &builder,
        Permission::ReadState,
        &Scope::Task(task("t1")),
    )
    .await;

    let outcome = decide(
        &database,
        &Actor::Session(builder),
        Permission::ChangeState,
        &Scope::Task(task("t1")),
    )
    .await;

    assert!(
        matches!(outcome, Outcome::Refused(Refusal::NoGrant { .. })),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn an_owner_scope_grant_covers_every_session_and_task() {
    let database = open("owner-scope").await;
    let chief = session("chief");
    let grant = grant(&database, &chief, Permission::ReadState, &Scope::Owner).await;

    for scope in [
        Scope::Owner,
        Scope::Session(session("builder")),
        Scope::Task(task("t9")),
    ] {
        let outcome = decide(
            &database,
            &Actor::Session(chief.clone()),
            Permission::ReadState,
            &scope,
        )
        .await;
        assert_eq!(
            outcome,
            Outcome::Allowed(Basis::Grant(grant.id)),
            "{scope:?}"
        );
    }
}

#[tokio::test]
async fn the_owner_is_allowed_without_any_grant() {
    let database = open("owner").await;

    for permission in Permission::ALL {
        let outcome = decide(&database, &Actor::Owner, permission, &Scope::Owner).await;
        assert_eq!(outcome, Outcome::Allowed(Basis::Owner), "{permission:?}");
    }
}

#[tokio::test]
async fn host_actions_cannot_be_granted_to_a_session() {
    let database = open("host-actions-grant").await;
    let write = database.write().await.unwrap();

    let result = cockpit_authorization::grant(
        &write,
        &session("builder"),
        Permission::HostActions,
        &Scope::Owner,
    )
    .await;

    assert!(
        matches!(
            result,
            Err(Error::OwnerOnly {
                permission: Permission::HostActions
            })
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_session_asking_for_host_actions_is_refused_as_owner_only() {
    let database = open("host-actions-ask").await;

    let outcome = decide(
        &database,
        &Actor::Session(session("builder")),
        Permission::HostActions,
        &Scope::Session(session("builder")),
    )
    .await;

    assert_eq!(
        outcome,
        Outcome::Refused(Refusal::OwnerOnly {
            permission: Permission::HostActions
        })
    );
}

#[tokio::test]
async fn granting_an_active_grant_again_returns_it_without_a_new_revision() {
    let database = open("duplicate-grant").await;
    let builder = session("builder");
    let scope = Scope::Task(task("t1"));
    let first = grant(&database, &builder, Permission::ReadState, &scope).await;

    let second = grant(&database, &builder, Permission::ReadState, &scope).await;

    assert_eq!(second, first);
}

#[tokio::test]
async fn a_revoked_grant_refuses_the_next_evaluation_at_a_later_revision() {
    let database = open("revoke").await;
    let builder = session("builder");
    let scope = Scope::Task(task("t1"));
    let grant = grant(&database, &builder, Permission::ChangeState, &scope).await;
    let before = {
        let read = database.read().await.unwrap();
        evaluate(
            &read,
            &Actor::Session(builder.clone()),
            Permission::ChangeState,
            &scope,
        )
        .await
        .unwrap()
    };

    let write = database.write().await.unwrap();
    revoke(&write, grant.id).await.unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();
    let after = evaluate(
        &read,
        &Actor::Session(builder),
        Permission::ChangeState,
        &scope,
    )
    .await
    .unwrap();

    assert!(
        matches!(after.outcome, Outcome::Refused(Refusal::NoGrant { .. })),
        "{after:?}"
    );
    assert!(
        after.revision > before.revision,
        "{before:?} then {after:?}"
    );
}

#[tokio::test]
async fn every_decision_carries_the_revision_of_the_grants_it_read() {
    let database = open("decision-revision").await;
    let builder = session("builder");
    let grant = grant(&database, &builder, Permission::ReadState, &Scope::Owner).await;

    let read = database.read().await.unwrap();
    let decision = evaluate(
        &read,
        &Actor::Session(builder),
        Permission::ReadState,
        &Scope::Owner,
    )
    .await
    .unwrap();

    assert_eq!(decision.revision, grant.granted_at);
}

#[tokio::test]
async fn revoking_a_grant_twice_fails() {
    let database = open("revoke-twice").await;
    let grant = grant(
        &database,
        &session("builder"),
        Permission::ReadState,
        &Scope::Owner,
    )
    .await;
    let write = database.write().await.unwrap();
    revoke(&write, grant.id).await.unwrap();

    let second = revoke(&write, grant.id).await;

    assert!(
        matches!(second, Err(Error::GrantNotActive { grant: refused }) if refused == grant.id),
        "{second:?}"
    );
}

#[tokio::test]
async fn active_grants_lists_only_the_holders_unrevoked_grants() {
    let database = open("active-grants").await;
    let builder = session("builder");
    let kept = grant(&database, &builder, Permission::ReadState, &Scope::Owner).await;
    let revoked = grant(&database, &builder, Permission::ChangeState, &Scope::Owner).await;
    grant(
        &database,
        &session("reviewer"),
        Permission::ReadState,
        &Scope::Owner,
    )
    .await;
    let write = database.write().await.unwrap();
    revoke(&write, revoked.id).await.unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    let grants = active_grants(&read, &builder).await.unwrap();

    assert_eq!(grants, [kept]);
}

#[tokio::test]
async fn grants_and_revocations_survive_a_restart() {
    let path = common::database_path("restart");
    let builder = session("builder");
    let (kept, revoked) = {
        let database = Database::open(&path, &[cockpit_authorization::MIGRATIONS])
            .await
            .unwrap();
        let kept = grant(&database, &builder, Permission::ReadState, &Scope::Owner).await;
        let revoked = grant(&database, &builder, Permission::ChangeState, &Scope::Owner).await;
        let write = database.write().await.unwrap();
        revoke(&write, revoked.id).await.unwrap();
        write.commit().await.unwrap();
        (kept, revoked)
    };

    let database = Database::open(&path, &[cockpit_authorization::MIGRATIONS])
        .await
        .unwrap();

    let actor = Actor::Session(builder);
    assert_eq!(
        decide(&database, &actor, Permission::ReadState, &Scope::Owner).await,
        Outcome::Allowed(Basis::Grant(kept.id))
    );
    assert!(matches!(
        decide(&database, &actor, revoked.permission, &revoked.scope).await,
        Outcome::Refused(Refusal::NoGrant { .. })
    ));
}

#[test]
fn an_empty_id_is_refused() {
    assert!(matches!(
        cockpit_authorization::SessionId::new(""),
        Err(Error::EmptyId)
    ));
    assert!(matches!(
        cockpit_authorization::TaskId::new(""),
        Err(Error::EmptyId)
    ));
}
