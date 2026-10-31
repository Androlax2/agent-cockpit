//! Items, statuses, proposals, decline memory, waits, caps, blocker reports and revisions on
//! a real state database.

mod common;

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_tasks::{Actor, Edit, ItemId, ListFilter, NextAction, Refusal, Settings, Status};
use common::{Fixture, NOW, new_item, refusal};

#[tokio::test]
async fn a_session_files_in_its_own_project_unqueued() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;

    let item = fixture
        .add(&agent, new_item(None, "Fix the login"))
        .await
        .unwrap();

    assert_eq!(item.project, "app");
    assert_eq!(item.status, Status::Todo);
    assert!(!item.is_queued);
    assert_eq!(item.filed_by.as_ref().map(SessionId::as_str), Some("s1"));
    assert_eq!(item.next_action(), NextAction::Queue);
}

#[tokio::test]
async fn a_session_filing_in_another_project_hands_it_across_queued() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;

    let item = fixture
        .add(&agent, new_item(Some("lib"), "Expose the parser"))
        .await
        .unwrap();

    assert_eq!(item.project, "lib");
    assert_eq!(item.filed_from.as_deref(), Some("app"));
    assert!(item.is_queued);
    assert_eq!(item.next_action(), NextAction::AwaitPlacement);
}

#[tokio::test]
async fn the_cross_project_cap_refuses_a_filing_past_it_until_one_is_finished() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let first = fixture
        .add(&agent, new_item(Some("lib"), "One"))
        .await
        .unwrap();
    fixture.propose(&agent, Some("lib"), "Two").await.unwrap();

    let refused = refusal(fixture.add(&agent, new_item(Some("lib"), "Three")).await);
    assert_eq!(
        refused,
        Refusal::CapReached {
            project: "lib".to_owned(),
            open: 2,
            cap: 2
        }
    );

    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::finish(&write, &Actor::Owner, first.id, None, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    fixture
        .add(&agent, new_item(Some("lib"), "Three"))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_proposal_becomes_work_only_when_the_owner_approves_it() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let proposal = fixture.propose(&agent, None, "Cache builds").await.unwrap();
    assert_eq!(proposal.status, Status::Proposed);
    assert_eq!(
        proposal.why.as_deref(),
        Some("It would save a step every day.")
    );
    assert_eq!(proposal.next_action(), NextAction::DecideProposal);

    let write = fixture.database.write().await.unwrap();
    let started = cockpit_tasks::start(&write, &agent, proposal.id, NOW).await;
    assert!(matches!(
        refusal(started),
        Refusal::WrongStatus {
            status: Status::Proposed,
            ..
        }
    ));
    let approved_by_agent = cockpit_tasks::approve(&write, &agent, proposal.id, NOW).await;
    assert!(matches!(
        refusal(approved_by_agent),
        Refusal::OwnerOnly { .. }
    ));
    let approved = cockpit_tasks::approve(&write, &Actor::Owner, proposal.id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(approved.status, Status::Todo);
}

#[tokio::test]
async fn a_declined_title_cannot_be_filed_again_in_that_project() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let proposal = fixture
        .propose(&agent, None, "Rewrite in Go")
        .await
        .unwrap();
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::decline(&write, &Actor::Owner, proposal.id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    for (project, title) in [(None, "rewrite  in GO"), (Some("app"), "Rewrite in Go")] {
        let refused = refusal(fixture.propose(&agent, project, title).await);
        assert_eq!(
            refused,
            Refusal::Declined {
                project: "app".to_owned()
            }
        );
    }
    let refused = refusal(fixture.add(&agent, new_item(None, "Rewrite in Go")).await);
    assert!(matches!(refused, Refusal::Declined { .. }));
    fixture
        .propose(&agent, Some("lib"), "Rewrite in Go")
        .await
        .unwrap();
}

#[tokio::test]
async fn a_blocker_report_goes_queued_to_the_configured_project() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let write = fixture.database.write().await.unwrap();

    let report = cockpit_tasks::report_blocker(
        &write,
        &agent,
        &fixture.settings,
        "Cannot reach the registry",
        "greyproxy blocks registry.example.com",
        "",
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();

    assert_eq!(report.project, "dotfiles");
    assert!(report.is_blocker_report);
    assert!(report.is_queued);
    assert_eq!(report.filed_from.as_deref(), Some("app"));
    assert_eq!(
        report.why.as_deref(),
        Some("greyproxy blocks registry.example.com")
    );
}

#[tokio::test]
async fn a_blocker_report_without_a_configured_project_is_refused() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let write = fixture.database.write().await.unwrap();

    let refused = refusal(
        cockpit_tasks::report_blocker(
            &write,
            &agent,
            &Settings {
                blocker_project: None,
                cross_project_open_max: 2,
            },
            "Cannot reach the registry",
            "blocked",
            "",
            NOW,
        )
        .await,
    );

    assert_eq!(refused, Refusal::NoBlockerProject);
}

