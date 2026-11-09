//! Explicit placement shares the routine scheduler's retained idle clock.
mod common;
use cockpit_scheduling::{Handoff, Inputs, Presence};
use cockpit_tasks::Priority;
use common::{Fixture, NOW};

#[tokio::test]
async fn a_fresh_waiting_report_does_not_reset_a_handoff_sessions_idle_window() {
    let fixture = Fixture::with_budget(2).await;
    let mut session = fixture.session("a-1", "a").await;
    fixture.dispatch_at(NOW, &[session.clone()]).await;
    let item = fixture.item("a", "Explicit work", Priority::NORMAL).await;
    session.presence = Presence::Waiting { since: NOW + 1 };
    let write = fixture.database.write().await.unwrap();
    let placed = cockpit_scheduling::handoff(
        &write,
        &Handoff {
            item: item.id,
            intent_revision: item.intent_revision,
            state_revision: item.state_revision,
            session: session.session.clone(),
        },
        &Inputs {
            candidates: vec![session],
            viewed: None,
            usage: Vec::new(),
            now: NOW + 1,
        },
        &fixture.settings,
        &fixture.policy,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(placed.item, item.id);
    write.commit().await.unwrap();
}
