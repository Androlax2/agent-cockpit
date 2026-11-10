//! Network requests and the owner's decisions on them (#674), through the crate's storage API.

use cockpit_sandbox::{
    AllowEntry, AllowSource, AskedDestination, AskingDestination, DeniedDestination,
    EgressDecisions, GITHUB_HOST, MIGRATIONS, NETWORK_ALLOW_GRACE_SECONDS, NetworkError,
    NetworkGrant, NetworkRefusal, NetworkRequestState, OwnerClient, OwnerDecision, Subject, Target,
};
use cockpit_turso::Database;

const ASKED_AT: i64 = 1_000;
const DECIDED_AT: i64 = ASKED_AT + NETWORK_ALLOW_GRACE_SECONDS;

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

fn session(id: &str) -> Subject {
    Subject::Session {
        session: id.to_owned(),
    }
}

fn example() -> AskedDestination {
    AskedDestination {
        host: "unknown.example.com".to_owned(),
        port: 443,
    }
}

impl Store {
    async fn file(&self, subject: &str, project: &str) -> Option<i64> {
        let write = self.database.write().await.unwrap();
        let filed = cockpit_sandbox::file_network_request(
            &write,
            &session(subject),
            project,
            &example(),
            ASKED_AT,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        filed.map(|request| request.id)
    }

    async fn allow(&self, request: i64, always: bool, at: i64) -> Result<(), NetworkError> {
        let write = self.database.write().await.unwrap();
        cockpit_sandbox::allow_network_request(&write, request, always, Some(OwnerClient::Cli), at)
            .await?;
        write.commit().await.unwrap();
        Ok(())
    }

    async fn grant_github(&self, project: &str) -> Result<NetworkGrant, NetworkError> {
        let write = self.database.write().await.unwrap();
        let granted =
            cockpit_sandbox::grant_github(&write, project, Some(OwnerClient::Cli), DECIDED_AT)
                .await?;
        write.commit().await.unwrap();
        Ok(granted)
    }

    async fn deny(&self, request: i64) {
        let write = self.database.write().await.unwrap();
        cockpit_sandbox::deny_network_request(&write, request, Some(OwnerClient::Tui), DECIDED_AT)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    async fn decisions(&self, subject: &str, project: &str) -> EgressDecisions {
        let read = self.database.read().await.unwrap();
        cockpit_sandbox::egress_decisions(&read, project, &session(subject))
            .await
            .unwrap()
    }
}

fn asked_example(request: i64) -> AskingDestination {
    AskingDestination {
        host: "unknown.example.com".to_owned(),
        port: 443,
        request,
    }
}

fn entry(source: AllowSource) -> AllowEntry {
    AllowEntry {
        target: Target::Name("unknown.example.com".to_owned()),
        port: 443,
        source,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_destination_is_filed_once_and_allows_nothing_until_decided() {
    let store = store().await;

    let first = store.file("s1", "app").await;
    let again = store.file("s1", "app").await;

    assert!(first.is_some());
    assert_eq!(again, None);
    let read = store.database.read().await.unwrap();
    let asking = cockpit_sandbox::asking_network_requests(&read)
        .await
        .unwrap();
    assert_eq!(asking.len(), 1);
    assert_eq!(asking[0].state, NetworkRequestState::Asking);
    assert_eq!(asking[0].decision, None);
    drop(read);
    assert_eq!(
        store.decisions("s1", "app").await,
        EgressDecisions {
            asking: vec![asked_example(first.unwrap())],
            ..EgressDecisions::default()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_once_lets_only_the_asking_session_through_for_the_rest_of_it() {
    let store = store().await;
    let request = store.file("s1", "app").await.unwrap();

    store.allow(request, false, DECIDED_AT).await.unwrap();

    assert_eq!(
        store.decisions("s1", "app").await.allowed,
        vec![entry(AllowSource::Request { request })]
    );
    assert_eq!(
        store.decisions("s2", "app").await,
        EgressDecisions::default()
    );
    let read = store.database.read().await.unwrap();
    let decided = cockpit_sandbox::network_request(&read, request)
        .await
        .unwrap();
    assert_eq!(decided.state, NetworkRequestState::AllowedOnce);
    assert_eq!(
        decided.decision,
        Some(OwnerDecision {
            at: DECIDED_AT,
            through: Some(OwnerClient::Cli),
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_always_grants_the_project_and_no_other() {
    let store = store().await;
    let request = store.file("s1", "app").await.unwrap();

    store.allow(request, true, DECIDED_AT).await.unwrap();

    let read = store.database.read().await.unwrap();
    let grants = cockpit_sandbox::network_grants(&read, Some("app"))
        .await
        .unwrap();
    drop(read);
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].request, Some(request));
    let granted = entry(AllowSource::Grant {
        grant: grants[0].id.to_string(),
    });
    assert_eq!(store.decisions("s2", "app").await.allowed, vec![granted]);
    assert_eq!(
        store.decisions("s3", "other").await,
        EgressDecisions::default()
    );
    assert_eq!(store.file("s2", "app").await, None, "the grant covers it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_grant_allows_nothing_and_the_destination_is_asked_about_again() {
    let store = store().await;
    let request = store.file("s1", "app").await.unwrap();
    store.allow(request, true, DECIDED_AT).await.unwrap();
    let read = store.database.read().await.unwrap();
    let grant = cockpit_sandbox::network_grants(&read, None).await.unwrap()[0].id;
    drop(read);

    let write = store.database.write().await.unwrap();
    let revoked =
        cockpit_sandbox::revoke_network_grant(&write, grant, Some(OwnerClient::Tui), DECIDED_AT)
            .await
            .unwrap();
    write.commit().await.unwrap();

    assert_eq!(
        revoked.revoked,
        Some(OwnerDecision {
            at: DECIDED_AT,
            through: Some(OwnerClient::Tui),
        })
    );
    assert_eq!(
        store.decisions("s1", "app").await,
        EgressDecisions::default()
    );
    assert!(store.file("s1", "app").await.is_some());
    let write = store.database.write().await.unwrap();
    let again = cockpit_sandbox::revoke_network_grant(&write, grant, None, DECIDED_AT).await;
    assert!(matches!(
        again,
        Err(NetworkError::Refused(NetworkRefusal::AlreadyRevoked))
    ));
}

/// #966: the launcher names the request in its refusal, so the agent can tell the owner.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_waiting_on_the_owner_reaches_the_launcher_by_its_number() {
    let store = store().await;

    let request = store.file("s1", "app").await.unwrap();

    assert_eq!(
        store.decisions("s1", "app").await.asking,
        vec![asked_example(request)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_destination_stays_refused_and_is_not_asked_about_again() {
    let store = store().await;
    let request = store.file("s1", "app").await.unwrap();

    store.deny(request).await;

    assert_eq!(
        store.decisions("s1", "app").await,
        EgressDecisions {
            allowed: Vec::new(),
            denied: vec![DeniedDestination {
                host: "unknown.example.com".to_owned(),
                port: 443,
                request,
            }],
            asking: Vec::new(),
        }
    );
    assert_eq!(store.file("s1", "app").await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_is_taken_once() {
    let store = store().await;
    let request = store.file("s1", "app").await.unwrap();
    store.deny(request).await;

    let allowed = store.allow(request, true, DECIDED_AT).await;

    assert!(matches!(
        allowed,
        Err(NetworkError::Refused(NetworkRefusal::AlreadyDecided {
            state: "denied"
        }))
    ));
}

/// #156: a keystroke meant for what the queue showed before does not allow a host.
#[tokio::test(flavor = "multi_thread")]
async fn an_allow_within_the_grace_after_the_request_appeared_is_refused() {
    let store = store().await;
    let request = store.file("s1", "app").await.unwrap();

    let allowed = store.allow(request, false, ASKED_AT + 1).await;

    assert!(matches!(
        allowed,
        Err(NetworkError::Refused(NetworkRefusal::TooSoon {
            wait_seconds: 1
        }))
    ));
    assert_eq!(
        store.decisions("s1", "app").await,
        EgressDecisions {
            asking: vec![asked_example(request)],
            ..EgressDecisions::default()
        }
    );
}

/// Two sessions asked before either was decided: the second "always" joins the first's grant.
#[tokio::test(flavor = "multi_thread")]
async fn allow_always_of_a_host_the_project_was_granted_since_joins_that_grant() {
    let store = store().await;
    let first = store.file("s1", "app").await.unwrap();
    let second = store.file("s2", "app").await.unwrap();
    store.allow(first, true, DECIDED_AT).await.unwrap();

    store.allow(second, true, DECIDED_AT).await.unwrap();

    let read = store.database.read().await.unwrap();
    let grants = cockpit_sandbox::network_grants(&read, Some("app"))
        .await
        .unwrap();
    let joined = cockpit_sandbox::network_request(&read, second)
        .await
        .unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(joined.state, NetworkRequestState::AllowedAlways);
    assert_eq!(joined.grant, Some(grants[0].id));
}

/// #1256: the owner grants a project github.com without a request; its sessions are let
/// through and file no request for it, another project's are not.
#[tokio::test(flavor = "multi_thread")]
async fn a_github_grant_without_a_request_allows_the_projects_sessions_only() {
    let store = store().await;

    let granted = store.grant_github("app").await.unwrap();

    assert_eq!(granted.project, "app");
    assert_eq!((granted.host.as_str(), granted.port), (GITHUB_HOST, 443));
    assert_eq!(granted.request, None);
    assert_eq!(
        granted.granted,
        OwnerDecision {
            at: DECIDED_AT,
            through: Some(OwnerClient::Cli),
        }
    );
    let github = AllowEntry {
        target: Target::Name(GITHUB_HOST.to_owned()),
        port: 443,
        source: AllowSource::Grant {
            grant: granted.id.to_string(),
        },
    };
    assert_eq!(store.decisions("s1", "app").await.allowed, vec![github]);
    assert_eq!(
        store.decisions("s2", "other").await,
        EgressDecisions::default()
    );
    let write = store.database.write().await.unwrap();
    let asked = AskedDestination {
        host: GITHUB_HOST.to_owned(),
        port: 443,
    };
    let filed = cockpit_sandbox::file_network_request(&write, &session("s1"), "app", &asked, 1)
        .await
        .unwrap();
    assert_eq!(filed, None, "the grant covers it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_github_grant_of_the_project_is_refused_naming_the_first() {
    let store = store().await;
    let first = store.grant_github("app").await.unwrap();

    let again = store.grant_github("app").await;

    assert!(
        matches!(
            &again,
            Err(NetworkError::Refused(NetworkRefusal::AlreadyGranted { grant })) if *grant == first.id
        ),
        "{again:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_github_grant_can_be_granted_again() {
    let store = store().await;
    let first = store.grant_github("app").await.unwrap();
    let write = store.database.write().await.unwrap();
    cockpit_sandbox::revoke_network_grant(&write, first.id, None, DECIDED_AT)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let again = store.grant_github("app").await.unwrap();

    assert_ne!(again.id, first.id);
    let read = store.database.read().await.unwrap();
    let active = cockpit_sandbox::network_grants(&read, Some("app"))
        .await
        .unwrap();
    assert_eq!(active, vec![again]);
}

/// #1256: the migration that lets a grant have no request keeps the grants made before it,
/// each still naming its request.
#[tokio::test(flavor = "multi_thread")]
async fn grants_made_before_direct_grants_keep_their_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let before_direct_grants = cockpit_turso::MigrationSet {
        owner: MIGRATIONS.owner,
        migrations: &MIGRATIONS.migrations[..3],
    };
    let database = Database::open(&path, &[before_direct_grants])
        .await
        .unwrap();
    let old = Store {
        _dir: tempfile::tempdir().unwrap(),
        database,
    };
    let request = old.file("s1", "app").await.unwrap();
    old.allow(request, true, DECIDED_AT).await.unwrap();
    drop(old);

    let database = Database::open(&path, &[MIGRATIONS]).await.unwrap();

    let read = database.read().await.unwrap();
    let grants = cockpit_sandbox::network_grants(&read, None).await.unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].request, Some(request));
    let decided = cockpit_sandbox::network_request(&read, request)
        .await
        .unwrap();
    assert_eq!(decided.grant, Some(grants[0].id));
}
