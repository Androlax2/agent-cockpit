//! Acceptance of issue #25: prose, payload fields and plugin manifests cannot widen a grant.
//!
//! Prose has no input in the evaluation API: only the authenticated actor, a permission and a
//! scope reach `evaluate`. Payload fields reach it as the ids inside a scope, so the ids below
//! are the spellings a payload could use to stretch a grant: a case variant, a SQL wildcard,
//! and an id that reads like the owner. Plugin manifests reach it as permission names.
//!
//! The property is checked over that bounded universe rather than by random sampling: every
//! grantable grant is given and then revoked, in a fixed shuffled order. After each change the
//! request it decides is checked, and every few changes every request is, against a reference
//! model in which a session is allowed only by an active grant to itself, of that exact
//! permission, whose scope is the owner's or exactly the requested one.
mod common;

use std::str::FromStr;

use cockpit_authorization::{
    Actor, Basis, Error, Grant, Outcome, Permission, Scope, evaluate, revoke,
};
use cockpit_turso::{Database, ReadTransaction};
use common::{grant, open, session, task};

const PAYLOAD_IDS: &[&str] = &["a", "A", "%", "owner"];

fn scopes() -> Vec<Scope> {
    let mut scopes = vec![Scope::Owner];
    scopes.extend(PAYLOAD_IDS.iter().map(|id| Scope::Session(session(id))));
    scopes.extend(PAYLOAD_IDS.iter().map(|id| Scope::Task(task(id))));
    scopes
}

fn actors() -> Vec<Actor> {
    let mut actors = vec![Actor::Owner];
    actors.extend(PAYLOAD_IDS.iter().map(|id| Actor::Session(session(id))));
    actors
}

/// Every grant the owner can give in this universe; host actions are never grantable.
fn grantable() -> Vec<(cockpit_authorization::SessionId, Permission, Scope)> {
    let mut grants = Vec::new();
    for holder in PAYLOAD_IDS {
        for permission in Permission::ALL {
            if permission == Permission::HostActions {
                continue;
            }
            for scope in scopes() {
                grants.push((session(holder), permission, scope));
            }
        }
    }
    grants
}

/// The model's rule: a grant allows its holder its permission over its scope, and an owner
/// scope covers every scope.
fn model_grant_allows(grant: &Grant, actor: &Actor, permission: Permission, scope: &Scope) -> bool {
    Actor::Session(grant.holder.clone()) == *actor
        && grant.permission == permission
        && (grant.scope == Scope::Owner || grant.scope == *scope)
}

fn model_allows(active: &[Grant], actor: &Actor, permission: Permission, scope: &Scope) -> bool {
    match actor {
        Actor::Owner => true,
        Actor::Session(_) if permission == Permission::HostActions => false,
        Actor::Session(_) => active
            .iter()
            .any(|grant| model_grant_allows(grant, actor, permission, scope)),
    }
}

/// Checks one request's outcome against the model, including that an allowing grant is
/// active, held by the actor, of that permission and covering the scope.
async fn assert_request_matches_the_model(
    read: &ReadTransaction<'_>,
    active: &[Grant],
    (actor, permission, scope): (&Actor, Permission, &Scope),
    step: &str,
) {
    let outcome = evaluate(read, actor, permission, scope)
        .await
        .unwrap()
        .outcome;
    let is_allowed = match &outcome {
        Outcome::Allowed(Basis::Owner) => *actor == Actor::Owner,
        Outcome::Allowed(Basis::Grant(id)) => active
            .iter()
            .any(|grant| grant.id == *id && model_grant_allows(grant, actor, permission, scope)),
        Outcome::Refused(_) => false,
    };
    assert_eq!(
        is_allowed,
        model_allows(active, actor, permission, scope),
        "after {step}: {actor:?} {permission:?} {scope:?} -> {outcome:?}"
    );
}

