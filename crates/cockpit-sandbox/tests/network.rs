//! Network requests and the owner's decisions on them (#674), through the crate's storage API.

use cockpit_sandbox::{
    AllowEntry, AllowSource, AskedDestination, AskingDestination, DeniedDestination,
    EgressDecisions, GITHUB_HOST, MIGRATIONS, NETWORK_ALLOW_GRACE_SECONDS, NetworkError,
    NetworkGrant, NetworkRefusal, NetworkRequest, NetworkRequestState, NetworkTarget, OwnerClient,
    OwnerDecision, ServiceAsk, Subject, Target,
};
use cockpit_sandbox::{AllowedService, AskedService, DecidedService, Endpoint, Forward};
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
            services: Default::default(),
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
            services: Default::default(),
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
            services: Default::default(),
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
    assert_eq!(
        granted.target,
        NetworkTarget::Host {
            host: GITHUB_HOST.to_owned(),
            port: 443
        }
    );
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

/// A database at the first `versions` of the sandbox migrations, holding a host request of
/// `s1` in `app` its owner allowed always: request 1, grant 1, as those versions wrote them.
async fn allowed_always_before(path: &std::path::Path, versions: usize) {
    let earlier = cockpit_turso::MigrationSet {
        owner: MIGRATIONS.owner,
        migrations: &MIGRATIONS.migrations[..versions],
    };
    let database = Database::open(path, &[earlier]).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO sandbox_network_grants
                (id, project, host, port, request, granted_at, granted_through)
             VALUES (1, 'app', 'unknown.example.com', 443, 1, ?1, 'cli')",
            [DECIDED_AT],
        )
        .await
        .unwrap();
    write
        .execute(
            "INSERT INTO sandbox_network_requests
                (id, subject_kind, subject, project, host, port, state, asked_at, decided_at,
                 decided_through, grant_id)
             VALUES (1, 'session', 's1', 'app', 'unknown.example.com', 443, 'allowed-always',
                ?1, ?2, 'cli', 1)",
            [ASKED_AT, DECIDED_AT],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
}

/// #1256: the migration that lets a grant have no request keeps the grants made before it,
/// each still naming its request.
#[tokio::test(flavor = "multi_thread")]
async fn grants_made_before_direct_grants_keep_their_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    allowed_always_before(&path, 3).await;

    let database = Database::open(&path, &[MIGRATIONS]).await.unwrap();

    let read = database.read().await.unwrap();
    let grants = cockpit_sandbox::network_grants(&read, None).await.unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].request, Some(1));
    let decided = cockpit_sandbox::network_request(&read, 1).await.unwrap();
    assert_eq!(decided.grant, Some(grants[0].id));
}

/// #880: the migration that adds service targets keeps every earlier request and grant as a
/// host, still allowing its sessions.
#[tokio::test(flavor = "multi_thread")]
async fn requests_and_grants_made_before_services_stay_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    allowed_always_before(&path, 4).await;

    let database = Database::open(&path, &[MIGRATIONS]).await.unwrap();

    let read = database.read().await.unwrap();
    let host = NetworkTarget::Host {
        host: "unknown.example.com".to_owned(),
        port: 443,
    };
    let grants = cockpit_sandbox::network_grants(&read, None).await.unwrap();
    assert_eq!(grants[0].target, host);
    let decided = cockpit_sandbox::network_request(&read, 1).await.unwrap();
    assert_eq!(decided.target, host);
    let decisions = cockpit_sandbox::egress_decisions(&read, "app", &session("s1"))
        .await
        .unwrap();
    assert_eq!(
        decisions.allowed,
        [entry(AllowSource::Grant {
            grant: "1".to_owned()
        })]
    );
}

/// The `figcraft` service, served at port 3845 in the sandbox, relayed to `target`.
fn figcraft_to(target: Endpoint) -> Forward {
    Forward {
        service: "figcraft".to_owned(),
        listen: Endpoint::Port { port: 3845 },
        target,
    }
}

fn figcraft_port() -> Forward {
    figcraft_to(Endpoint::Port { port: 3845 })
}

