//! Issue #1131: a coordinator grant covers one project, or everything. A request over another
//! project, or over a directory in none, is refused naming the scope the owner would grant;
//! grants stored before the project scope keep covering everything.
mod common;

use cockpit_authorization::{
    Actor, Basis, Error, Outcome, Permission, ProjectName, Refusal, Scope, StartAllowance,
    StartTerms, active_grants, evaluate, grant_coordinator, grant_start_sessions,
    revoke_permission, start_allowance,
};
use cockpit_turso::Database;
use common::{open, session};

const NOW: i64 = 1_800_000_000;

fn project(name: &str) -> Scope {
    Scope::Project(ProjectName::new(name).unwrap())
}

async fn give(database: &Database, permission: Permission, scope: &Scope) {
    let write = database.write().await.unwrap();
    grant_coordinator(&write, &session("coordinator"), permission, scope)
        .await
        .unwrap();
    write.commit().await.unwrap();
}

async fn outcome(database: &Database, permission: Permission, scope: &Scope) -> Outcome {
    let read = database.read().await.unwrap();
    evaluate(
        &read,
        &Actor::Session(session("coordinator")),
        permission,
        scope,
    )
    .await
    .unwrap()
    .outcome
}

#[tokio::test]
async fn a_project_grant_allows_its_own_project() {
    let database = open("project-own").await;
    give(&database, Permission::MessageSessions, &project("alpha")).await;

    let allowed = outcome(&database, Permission::MessageSessions, &project("alpha")).await;

    assert!(
        matches!(allowed, Outcome::Allowed(Basis::Grant(_))),
        "{allowed:?}"
    );
}

#[tokio::test]
async fn a_project_grant_refuses_another_project_naming_it() {
    let database = open("project-other").await;
    give(&database, Permission::MessageSessions, &project("alpha")).await;

    let refused = outcome(&database, Permission::MessageSessions, &project("beta")).await;

    assert_eq!(
        refused,
        Outcome::Refused(Refusal::NoGrant {
            permission: Permission::MessageSessions,
            scope: project("beta"),
        })
    );
}