async fn assert_every_request_matches_the_model(database: &Database, active: &[Grant], step: &str) {
    let read = database.read().await.unwrap();
    for actor in actors() {
        for permission in Permission::ALL {
            for scope in scopes() {
                assert_request_matches_the_model(&read, active, (&actor, permission, &scope), step)
                    .await;
            }
        }
    }
}

/// The request the grant just given or revoked decides, checked right after the change; the
/// whole universe is checked every `FULL_CHECK_EVERY` changes, which keeps the debug-build
/// test near ten seconds instead of fifty.
async fn assert_changed_grant_matches_the_model(
    database: &Database,
    active: &[Grant],
    changed: &Grant,
    step: &str,
) {
    let read = database.read().await.unwrap();
    let actor = Actor::Session(changed.holder.clone());
    assert_request_matches_the_model(
        &read,
        active,
        (&actor, changed.permission, &changed.scope),
        step,
    )
    .await;
}

const FULL_CHECK_EVERY: usize = 5;

/// The check after the change at `position` of `change_count`: the changed grant's request,
/// and every request every `FULL_CHECK_EVERY` changes and after the last one.
async fn assert_after_change(
    database: &Database,
    active: &[Grant],
    changed: &Grant,
    step: &str,
    position: usize,
    change_count: usize,
) {
    assert_changed_grant_matches_the_model(database, active, changed, step).await;
    if position.is_multiple_of(FULL_CHECK_EVERY) || position + 1 == change_count {
        assert_every_request_matches_the_model(database, active, step).await;
    }
}

/// A fixed permutation, so a failure reproduces; the order mixes holders, permissions and
/// scopes instead of granting them in table order.
fn shuffled<T>(mut items: Vec<T>, seed: u64) -> Vec<T> {
    let mut state = seed;
    for index in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(index, (state % (index as u64 + 1)) as usize);
    }
    items
}

#[tokio::test]
async fn no_sequence_of_grants_and_revocations_allows_more_than_its_active_grants() {
    let database = open("no-widening").await;
    let mut active: Vec<Grant> = Vec::new();
    assert_every_request_matches_the_model(&database, &active, "no grant").await;

    let to_grant = shuffled(grantable(), 0x9e37_79b9_7f4a_7c15);
    let grant_count = to_grant.len();
    for (position, (holder, permission, scope)) in to_grant.into_iter().enumerate() {
        let granted = grant(&database, &holder, permission, &scope).await;
        active.push(granted.clone());
        let step = format!("granting {granted:?}");
        assert_after_change(&database, &active, &granted, &step, position, grant_count).await;
    }
    let to_revoke = shuffled(active.clone(), 0x2545_f491_4f6c_dd1d);
    for (position, revoked) in to_revoke.iter().enumerate() {
        let write = database.write().await.unwrap();
        revoke(&write, revoked.id).await.unwrap();
        write.commit().await.unwrap();
        active.retain(|grant| grant.id != revoked.id);
        let step = format!("revoking {revoked:?}");
        assert_after_change(
            &database,
            &active,
            revoked,
            &step,
            position,
            to_revoke.len(),
        )
        .await;
    }
}

#[test]
fn every_permission_reads_back_from_its_own_name() {
    for permission in Permission::ALL {
        assert_eq!(
            Permission::from_str(permission.as_str()).unwrap(),
            permission
        );
    }
}

/// What a plugin manifest or a skill could write to ask for more than one exact permission.
#[test]
fn a_permission_name_that_is_not_exact_is_refused() {
    for name in [
        "",
        "*",
        "all",
        "admin",
        "owner",
        "READ-STATE",
        "Read-State",
        " read-state",
        "read-state ",
        "read-state,change-state",
        "read-state change-state",
        "read_state",
        "read-*",
        "read%",
    ] {
        let parsed = Permission::from_str(name);
        assert!(
            matches!(parsed, Err(Error::UnknownPermission { .. })),
            "{name:?}: {parsed:?}"
        );
    }
}