#[tokio::test]
async fn an_item_waiting_on_unfinished_items_cannot_start_and_names_them() {
    let fixture = Fixture::new().await;
    let first = fixture.owner_adds("app", "Schema").await;
    let mut second = new_item(Some("app"), "Backfill");
    second.after = vec![first.id];
    let second = fixture.add(&Actor::Owner, second).await.unwrap();
    assert_eq!(
        second.next_action(),
        NextAction::FinishDependencies {
            items: vec![first.id]
        }
    );

    let write = fixture.database.write().await.unwrap();
    let refused = refusal(cockpit_tasks::start(&write, &Actor::Owner, second.id, NOW).await);

    assert_eq!(
        refused,
        Refusal::Waiting {
            item: second.id,
            on: vec![first.id]
        }
    );
}

#[tokio::test]
async fn finishing_a_waited_on_item_frees_the_wait_and_passes_on_its_summary() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let first = fixture.owner_adds("app", "Schema").await;
    let mut second = new_item(Some("app"), "Backfill");
    second.after = vec![first.id];
    let second = fixture.add(&Actor::Owner, second).await.unwrap();

    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::start(&write, &agent, first.id, NOW)
        .await
        .unwrap();
    cockpit_tasks::finish(&write, &agent, first.id, Some("Added column `kind`."), NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let second = fixture.item(second.id).await;
    assert!(second.waiting_on().is_empty());
    assert_eq!(
        second.waits[0].summary.as_deref(),
        Some("Added column `kind`.")
    );
    assert_eq!(second.next_action(), NextAction::Queue);
}

#[tokio::test]
async fn a_wait_that_comes_back_to_the_item_is_refused() {
    let fixture = Fixture::new().await;
    let first = fixture.owner_adds("app", "One").await;
    let second = fixture.owner_adds("app", "Two").await;
    let third = fixture.owner_adds("app", "Three").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::wait(&write, &Actor::Owner, second.id, vec![first.id], NOW)
        .await
        .unwrap();
    cockpit_tasks::wait(&write, &Actor::Owner, third.id, vec![second.id], NOW)
        .await
        .unwrap();

    for on in [third.id, first.id] {
        let refused = refusal(cockpit_tasks::wait(&write, &Actor::Owner, on, vec![on], NOW).await);
        assert!(matches!(refused, Refusal::Cycle { .. }), "{refused:?}");
    }
    let refused =
        refusal(cockpit_tasks::wait(&write, &Actor::Owner, first.id, vec![third.id], NOW).await);
    assert_eq!(
        refused,
        Refusal::Cycle {
            item: first.id,
            on: third.id
        }
    );
}

#[tokio::test]
async fn deleting_an_item_open_items_wait_on_is_refused() {
    let fixture = Fixture::new().await;
    let first = fixture.owner_adds("app", "Schema").await;
    let mut second = new_item(Some("app"), "Backfill");
    second.after = vec![first.id];
    let second = fixture.add(&Actor::Owner, second).await.unwrap();
    let write = fixture.database.write().await.unwrap();

    let refused = refusal(cockpit_tasks::delete(&write, &Actor::Owner, first.id).await);

    assert_eq!(
        refused,
        Refusal::WaitedOn {
            item: first.id,
            by: vec![second.id]
        }
    );
}