#[tokio::test]
async fn a_project_grant_covers_no_session_and_not_everything() {
    let database = open("project-narrow").await;
    give(&database, Permission::ObserveSessions, &project("alpha")).await;

    for scope in [Scope::Owner, Scope::Session(session("worker"))] {
        let refused = outcome(&database, Permission::ObserveSessions, &scope).await;
        assert!(
            matches!(refused, Outcome::Refused(_)),
            "{scope:?}: {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_grant_over_everything_covers_every_project() {
    let database = open("project-everything").await;
    give(&database, Permission::EndSessions, &Scope::Owner).await;

    for name in ["alpha", "beta"] {
        let allowed = outcome(&database, Permission::EndSessions, &project(name)).await;
        assert!(
            matches!(allowed, Outcome::Allowed(_)),
            "{name}: {allowed:?}"
        );
    }
}

#[tokio::test]
async fn granting_over_another_scope_replaces_the_grant() {
    let database = open("project-regrant").await;
    give(&database, Permission::ObserveSessions, &Scope::Owner).await;
    give(&database, Permission::ObserveSessions, &project("alpha")).await;
    let read = database.read().await.unwrap();

    let held = active_grants(&read, &session("coordinator")).await.unwrap();

    let scopes: Vec<&Scope> = held.iter().map(|grant| &grant.scope).collect();
    assert_eq!(scopes, [&project("alpha")]);
}

#[tokio::test]
async fn granting_the_same_scope_again_keeps_the_grant() {
    let database = open("project-same").await;
    give(&database, Permission::ObserveSessions, &project("alpha")).await;
    let first = {
        let read = database.read().await.unwrap();
        active_grants(&read, &session("coordinator")).await.unwrap()
    };
    give(&database, Permission::ObserveSessions, &project("alpha")).await;
    let read = database.read().await.unwrap();

    let second = active_grants(&read, &session("coordinator")).await.unwrap();

    assert_eq!(first, second);
}

#[tokio::test]
async fn revoking_a_permission_takes_back_its_grant_over_any_scope() {
    let database = open("project-revoke").await;
    give(&database, Permission::MessageSessions, &project("alpha")).await;
    let write = database.write().await.unwrap();

    let revoked = revoke_permission(&write, &session("coordinator"), Permission::MessageSessions)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(revoked.len(), 1);
    let refused = outcome(&database, Permission::MessageSessions, &project("alpha")).await;
    assert!(matches!(refused, Outcome::Refused(_)), "{refused:?}");
}

#[tokio::test]
async fn start_sessions_is_given_with_its_terms_only() {
    let database = open("project-start-terms").await;
    let write = database.write().await.unwrap();

    let refused = grant_coordinator(
        &write,
        &session("coordinator"),
        Permission::StartSessions,
        &project("alpha"),
    )
    .await;

    assert!(
        matches!(refused, Err(Error::NotCoordinator { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_start_in_another_project_is_refused_naming_it() {
    let database = open("project-start").await;
    let write = database.write().await.unwrap();
    grant_start_sessions(
        &write,
        &session("coordinator"),
        &project("alpha"),
        StartTerms::DEFAULT,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();

    let refused = start_allowance(&read, &session("coordinator"), &project("beta"), NOW, 0)
        .await
        .unwrap();
    let allowed = start_allowance(&read, &session("coordinator"), &project("alpha"), NOW, 0)
        .await
        .unwrap();

    assert_eq!(
        refused,
        StartAllowance::Refused(Refusal::NoGrant {
            permission: Permission::StartSessions,
            scope: project("beta"),
        })
    );
    assert!(
        matches!(allowed, StartAllowance::Allowed { .. }),
        "{allowed:?}"
    );
}

/// The migration that widens the scopes copies every grant: one stored before it, over
/// everything, still covers every project, with its start terms.
#[tokio::test]
async fn grants_stored_before_project_scopes_keep_covering_everything() {
    const BEFORE_PROJECTS: cockpit_turso::MigrationSet = cockpit_turso::MigrationSet {
        owner: "authorization",
        migrations: cockpit_authorization::MIGRATIONS.migrations.split_at(6).0,
    };
    let path = common::database_path("project-migration");
    let before = Database::open(&path, &[BEFORE_PROJECTS]).await.unwrap();
    let write = before.write().await.unwrap();
    write
        .execute(
            "INSERT INTO authorization_grants
                (id, holder, permission, scope_kind, scope_id, granted_at, starts_per_hour,
                 starts_unconfirmed, starts_max_active)
             VALUES (7, 'coordinator', 'start-sessions', 'owner', NULL, 1, 10, 1, 3),
                    (8, 'coordinator', 'message-sessions', 'owner', NULL, 2, NULL, 0, NULL)",
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
    let kept: Vec<(i64, &Scope, Option<StartTerms>)> = held
        .iter()
        .map(|grant| (grant.id.value(), &grant.scope, grant.start_terms))
        .collect();
    assert_eq!(
        kept,
        [
            (
                7,
                &Scope::Owner,
                Some(StartTerms {
                    per_hour: 10,
                    max_active: 3,
                    needs_confirmation: false,
                })
            ),
            (8, &Scope::Owner, None),
        ]
    );
    drop(read);
    let allowed = outcome(&after, Permission::MessageSessions, &project("beta")).await;
    assert!(matches!(allowed, Outcome::Allowed(_)), "{allowed:?}");
}

#[tokio::test]
async fn a_project_grant_reads_back_with_its_project() {
    let database = open("project-read-back").await;
    give(&database, Permission::EndSessions, &project("alpha")).await;
    let read = database.read().await.unwrap();

    let held = active_grants(&read, &session("coordinator")).await.unwrap();

    assert_eq!(held[0].scope, project("alpha"));
}
