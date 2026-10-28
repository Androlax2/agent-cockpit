#![allow(dead_code)]

use cockpit::chief::{WorkerState, store};
use cockpit::teams::{TransportOutcome, VerifiedSession};
use rusqlite::Connection;

pub fn remove_link_schema(connection: &Connection) {
    connection.execute_batch("ALTER TABLE messages DROP COLUMN link_id;
        ALTER TABLE messages DROP COLUMN scope_kind;
        ALTER TABLE threads DROP COLUMN link_id;
        ALTER TABLE threads DROP COLUMN scope_kind;
        DROP TABLE delivery_recipients;
        DROP TABLE links; DROP TABLE link_participants; DROP TABLE link_proposals; DROP TABLE link_work;
        DROP INDEX one_inflight_delivery;
        CREATE UNIQUE INDEX one_inflight_delivery ON messages(recipient,recipient_epoch) WHERE state='submitting';
        PRAGMA user_version=10;").unwrap();
}

/// Turns a migrated database back into the version 12 shape: the dispatch tables exist, the inbox and
/// delivery tables carry an `assignment` foreign key, and the stop/answer records do not.
pub fn restore_version_12_dispatch_schema(connection: &Connection) {
    connection.execute_batch("DROP TABLE chief_answers; DROP TABLE chief_stops;
        ALTER TABLE chief_decisions DROP COLUMN session_id;
        ALTER TABLE chief_decisions DROP COLUMN stop_id;
        ALTER TABLE chief_decisions DROP COLUMN category;
        ALTER TABLE chief_decisions DROP COLUMN suggested_answer;
        ALTER TABLE chief_decisions DROP COLUMN later_until;
        CREATE TABLE chief_worker_bindings (
            session_id TEXT NOT NULL, agent TEXT NOT NULL, project TEXT NOT NULL, runtime TEXT NOT NULL,
            epoch TEXT NOT NULL UNIQUE, role TEXT NOT NULL CHECK(role IN ('designer','developer','qa')),
            revision INTEGER NOT NULL CHECK(revision>0), state TEXT NOT NULL CHECK(state IN ('bound','revoked')),
            approval_hash TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(session_id,epoch));
        CREATE UNIQUE INDEX chief_one_bound_worker ON chief_worker_bindings(session_id) WHERE state='bound';
        CREATE TABLE chief_work_revisions (
            project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
            brief TEXT NOT NULL, approval_hash TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('approved','revoked','superseded','completed')),
            priority INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL,
            PRIMARY KEY(project,task_id,revision));
        CREATE TABLE chief_assignments (
            id TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, work_revision INTEGER NOT NULL,
            worker TEXT NOT NULL, worker_runtime TEXT NOT NULL, worker_epoch TEXT NOT NULL,
            chief_grant TEXT NOT NULL, chief_epoch TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('queued','submitting','received','accepted','working','blocked','uncertain','completed','cancelled')),
            revision INTEGER NOT NULL CHECK(revision>0), submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            created_at INTEGER NOT NULL, finished_at INTEGER,
            FOREIGN KEY(project,task_id,work_revision) REFERENCES chief_work_revisions(project,task_id,revision),
            UNIQUE(chief_epoch,submission_key));
        CREATE UNIQUE INDEX chief_one_open_work_claim ON chief_assignments(project,task_id) WHERE state NOT IN ('completed','cancelled');
        CREATE UNIQUE INDEX chief_one_open_worker_assignment ON chief_assignments(worker) WHERE state NOT IN ('completed','cancelled');
        CREATE TABLE chief_evidence (
            id TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, work_revision INTEGER NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('tests','qa','apply')), commit_id TEXT NOT NULL,
            source TEXT NOT NULL CHECK(source IN ('host_run','qa','reviewed_apply','host_attestation')),
            source_id TEXT NOT NULL, result TEXT NOT NULL, created_at INTEGER NOT NULL,
            FOREIGN KEY(project,task_id,work_revision) REFERENCES chief_work_revisions(project,task_id,revision),
            UNIQUE(source,source_id,project,task_id,work_revision,kind));
        CREATE TABLE chief_work_proposals (
            id TEXT PRIMARY KEY, sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL, brief TEXT NOT NULL,
            approval_hash TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(sender,submission_key), UNIQUE(project,task_id,revision));
        CREATE TABLE chief_sources (id TEXT PRIMARY KEY, evidence TEXT NOT NULL, approval_hash TEXT NOT NULL, imported INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
        CREATE TABLE chief_handoffs (handoff_id TEXT PRIMARY KEY, work TEXT NOT NULL);
        CREATE TABLE chief_apply_reviews (id TEXT PRIMARY KEY, review TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE TABLE chief_qa_results (sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, evidence_id TEXT NOT NULL, PRIMARY KEY(sender,submission_key));
        CREATE TABLE chief_submissions (sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, response TEXT NOT NULL, PRIMARY KEY(sender,submission_key));
        CREATE TRIGGER chief_work_immutable BEFORE UPDATE OF project,task_id,revision,brief,approval_hash ON chief_work_revisions
            BEGIN SELECT RAISE(ABORT,'approved chief brief and hash are immutable'); END;
        CREATE TRIGGER chief_evidence_immutable BEFORE UPDATE ON chief_evidence
            BEGIN SELECT RAISE(ABORT,'chief evidence is immutable'); END;
        CREATE TRIGGER review_chief_assignments_INSERT AFTER INSERT ON chief_assignments
            BEGIN UPDATE chief_review_generation SET revision=revision+1 WHERE id=1; END;
        CREATE TRIGGER review_chief_evidence_UPDATE AFTER UPDATE ON chief_evidence
            BEGIN UPDATE chief_review_generation SET revision=revision+1 WHERE id=1; END;
        ALTER TABLE chief_inbox RENAME TO chief_inbox_v13;
        CREATE TABLE chief_inbox (
            id TEXT PRIMARY KEY, assignment TEXT REFERENCES chief_assignments(id), chief_epoch TEXT NOT NULL,
            recipient TEXT NOT NULL, recipient_identity TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
            work TEXT NOT NULL, envelope TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('queued','submitting','received','uncertain','cancelled','failed')),
            next_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0);
        INSERT INTO chief_inbox SELECT id,NULL,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at,attempts FROM chief_inbox_v13;
        DROP TABLE chief_inbox_v13;
        ALTER TABLE chief_delivery_attempts RENAME TO chief_delivery_attempts_v13;
        CREATE TABLE chief_delivery_attempts (
            id TEXT PRIMARY KEY, assignment TEXT REFERENCES chief_assignments(id), event_id TEXT REFERENCES chief_events(id),
            recipient TEXT NOT NULL, recipient_runtime TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
            kind TEXT NOT NULL, envelope TEXT NOT NULL, idle_turn TEXT,
            state TEXT NOT NULL CHECK(state IN ('submitting','received','not_submitted','uncertain','cancelled')),
            native_id TEXT, receipt TEXT, started_at INTEGER NOT NULL, finished_at INTEGER);
        INSERT INTO chief_delivery_attempts SELECT id,NULL,event_id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,native_id,receipt,started_at,finished_at FROM chief_delivery_attempts_v13;
        DROP TABLE chief_delivery_attempts_v13;
        CREATE UNIQUE INDEX chief_inbox_inflight ON chief_inbox(recipient) WHERE state IN ('submitting','uncertain');
        CREATE UNIQUE INDEX chief_one_queued_wake ON chief_inbox(chief_epoch)
            WHERE json_extract(envelope,'$.kind')='chief_wake' AND state IN ('queued','submitting','uncertain');
        CREATE UNIQUE INDEX chief_one_inflight_recipient ON chief_delivery_attempts(recipient) WHERE state IN ('submitting','uncertain');
        PRAGMA user_version=12;").unwrap();
}

pub fn session() -> VerifiedSession {
    VerifiedSession {
        session_id: "aaaaaaaa-0000-4000-8000-000000000000".into(),
        agent: "claude".into(),
        project: "project-a".into(),
        runtime: "launch-123:pid-456:start-789".into(),
    }
}

pub fn admit(
    connection: &mut Connection,
    live: &[WorkerState],
    chief: &VerifiedSession,
    now_ms: i64,
) {
    store::verify_admission(
        connection,
        chief,
        "opencode-queued-v1",
        "Controlled test fixture, not a live support claim",
        now_ms,
    )
    .unwrap();
    store::observe(connection, live, now_ms).unwrap();
    let epoch: String = connection
        .query_row(
            "SELECT epoch FROM chief_grants WHERE state='active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let dispatch = store::claim(connection, chief, &format!("bootstrap-{epoch}"), now_ms)
        .unwrap()
        .expect("verified native chief bootstrap");
    assert_eq!(dispatch.envelope["kind"], "chief_wake");
    store::finish_for(
        connection,
        chief,
        &dispatch.id,
        &TransportOutcome::Received {
            native_id: format!("native-{epoch}"),
        },
        now_ms,
    )
    .unwrap();
}