impl Store {
    async fn file_service(
        &self,
        subject: &str,
        project: &str,
        asked: &AskedService,
    ) -> Option<i64> {
        let write = self.database.write().await.unwrap();
        let filed = cockpit_sandbox::file_service_request(
            &write,
            &session(subject),
            project,
            asked,
            ASKED_AT,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        filed.map(|request| request.id)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_service_is_filed_once_and_waits_on_the_owner_by_its_number() {
    let store = store().await;
    let asked = AskedService::of(&figcraft_port());

    let first = store.file_service("s1", "app", &asked).await.unwrap();
    let again = store.file_service("s1", "app", &asked).await;

    assert_eq!(again, None);
    let decisions = store.decisions("s1", "app").await;
    assert_eq!(
        decisions.services.asking,
        [DecidedService {
            service: asked.service,
            record: asked.record,
            request: first,
        }]
    );
    assert!(decisions.services.allowed.is_empty());
    assert!(decisions.asking.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_and_a_service_of_one_session_are_two_requests() {
    let store = store().await;

    let host = store.file("s1", "app").await.unwrap();
    let service = store
        .file_service("s1", "app", &AskedService::of(&figcraft_port()))
        .await
        .unwrap();

    assert_ne!(host, service);
    let read = store.database.read().await.unwrap();
    let asking = cockpit_sandbox::asking_network_requests(&read)
        .await
        .unwrap();
    let targets: Vec<NetworkTarget> = asking.into_iter().map(|request| request.target).collect();
    assert_eq!(
        targets,
        [
            NetworkTarget::Host {
                host: "unknown.example.com".to_owned(),
                port: 443
            },
            NetworkTarget::Service {
                service: "figcraft".to_owned(),
                record: AskedService::of(&figcraft_port()).record
            }
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_always_grants_the_projects_sessions_that_one_service_record() {
    let store = store().await;
    let asked = AskedService::of(&figcraft_port());
    let request = store.file_service("s1", "app", &asked).await.unwrap();

    store.allow(request, true, DECIDED_AT).await.unwrap();

    let read = store.database.read().await.unwrap();
    let grant = cockpit_sandbox::network_grants(&read, None).await.unwrap()[0].id;
    drop(read);
    let allowed = AllowedService {
        service: asked.service.clone(),
        record: asked.record.clone(),
        source: AllowSource::Grant {
            grant: grant.to_string(),
        },
        live: None,
    };
    assert_eq!(
        store.decisions("s2", "app").await.services.allowed,
        [allowed]
    );
    assert!(
        store
            .decisions("s3", "other")
            .await
            .services
            .allowed
            .is_empty()
    );
    assert!(store.decisions("s2", "app").await.allowed.is_empty());
    assert_eq!(store.file_service("s2", "app", &asked).await, None);
}

/// #390: a container port the owner allowed, always or for the session, opens live: its
/// launcher reads the forward back from the record.
#[tokio::test(flavor = "multi_thread")]
async fn an_allowed_container_port_opens_live() {
    for is_always in [true, false] {
        let store = store().await;
        let forward = Forward::container_port("torlek-e3", 8080);
        let request = store
            .file_service("s1", "app", &AskedService::of(&forward))
            .await
            .unwrap();

        store.allow(request, is_always, DECIDED_AT).await.unwrap();

        let services = store.decisions("s1", "app").await.services;
        let live: Vec<&Forward> = services.live_forwards().collect();
        assert_eq!(live, [&forward], "always: {is_always}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_target_is_another_record_the_grant_does_not_cover() {
    let store = store().await;
    let request = store
        .file_service("s1", "app", &AskedService::of(&figcraft_port()))
        .await
        .unwrap();
    store.allow(request, true, DECIDED_AT).await.unwrap();

    let changed = figcraft_to(Endpoint::Socket {
        path: "/run/figcraft.sock".into(),
    });
    let asked = store
        .file_service("s2", "app", &AskedService::of(&changed))
        .await;

    assert!(asked.is_some());
    let decisions = store.decisions("s2", "app").await;
    assert_eq!(decisions.services.allowing(&changed), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_once_and_deny_of_a_service_reach_only_the_asking_session() {
    let store = store().await;
    let allowed = store
        .file_service("s1", "app", &AskedService::of(&figcraft_port()))
        .await
        .unwrap();
    let denied = store
        .file_service("s2", "app", &AskedService::of(&figcraft_port()))
        .await
        .unwrap();

    store.allow(allowed, false, DECIDED_AT).await.unwrap();
    store.deny(denied).await;

    let forward = figcraft_port();
    let first = store.decisions("s1", "app").await.services;
    let second = store.decisions("s2", "app").await.services;
    assert!(first.allowing(&forward).is_some());
    assert_eq!(second.allowing(&forward), None);
    assert_eq!(second.denial(&forward), Some(denied));
    assert!(store.decisions("s3", "app").await.services == Default::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_service_grant_allows_nothing_and_the_record_is_asked_about_again() {
    let store = store().await;
    let asked = AskedService::of(&figcraft_port());
    let request = store.file_service("s1", "app", &asked).await.unwrap();
    store.allow(request, true, DECIDED_AT).await.unwrap();
    let read = store.database.read().await.unwrap();
    let grant = cockpit_sandbox::network_grants(&read, None).await.unwrap()[0].id;
    drop(read);

    let write = store.database.write().await.unwrap();
    cockpit_sandbox::revoke_network_grant(&write, grant, None, DECIDED_AT)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert!(
        store
            .decisions("s1", "app")
            .await
            .services
            .allowed
            .is_empty()
    );
    assert!(store.file_service("s2", "app", &asked).await.is_some());
}

const REASON: &str = "read the design file the task names";

impl Store {
    /// `subject`'s agent asks ahead for the `figcraft` service at `forward`.
    async fn ask_service(&self, subject: &str, project: &str, forward: &Forward) -> ServiceAsk {
        let write = self.database.write().await.unwrap();
        let asked = cockpit_sandbox::ask_service(
            &write,
            &session(subject),
            project,
            forward,
            REASON,
            ASKED_AT,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        asked
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_asked_ahead_waits_on_the_owner_with_the_agents_reason() {
    let store = store().await;

    let ServiceAsk::Filed(filed) = store.ask_service("s1", "app", &figcraft_port()).await else {
        panic!("the ask files a request");
    };

    assert_eq!(filed.reason.as_deref(), Some(REASON));
    assert_eq!(filed.state, NetworkRequestState::Asking);
    assert_eq!(
        filed.target,
        NetworkTarget::Service {
            service: "figcraft".to_owned(),
            record: figcraft_port().record(),
        }
    );
    let read = store.database.read().await.unwrap();
    let asking = cockpit_sandbox::asking_network_requests(&read)
        .await
        .unwrap();
    assert_eq!(asking, [filed]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_asked_again_returns_the_standing_request() {
    let store = store().await;
    let ServiceAsk::Filed(filed) = store.ask_service("s1", "app", &figcraft_port()).await else {
        panic!("the first ask files a request");
    };

    let again = store.ask_service("s1", "app", &figcraft_port()).await;

    assert_eq!(again, ServiceAsk::Standing(filed));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_the_project_was_granted_is_not_asked_again() {
    let store = store().await;
    let asked = AskedService::of(&figcraft_port());
    let request = store.file_service("s1", "app", &asked).await.unwrap();
    store.allow(request, true, DECIDED_AT).await.unwrap();

    let ServiceAsk::Granted(grant) = store.ask_service("s2", "app", &figcraft_port()).await else {
        panic!("the grant answers the ask");
    };

    assert_eq!(grant.request, Some(request));
    let read = store.database.read().await.unwrap();
    assert!(
        cockpit_sandbox::asking_network_requests(&read)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_service_carries_no_reason() {
    let store = store().await;
    let request = store
        .file_service("s1", "app", &AskedService::of(&figcraft_port()))
        .await
        .unwrap();

    let read = store.database.read().await.unwrap();
    let filed = cockpit_sandbox::network_request(&read, request)
        .await
        .unwrap();
    assert_eq!(filed.reason, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_decided_list_holds_every_decision_oldest_first_and_no_asking_request() {
    let store = store().await;
    let denied = store.file("s1", "app").await.unwrap();
    store.file("s2", "app").await.unwrap();
    let allowed = store.file("s3", "app").await.unwrap();
    store.deny(denied).await;
    store.allow(allowed, true, DECIDED_AT + 10).await.unwrap();
    let read = store.database.read().await.unwrap();

    let decided = cockpit_sandbox::decided_network_requests(&read)
        .await
        .unwrap();

    let states: Vec<(i64, NetworkRequestState)> = decided
        .iter()
        .map(|request| (request.id, request.state))
        .collect();
    assert_eq!(
        states,
        [
            (denied, NetworkRequestState::Denied),
            (allowed, NetworkRequestState::AllowedAlways),
        ]
    );
}

fn check_run(run: &str) -> Subject {
    Subject::Check {
        run: run.to_owned(),
        check: "test".to_owned(),
    }
}

impl Store {
    async fn file_for(&self, subject: &Subject, project: &str) -> Option<i64> {
        let write = self.database.write().await.unwrap();
        let filed =
            cockpit_sandbox::file_network_request(&write, subject, project, &example(), ASKED_AT)
                .await
                .unwrap();
        write.commit().await.unwrap();
        filed.map(|request| request.id)
    }

    async fn standing_for(&self, subject: &Subject, project: &str) -> Option<NetworkRequest> {
        let read = self.database.read().await.unwrap();
        cockpit_sandbox::standing_check_request(&read, subject, project, &example())
            .await
            .unwrap()
    }
}

/// #352 (#862, Q2): a check run's refused host is a request naming the run and its check.
#[tokio::test(flavor = "multi_thread")]
async fn a_check_runs_refused_destination_is_filed_naming_the_run_and_its_check() {
    let store = store().await;

    let request = store.file_for(&check_run("execution-1"), "app").await;

    let read = store.database.read().await.unwrap();
    let asking = cockpit_sandbox::asking_network_requests(&read)
        .await
        .unwrap();
    assert_eq!(asking.len(), 1);
    assert_eq!(asking[0].id, request.unwrap());
    assert_eq!(asking[0].subject, check_run("execution-1"));
}

/// #352: a check run ends before the owner can decide, so "once" would allow nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_check_runs_request_is_not_allowed_once() {
    let store = store().await;
    let request = store
        .file_for(&check_run("execution-1"), "app")
        .await
        .unwrap();

    let allowed = store.allow(request, false, DECIDED_AT).await;

    assert!(
        matches!(
            allowed,
            Err(NetworkError::Refused(NetworkRefusal::OnceForACheck))
        ),
        "{allowed:?}"
    );
}

/// #352: one card per check and host, whichever run asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_rerun_of_the_check_files_no_second_request_while_one_asks() {
    let store = store().await;
    store.file_for(&check_run("execution-1"), "app").await;

    let rerun = store.file_for(&check_run("execution-2"), "app").await;

    assert_eq!(rerun, None);
}

/// #352: the store holds one standing request per check and host, whatever files it.
#[tokio::test(flavor = "multi_thread")]
async fn the_store_refuses_a_second_standing_request_of_a_check_for_a_host() {
    let store = store().await;
    let request = store.file_for(&check_run("execution-1"), "app").await;
    let write = store.database.write().await.unwrap();

    let second = write
        .execute(
            "INSERT INTO sandbox_network_requests
                (subject_kind, subject, subject_check, project, kind, host, port, state,
                 asked_at)
             SELECT 'check', 'execution-2', subject_check, project, kind, host, port, state,
                 asked_at
             FROM sandbox_network_requests WHERE id = ?1",
            [request.unwrap()],
        )
        .await;

    assert!(second.is_err(), "{second:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_check_or_project_asking_for_the_host_files_its_own_request() {
    let store = store().await;
    store.file_for(&check_run("execution-1"), "app").await;
    let lint = Subject::Check {
        run: "execution-2".to_owned(),
        check: "lint".to_owned(),
    };

    let other_check = store.file_for(&lint, "app").await;
    let other_project = store.file_for(&check_run("execution-3"), "web").await;

    assert!(other_check.is_some());
    assert!(other_project.is_some());
}

/// #352: a denial holds for the check's next runs, each of which names it.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_check_request_stands_for_the_checks_next_run() {
    let store = store().await;
    let request = store
        .file_for(&check_run("execution-1"), "app")
        .await
        .unwrap();
    store.deny(request).await;

    let rerun = store.file_for(&check_run("execution-2"), "app").await;

    assert_eq!(rerun, None);
    let standing = store.standing_for(&check_run("execution-2"), "app").await;
    assert_eq!(
        standing.map(|standing| (standing.id, standing.state)),
        Some((request, NetworkRequestState::Denied))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_granted_host_leaves_the_check_nothing_standing() {
    let store = store().await;
    let request = store
        .file_for(&check_run("execution-1"), "app")
        .await
        .unwrap();
    store.allow(request, true, DECIDED_AT).await.unwrap();

    let standing = store.standing_for(&check_run("execution-2"), "app").await;

    assert_eq!(standing, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_runs_allow_always_also_allows_the_projects_sessions() {
    let store = store().await;
    let request = store
        .file_for(&check_run("execution-1"), "app")
        .await
        .unwrap();

    store.allow(request, true, DECIDED_AT).await.unwrap();

    let read = store.database.read().await.unwrap();
    let grant = cockpit_sandbox::network_request(&read, request)
        .await
        .unwrap()
        .grant
        .expect("allow always made a grant");
    drop(read);
    assert_eq!(
        store.decisions("s1", "app").await.allowed,
        vec![entry(AllowSource::Grant {
            grant: grant.to_string()
        })]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_run_asks_ahead_for_no_service() {
    let store = store().await;
    let write = store.database.write().await.unwrap();

    let asked = cockpit_sandbox::ask_service(
        &write,
        &check_run("execution-1"),
        "app",
        &figcraft_port(),
        "needs it",
        ASKED_AT,
    )
    .await;

    assert!(
        matches!(
            asked,
            Err(NetworkError::Refused(NetworkRefusal::NotASession))
        ),
        "{asked:?}"
    );
}

/// #352: the migration that lets a check run ask keeps every earlier request a session's.
#[tokio::test(flavor = "multi_thread")]
async fn requests_made_before_check_subjects_stay_their_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let earlier = cockpit_turso::MigrationSet {
        owner: MIGRATIONS.owner,
        migrations: &MIGRATIONS.migrations[..6],
    };
    let database = Database::open(&path, &[earlier]).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO sandbox_network_requests
                (id, subject_kind, subject, project, kind, host, port, state, asked_at)
             VALUES (1, 'session', 's1', 'app', 'host', 'unknown.example.com', 443, 'asking',
                ?1)",
            [ASKED_AT],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);

    let database = Database::open(&path, &[MIGRATIONS]).await.unwrap();

    let read = database.read().await.unwrap();
    let decided = cockpit_sandbox::network_request(&read, 1).await.unwrap();
    assert_eq!(decided.subject, session("s1"));
    assert_eq!(decided.state, NetworkRequestState::Asking);
}

fn containers(project: &str) -> Subject {
    Subject::Containers {
        project: project.to_owned(),
    }
}

/// #390 (#856, the note on #674): a project engine's proxy files a refused host naming the
/// project's containers.
#[tokio::test(flavor = "multi_thread")]
async fn a_containers_refused_destination_is_filed_naming_the_projects_containers() {
    let store = store().await;

    let request = store.file_for(&containers("app"), "app").await;

    let read = store.database.read().await.unwrap();
    let asking = cockpit_sandbox::asking_network_requests(&read)
        .await
        .unwrap();
    assert_eq!(asking.len(), 1);
    assert_eq!(asking[0].id, request.unwrap());
    assert_eq!(asking[0].subject, containers("app"));
    assert_eq!(asking[0].project, "app");
}

/// #390: the engine outlives sessions, so "once" would last until a reboot with no grant to
/// revoke.
#[tokio::test(flavor = "multi_thread")]
async fn a_containers_request_is_not_allowed_once() {
    let store = store().await;
    let request = store.file_for(&containers("app"), "app").await.unwrap();

    let allowed = store.allow(request, false, DECIDED_AT).await;

    assert!(
        matches!(
            allowed,
            Err(NetworkError::Refused(NetworkRefusal::OnceForContainers))
        ),
        "{allowed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_containers_ask_once_per_host_while_a_request_stands() {
    let store = store().await;
    store.file_for(&containers("app"), "app").await;

    let again = store.file_for(&containers("app"), "app").await;
    let other_project = store.file_for(&containers("web"), "web").await;

    assert_eq!(again, None);
    assert!(other_project.is_some());
}

/// #390 Q4 A: the containers get the project's "allow always" grants, the same a session's
/// request creates.
#[tokio::test(flavor = "multi_thread")]
async fn a_containers_allow_always_grants_the_project_its_sessions_included() {
    let store = store().await;
    let request = store.file_for(&containers("app"), "app").await.unwrap();

    store.allow(request, true, DECIDED_AT).await.unwrap();

    let read = store.database.read().await.unwrap();
    let grant = cockpit_sandbox::network_request(&read, request)
        .await
        .unwrap()
        .grant
        .expect("allow always made a grant");
    let granted = vec![entry(AllowSource::Grant {
        grant: grant.to_string(),
    })];
    let for_containers = cockpit_sandbox::egress_decisions(&read, "app", &containers("app"))
        .await
        .unwrap();
    drop(read);
    assert_eq!(for_containers.allowed, granted);
    assert_eq!(store.decisions("s1", "app").await.allowed, granted);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_containers_request_reaches_the_engines_proxy_and_no_session() {
    let store = store().await;
    let request = store.file_for(&containers("app"), "app").await.unwrap();

    store.deny(request).await;

    let read = store.database.read().await.unwrap();
    let for_containers = cockpit_sandbox::egress_decisions(&read, "app", &containers("app"))
        .await
        .unwrap();
    drop(read);
    assert_eq!(
        for_containers.denied,
        vec![DeniedDestination {
            host: "unknown.example.com".to_owned(),
            port: 443,
            request,
        }]
    );
    assert_eq!(store.decisions("s1", "app").await.denied, vec![]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_projects_containers_ask_ahead_for_no_service() {
    let store = store().await;
    let write = store.database.write().await.unwrap();

    let asked = cockpit_sandbox::ask_service(
        &write,
        &containers("app"),
        "app",
        &figcraft_port(),
        "needs it",
        ASKED_AT,
    )
    .await;

    assert!(
        matches!(
            asked,
            Err(NetworkError::Refused(NetworkRefusal::NotASession))
        ),
        "{asked:?}"
    );
}

/// #390: the store holds no containers request allowed once, whatever writes it.
#[tokio::test(flavor = "multi_thread")]
async fn the_store_refuses_a_containers_request_allowed_once() {
    let store = store().await;
    let request = store.file_for(&containers("app"), "app").await.unwrap();
    let write = store.database.write().await.unwrap();

    let once = write
        .execute(
            "UPDATE sandbox_network_requests SET state = 'allowed-once', decided_at = ?2
             WHERE id = ?1",
            (request, DECIDED_AT),
        )
        .await;

    assert!(once.is_err(), "{once:?}");
}

/// #390: the migration that lets a project's containers ask keeps the sessions' and check
/// runs' requests as they were.
#[tokio::test(flavor = "multi_thread")]
async fn requests_made_before_containers_subjects_keep_their_subjects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let earlier = cockpit_turso::MigrationSet {
        owner: MIGRATIONS.owner,
        migrations: &MIGRATIONS.migrations[..7],
    };
    let database = Database::open(&path, &[earlier]).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO sandbox_network_requests
                (id, subject_kind, subject, subject_check, project, kind, host, port, state,
                 asked_at)
             VALUES (1, 'session', 's1', NULL, 'app', 'host', 'unknown.example.com', 443,
                'asking', ?1),
                (2, 'check', 'execution-1', 'test', 'app', 'host', 'unknown.example.com', 443,
                'asking', ?1)",
            [ASKED_AT],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);

    let database = Database::open(&path, &[MIGRATIONS]).await.unwrap();

    let read = database.read().await.unwrap();
    let session_request = cockpit_sandbox::network_request(&read, 1).await.unwrap();
    let check_request = cockpit_sandbox::network_request(&read, 2).await.unwrap();
    assert_eq!(session_request.subject, session("s1"));
    assert_eq!(check_request.subject, check_run("execution-1"));
    assert_eq!(check_request.state, NetworkRequestState::Asking);
}