#[tokio::test]
async fn a_session_finishes_only_the_item_it_is_doing() {
    let fixture = Fixture::new().await;
    let doer = fixture.session("s1", "app").await;
    let other = fixture.session("s2", "app").await;
    let item = fixture.owner_adds("app", "Schema").await;
    let write = fixture.database.write().await.unwrap();
    let started = cockpit_tasks::start(&write, &doer, item.id, NOW)
        .await
        .unwrap();
    assert_eq!(
        started.next_action(),
        NextAction::InProgress {
            session: Some(SessionId::new("s1").unwrap())
        }
    );

    let refused = refusal(cockpit_tasks::finish(&write, &other, item.id, None, NOW).await);

    assert_eq!(refused, Refusal::NotDoing { item: item.id });
}

#[tokio::test]
async fn a_session_without_change_state_over_itself_is_refused_naming_the_grant() {
    let fixture = Fixture::new().await;
    let session = SessionId::new("s9").unwrap();
    let ungranted = Actor::Session {
        session: session.clone(),
        project: Some("app".to_owned()),
    };

    let refused = refusal(fixture.add(&ungranted, new_item(None, "Anything")).await);

    assert_eq!(
        refused,
        Refusal::NotAuthorized(cockpit_authorization::Refusal::NoGrant {
            permission: Permission::ChangeState,
            scope: Scope::Session(session),
        })
    );
}

#[tokio::test]
async fn a_session_sees_its_project_and_what_it_filed_elsewhere_only() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let own = fixture.owner_adds("app", "Ours").await;
    let unrelated = fixture.owner_adds("lib", "Theirs").await;
    let filed = fixture
        .add(&agent, new_item(Some("lib"), "Filed there"))
        .await
        .unwrap();
    let read = fixture.database.read().await.unwrap();

    let seen: Vec<ItemId> = cockpit_tasks::list(&read, &agent, &ListFilter::default())
        .await
        .unwrap()
        .iter()
        .map(|item| item.id)
        .collect();

    assert_eq!(seen, vec![own.id, filed.id]);
    let hidden = refusal(cockpit_tasks::item(&read, &agent, unrelated.id).await);
    assert_eq!(hidden, Refusal::NotFound { item: unrelated.id });
}

#[tokio::test]
async fn an_edit_is_an_intent_revision_and_a_status_change_a_state_revision() {
    let fixture = Fixture::new().await;
    let item = fixture.owner_adds("app", "Schema").await;
    let write = fixture.database.write().await.unwrap();

    let edited = cockpit_tasks::edit(
        &write,
        &Actor::Owner,
        item.id,
        Some(1),
        Edit {
            notes: Some("Add `kind`.".to_owned()),
            ..Edit::default()
        },
        NOW,
    )
    .await
    .unwrap();
    let started = cockpit_tasks::start(&write, &Actor::Owner, item.id, NOW)
        .await
        .unwrap();

    assert_eq!((edited.intent_revision, edited.state_revision), (2, 1));
    assert_eq!((started.intent_revision, started.state_revision), (2, 2));
}

#[tokio::test]
async fn an_edit_made_on_an_older_intent_revision_is_refused() {
    let fixture = Fixture::new().await;
    let item = fixture.owner_adds("app", "Schema").await;
    let write = fixture.database.write().await.unwrap();
    let retitle = |title: &str| Edit {
        title: Some(title.to_owned()),
        ..Edit::default()
    };
    cockpit_tasks::edit(&write, &Actor::Owner, item.id, Some(1), retitle("A"), NOW)
        .await
        .unwrap();

    let refused = refusal(
        cockpit_tasks::edit(&write, &Actor::Owner, item.id, Some(1), retitle("B"), NOW).await,
    );

    assert_eq!(refused, Refusal::StaleRevision { current: 2 });
}

