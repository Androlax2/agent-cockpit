//! Login grants (#59, #858), through the crate's storage API: the owner lets one project's
//! sessions use a login account, recorded with when and through which client, until revoked.

use cockpit_sandbox::{
    LoginGrant, LoginGrantError, LoginGrantRefusal, MIGRATIONS, OwnerClient, OwnerDecision,
};
use cockpit_turso::Database;

const GRANTED_AT: i64 = 1_000;
const REVOKED_AT: i64 = 2_000;

struct Store {
    _dir: tempfile::TempDir,
    database: Database,
}

async fn store() -> Store {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(&dir.path().join("state.db"), &[MIGRATIONS])
        .await
        .unwrap();
    Store {
        _dir: dir,
        database,
    }
}

impl Store {
    async fn grant(&self, project: &str, name: &str) -> Result<LoginGrant, LoginGrantError> {
        let write = self.database.write().await.unwrap();
        let granted = cockpit_sandbox::grant_login(
            &write,
            project,
            "codex",
            name,
            Some(OwnerClient::Cli),
            GRANTED_AT,
        )
        .await?;
        write.commit().await.unwrap();
        Ok(granted)
    }

    async fn revoke(&self, grant: i64) -> Result<LoginGrant, LoginGrantError> {
        let write = self.database.write().await.unwrap();
        let revoked =
            cockpit_sandbox::revoke_login_grant(&write, grant, Some(OwnerClient::Tui), REVOKED_AT)
                .await?;
        write.commit().await.unwrap();
        Ok(revoked)
    }

    async fn grants(&self, project: Option<&str>) -> Vec<LoginGrant> {
        let read = self.database.read().await.unwrap();
        cockpit_sandbox::login_grants(&read, project).await.unwrap()
    }
}

fn refusal(result: Result<LoginGrant, LoginGrantError>) -> LoginGrantRefusal {
    match result {
        Err(LoginGrantError::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_grant_records_its_project_provider_name_time_and_client() {
    let store = store().await;

    let grant = store.grant("cockpit", "work").await.unwrap();

    assert_eq!(
        (
            grant.project.as_str(),
            grant.provider.as_str(),
            grant.account.as_str()
        ),
        ("cockpit", "codex", "work")
    );
    assert_eq!(
        grant.granted,
        OwnerDecision {
            at: GRANTED_AT,
            through: Some(OwnerClient::Cli),
        }
    );
    assert_eq!(grant.revoked, None);
}

#[tokio::test]
async fn the_active_grants_are_listed_per_project_oldest_first() {
    let store = store().await;
    let first = store.grant("cockpit", "work").await.unwrap();
    let other = store.grant("website", "work").await.unwrap();
    let second = store.grant("cockpit", "spare").await.unwrap();

    let ids = |grants: Vec<LoginGrant>| grants.iter().map(|grant| grant.id).collect::<Vec<_>>();

    assert_eq!(
        ids(store.grants(Some("cockpit")).await),
        [first.id, second.id]
    );
    assert_eq!(
        ids(store.grants(None).await),
        [first.id, other.id, second.id]
    );
}

#[tokio::test]
async fn granting_an_account_the_project_holds_already_is_refused_naming_the_grant() {
    let store = store().await;
    let grant = store.grant("cockpit", "work").await.unwrap();

    let again = refusal(store.grant("cockpit", "work").await);

    assert_eq!(again, LoginGrantRefusal::AlreadyGranted { grant: grant.id });
}

#[tokio::test]
async fn a_revoked_grant_records_when_and_through_which_client_and_is_no_longer_active() {
    let store = store().await;
    let grant = store.grant("cockpit", "work").await.unwrap();

    let revoked = store.revoke(grant.id).await.unwrap();

    assert_eq!(
        revoked.revoked,
        Some(OwnerDecision {
            at: REVOKED_AT,
            through: Some(OwnerClient::Tui),
        })
    );
    assert!(store.grants(Some("cockpit")).await.is_empty());
}

#[tokio::test]
async fn an_account_can_be_granted_again_once_revoked() {
    let store = store().await;
    let grant = store.grant("cockpit", "work").await.unwrap();
    store.revoke(grant.id).await.unwrap();

    let again = store.grant("cockpit", "work").await.unwrap();

    assert_ne!(again.id, grant.id);
}

#[tokio::test]
async fn revoking_a_grant_twice_is_refused() {
    let store = store().await;
    let grant = store.grant("cockpit", "work").await.unwrap();
    store.revoke(grant.id).await.unwrap();

    assert_eq!(
        refusal(store.revoke(grant.id).await),
        LoginGrantRefusal::AlreadyRevoked
    );
}

#[tokio::test]
async fn revoking_an_unknown_grant_is_refused() {
    let store = store().await;

    assert_eq!(
        refusal(store.revoke(7).await),
        LoginGrantRefusal::NotFound { id: 7 }
    );
}
