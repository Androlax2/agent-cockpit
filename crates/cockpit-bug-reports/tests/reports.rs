//! Issue #427's draft rules against a real state database: one draft per fingerprint per
//! day for agents and the daemon, never a limit on the owner, a per-session bound on what a
//! session may open whatever it claims, and only drafts change.
mod common;

use cockpit_bug_reports::{
    Bundle, Edit, Error, Filed, Kind, Origin, Refusal, Report, ReportId, SESSION_DRAFTS_PER_DAY,
    Status,
};
use common::{DAY, Fixture, NOW, agent, daemon, owner, requested};

fn created(filed: Filed) -> Report {
    match filed {
        Filed::Created(report) => report,
        other => panic!("expected a new draft, got {other:?}"),
    }
}

fn refusal(result: Result<impl std::fmt::Debug, Error>) -> Refusal {
    match result {
        Err(Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn an_agent_report_opens_a_draft_with_its_fingerprint_and_the_daemon_facts() {
    let fixture = Fixture::new().await;

    let report = created(
        fixture
            .file(agent("s1", "todo add answered internal"))
            .await
            .unwrap(),
    );

    assert_eq!(report.status, Status::Draft);
    assert_eq!(report.origin, Origin::Agent);
    assert_eq!(report.session.as_deref(), Some("s1"));
    assert_eq!(report.title, "todo add answered internal");
    assert_eq!(report.fingerprint.text(), "cockpit-0.1.0/internal/todo-add");
    assert_eq!(report.environment, fixture.facts.environment);
    assert_eq!(report.bundle, Bundle::Pending);
    assert_eq!(report.occurrences, 1);
}

#[tokio::test]
async fn an_identical_second_agent_report_is_merged_into_the_first() {
    let fixture = Fixture::new().await;
    let first = created(
        fixture
            .file(agent("s1", "todo add answered internal"))
            .await
            .unwrap(),
    );

    let second = fixture
        .file_at(agent("s2", "todo add answered internal"), NOW + 3600)
        .await
        .unwrap();

    let Filed::Merged(merged) = second else {
        panic!("expected a merge, got {second:?}");
    };
    assert_eq!(merged.id, first.id);
    assert_eq!(merged.occurrences, 2);
    assert_eq!(merged.description, "todo add answered internal");
    let read = fixture.database.read().await.unwrap();
    assert_eq!(
        cockpit_bug_reports::list(&read, false).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_repeat_that_says_something_new_adds_its_description_to_the_draft() {
    let fixture = Fixture::new().await;
    created(fixture.file(agent("s1", "first words")).await.unwrap());

    let filed = fixture.file(agent("s1", "other words")).await.unwrap();

    assert_eq!(
        filed.report().description,
        "first words\n\nAlso reported (occurrence 2):\nother words"
    );
}

#[tokio::test]
async fn another_component_is_another_bug() {
    let fixture = Fixture::new().await;
    let first = created(fixture.file(agent("s1", "broken")).await.unwrap());
    let mut other = agent("s1", "broken");
    other.component = Some("session-new".to_owned());

    let second = created(fixture.file(other).await.unwrap());

    assert_ne!(first.id, second.id);
}

#[tokio::test]
async fn the_next_day_the_same_fingerprint_opens_a_new_draft() {
    let fixture = Fixture::new().await;
    let first = created(fixture.file(agent("s1", "broken")).await.unwrap());

    let next = created(
        fixture
            .file_at(agent("s1", "broken"), NOW + DAY)
            .await
            .unwrap(),
    );

    assert_ne!(first.id, next.id);
}

#[tokio::test]
async fn once_todays_draft_was_discarded_a_repeat_opens_nothing() {
    let fixture = Fixture::new().await;
    let first = created(
        fixture
            .file(daemon(Kind::Panic, "control/todo-add"))
            .await
            .unwrap(),
    );
    let write = fixture.database.write().await.unwrap();
    cockpit_bug_reports::discard(&write, first.id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let repeat = fixture
        .file(daemon(Kind::Panic, "control/todo-add"))
        .await
        .unwrap();

    assert!(matches!(repeat, Filed::AlreadyHandled(ref report) if report.id == first.id));
    let read = fixture.database.read().await.unwrap();
    assert!(
        cockpit_bug_reports::list(&read, false)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_owner_opens_a_draft_even_when_a_detected_one_has_the_same_fingerprint_today() {
    let fixture = Fixture::new().await;
    let detected = created(fixture.file(agent("s1", "broken")).await.unwrap());
    let mut by_owner = owner("I saw todo add fail");
    by_owner.kind = Kind::Internal;
    by_owner.component = Some("todo-add".to_owned());

    let report = created(fixture.file(by_owner).await.unwrap());

    assert_eq!(report.fingerprint, detected.fingerprint);
    assert_ne!(report.id, detected.id);
    assert_eq!(report.origin, Origin::Owner);
}

#[tokio::test]
async fn an_owner_requested_report_opens_a_draft_even_when_one_has_the_same_fingerprint_today() {
    let fixture = Fixture::new().await;
    let detected = created(fixture.file(agent("s1", "broken")).await.unwrap());

    let report = created(
        fixture
            .file(requested("s1", "the owner says todo add fails"))
            .await
            .unwrap(),
    );

    assert_eq!(report.fingerprint, detected.fingerprint);
    assert!(report.requested_by_owner);
}

#[tokio::test]
async fn a_session_opens_at_most_its_daily_drafts_whatever_it_claims() {
    let fixture = Fixture::new().await;
    for number in 0..SESSION_DRAFTS_PER_DAY {
        created(
            fixture
                .file(requested("s1", &format!("report {number}")))
                .await
                .unwrap(),
        );
    }

    let refused = refusal(fixture.file(requested("s1", "one more")).await);

    assert_eq!(
        refused,
        Refusal::SessionLimit {
            limit: SESSION_DRAFTS_PER_DAY
        }
    );
    assert!(refused.is_owners_to_lift());
    created(
        fixture
            .file(requested("s2", "another session"))
            .await
            .unwrap(),
    );
}

#[tokio::test]
async fn a_merge_is_not_counted_against_the_session() {
    let fixture = Fixture::new().await;
    for _ in 0..=SESSION_DRAFTS_PER_DAY {
        fixture.file(agent("s1", "the same")).await.unwrap();
    }

    let read = fixture.database.read().await.unwrap();
    let drafts = cockpit_bug_reports::list(&read, false).await.unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].occurrences, i64::from(SESSION_DRAFTS_PER_DAY) + 1);
}

#[tokio::test]
async fn the_owner_edits_a_draft() {
    let fixture = Fixture::new().await;
    let report = created(fixture.file(owner("it broke")).await.unwrap());
    let write = fixture.database.write().await.unwrap();

    let edited = cockpit_bug_reports::edit(
        &write,
        report.id,
        &Edit {
            title: Some("todo add fails".to_owned()),
            description: None,
            steps: Some("1. cockpit todo add x".to_owned()),
        },
        NOW + 1,
    )
    .await
    .unwrap();

    assert_eq!(edited.title, "todo add fails");
    assert_eq!(edited.description, "it broke");
    assert_eq!(edited.steps, "1. cockpit todo add x");
    assert_eq!(edited.changed_at, NOW + 1);
}

#[tokio::test]
async fn a_sent_report_keeps_its_issue_and_changes_no_more() {
    let fixture = Fixture::new().await;
    let report = created(fixture.file(owner("it broke")).await.unwrap());
    let write = fixture.database.write().await.unwrap();
    let sent = cockpit_bug_reports::mark_sent(
        &write,
        report.id,
        Some("https://github.com/Androlax2/agent-cockpit/issues/9"),
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(sent.status, Status::Sent);

    let refused = refusal(
        cockpit_bug_reports::edit(
            &write,
            report.id,
            &Edit {
                title: Some("x".to_owned()),
                ..Edit::default()
            },
            NOW,
        )
        .await,
    );

    assert!(matches!(
        refused,
        Refusal::WrongStatus {
            status: Status::Sent,
            ..
        }
    ));
}

#[tokio::test]
async fn the_list_shows_drafts_and_all_adds_the_others() {
    let fixture = Fixture::new().await;
    let kept = created(fixture.file(owner("one")).await.unwrap());
    let dropped = created(fixture.file(owner("two")).await.unwrap());
    let write = fixture.database.write().await.unwrap();
    cockpit_bug_reports::discard(&write, dropped.id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = fixture.database.read().await.unwrap();
    let drafts: Vec<ReportId> = cockpit_bug_reports::list(&read, false)
        .await
        .unwrap()
        .iter()
        .map(|report| report.id)
        .collect();
    let all = cockpit_bug_reports::list(&read, true).await.unwrap();

    assert_eq!(drafts, vec![kept.id]);
    assert_eq!(all.len(), 2);
    assert_eq!(cockpit_bug_reports::drafts_waiting(&read).await.unwrap(), 1);
}

#[tokio::test]
async fn a_draft_records_its_bundle_once() {
    let fixture = Fixture::new().await;
    let report = created(fixture.file(owner("it broke")).await.unwrap());
    let write = fixture.database.write().await.unwrap();
    let bundle = Bundle::Written {
        path: "/state/cockpit/bug-reports/1.tar".to_owned(),
        contents: "120\tcockpit-support-bundle/platform.json".to_owned(),
    };

    let attached = cockpit_bug_reports::attach_bundle(&write, report.id, &bundle, NOW)
        .await
        .unwrap();
    let again = cockpit_bug_reports::attach_bundle(
        &write,
        report.id,
        &Bundle::Failed {
            problem: "x".to_owned(),
        },
        NOW,
    )
    .await;

    assert_eq!(attached.bundle, bundle);
    assert!(matches!(
        refusal(again),
        Refusal::InvalidRequest {
            field: "bundle",
            ..
        }
    ));
}

#[tokio::test]
async fn reports_without_text_or_with_an_odd_component_are_refused() {
    let fixture = Fixture::new().await;
    let mut odd = agent("s1", "broken");
    odd.component = Some("Todo Add".to_owned());

    let blank = refusal(fixture.file(agent("s1", "  \n ")).await);
    let component = refusal(fixture.file(odd).await);

    assert!(matches!(
        blank,
        Refusal::InvalidRequest {
            field: "description",
            ..
        }
    ));
    assert!(matches!(
        component,
        Refusal::InvalidRequest {
            field: "component",
            ..
        }
    ));
}