#[tokio::test]
async fn a_title_with_a_line_break_is_refused() {
    let fixture = Fixture::new().await;

    let refused = refusal(
        fixture
            .add(&Actor::Owner, new_item(Some("app"), "Two\nlines"))
            .await,
    );

    assert!(
        matches!(refused, Refusal::InvalidRequest { field: "title", .. }),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_owners_decisions_are_refused_to_a_session() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let item = fixture.owner_adds("app", "Schema").await;
    let write = fixture.database.write().await.unwrap();
    let id = item.id;

    let refusals = [
        refusal(cockpit_tasks::decline(&write, &agent, id, NOW).await),
        refusal(cockpit_tasks::reopen(&write, &agent, id, NOW).await),
        refusal(cockpit_tasks::set_queued(&write, &agent, id, true, NOW).await),
        refusal(
            cockpit_tasks::set_priority(&write, &agent, id, cockpit_tasks::Priority::HIGH, NOW)
                .await,
        ),
        refusal(cockpit_tasks::edit(&write, &agent, id, None, Edit::default(), NOW).await),
        refusal(cockpit_tasks::wait(&write, &agent, id, vec![id], NOW).await),
        refusal(cockpit_tasks::delete(&write, &agent, id).await),
    ];

    for refused in refusals {
        assert!(matches!(refused, Refusal::OwnerOnly { .. }), "{refused:?}");
    }
}

#[tokio::test]
async fn a_session_outside_every_project_cannot_hand_items_across() {
    let fixture = Fixture::new().await;
    let Actor::Session { session, .. } = fixture.session("s1", "app").await else {
        unreachable!("a fixture session is a session");
    };
    let homeless = Actor::Session {
        session,
        project: None,
    };

    let refused = refusal(
        fixture
            .add(&homeless, new_item(Some("lib"), "Anything"))
            .await,
    );

    assert_eq!(refused, Refusal::NoProject);
}

#[tokio::test]
async fn a_session_cannot_wait_on_an_item_it_cannot_see() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let hidden = fixture.owner_adds("lib", "Private").await;
    let mut probe = new_item(None, "Probe");
    probe.after = vec![hidden.id];

    let refused = refusal(fixture.add(&agent, probe).await);

    assert_eq!(refused, Refusal::NotFound { item: hidden.id });
}

#[tokio::test]
async fn a_deleted_items_id_is_never_given_to_another() {
    let fixture = Fixture::new().await;
    let deleted = fixture.owner_adds("app", "Newest").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::delete(&write, &Actor::Owner, deleted.id)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let next = fixture.owner_adds("app", "Next").await;

    assert_ne!(next.id, deleted.id);
}

#[tokio::test]
async fn declining_a_proposal_open_items_wait_on_is_refused() {
    let fixture = Fixture::new().await;
    let agent = fixture.session("s1", "app").await;
    let proposal = fixture.propose(&agent, None, "Idea").await.unwrap();
    let mut waiting = new_item(Some("app"), "After the idea");
    waiting.after = vec![proposal.id];
    let waiting = fixture.add(&Actor::Owner, waiting).await.unwrap();
    let write = fixture.database.write().await.unwrap();

    let refused = refusal(cockpit_tasks::decline(&write, &Actor::Owner, proposal.id, NOW).await);

    assert_eq!(
        refused,
        Refusal::WaitedOn {
            item: proposal.id,
            by: vec![waiting.id]
        }
    );
}

#[tokio::test]
async fn the_owner_cannot_finish_an_item_that_still_waits() {
    let fixture = Fixture::new().await;
    let first = fixture.owner_adds("app", "Schema").await;
    let mut second = new_item(Some("app"), "Backfill");
    second.after = vec![first.id];
    let second = fixture.add(&Actor::Owner, second).await.unwrap();
    let write = fixture.database.write().await.unwrap();

    let refused = refusal(cockpit_tasks::finish(&write, &Actor::Owner, second.id, None, NOW).await);

    assert_eq!(
        refused,
        Refusal::Waiting {
            item: second.id,
            on: vec![first.id]
        }
    );
}
