//! Per-session budgets (#412, owner answer A): the daemon counts a session's agent minutes
//! since it started, and once they reach the budget the owner gave it, the queue places
//! nothing more on it and `queue explain` asks the owner for more time.
use super::*;

impl Home {
    /// `session` started ten minutes ago and worked a four-minute turn since.
    async fn worked_four_minutes(&self, session: &str) {
        let now = common::unix_now();
        let database = self.database();
        let write = database.write().await.unwrap();
        write
            .execute(
                "UPDATE sessions SET created_at = created_at - 600 WHERE id = ?1",
                [session],
            )
            .await
            .unwrap();
        let turn = cockpit_usage::Turn::new(session, "app", now - 300, now - 60).unwrap();
        cockpit_usage::record_turn(&write, &turn).await.unwrap();
        write.commit().await.unwrap();
    }
}

async fn budget(client: &Connection, session: &SessionView, minutes: u32) {
    request(
        client,
        Command::SessionBudget {
            session: session.id.clone(),
            budget: cockpit_protocol::TaskBudgetView::Minutes(minutes),
        },
    )
    .await
    .unwrap();
}

/// A daemon whose only session, given `minutes`, has worked four minutes, after a pass over
/// one queued item: the placements it made, and the item.
struct Dispatched {
    placed: Vec<cockpit_protocol::QueuePlacementView>,
    item: i64,
    client: Connection,
    _daemon: Daemon,
    _home: Home,
}

async fn dispatched_with_budget(minutes: u32) -> Dispatched {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let session = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    home.worked_four_minutes(&session.id).await;
    budget(&client, &session, minutes).await;
    let item = queued_item(&client, "Fix the login").await;

    let dispatched = request(&client, Command::QueueDispatch).await;

    let Ok(Reply::QueueDispatched { placed, .. }) = dispatched else {
        panic!("queue-dispatch: {dispatched:?}");
    };
    Dispatched {
        placed,
        item,
        client,
        _daemon: daemon,
        _home: home,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_within_its_budget_takes_work() {
    let dispatched = dispatched_with_budget(5).await;

    assert_eq!(dispatched.placed.len(), 1, "{:?}", dispatched.placed);
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_explain_asks_the_owner_for_more_time_once_the_session_spent_its_budget() {
    let dispatched = dispatched_with_budget(3).await;

    let explained = request(
        &dispatched.client,
        Command::QueueExplain {
            item: Some(dispatched.item),
        },
    )
    .await;

    assert_eq!(dispatched.placed, []);
    let Ok(Reply::QueueExplanations { explanations }) = explained else {
        panic!("queue-explain: {explained:?}");
    };
    assert_eq!(explanations[0].reason, "session-over-budget");
    assert!(
        explanations[0]
            .why
            .ends_with("has worked about 4 min since it started, its 3 min budget"),
        "{}",
        explanations[0].why
    );
}
