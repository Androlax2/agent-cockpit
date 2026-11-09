//! A spent budget tells the owner (#412, owner choice A): when a turn of placed work ends past
//! its item's budget, the daemon puts one `budget-spent` entity on the change feed, which
//! `torlek-notify` tells. Nothing is typed to the agent. The owner's next decision on the item
//! takes the entity back, so the next spent budget is news again.
use super::*;

const BUDGET_SPENT: &str = "budget-spent";

/// One `budget-spent` row of the state database: its id, revision and state, `None` once
/// retracted.
type Notice = (String, i64, Option<serde_json::Value>);

impl Home {
    /// `session`'s open placement moved ten minutes back, and a turn opened five minutes ago
    /// that the daemon ends from the agent's last report, which is not `working`.
    async fn turn_ends_under_placement(&self, session: &str) {
        let now = common::unix_now();
        let database = self.database();
        let write = database.write().await.unwrap();
        write
            .execute(
                "UPDATE scheduling_placements SET placed_at = placed_at - 600
                 WHERE session = ?1 AND ended_at IS NULL",
                [session],
            )
            .await
            .unwrap();
        write.commit().await.unwrap();
        self.turn_ends(session, now - 300).await;
    }

    /// A turn of `session` opened at `started_at` that the daemon ends from the agent's last
    /// report, which is not `working`.
    pub(super) async fn turn_ends(&self, session: &str, started_at: i64) {
        let database = self.database();
        let write = database.write().await.unwrap();
        cockpit_usage::turn_began(&write, session, "app", started_at)
            .await
            .unwrap();
        write.commit().await.unwrap();
        self.until_no_open_turn().await;
    }

    async fn until_no_open_turn(&self) {
        let database = self.database();
        let deadline = Instant::now() + GENEROUS;
        loop {
            let read = database.read().await.unwrap();
            if cockpit_usage::open_turns(&read).await.unwrap().is_empty() {
                return;
            }
            drop(read);
            assert!(Instant::now() < deadline, "the turn never ended");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn budget_notices(&self) -> Vec<Notice> {
        let database = self.database();
        let read = database.read().await.unwrap();
        let mut rows = read
            .query(
                "SELECT entity, revision, state FROM messaging_entities
                 WHERE kind = ?1 ORDER BY entity",
                [BUDGET_SPENT],
            )
            .await
            .unwrap();
        let mut notices = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let state: Option<String> = row.get(2).unwrap();
            notices.push((
                row.get(0).unwrap(),
                row.get(1).unwrap(),
                state.map(|text| serde_json::from_str(&text).unwrap()),
            ));
        }
        notices
    }
}

/// A daemon with one idle session working a queued item of `minutes` budget.
async fn placed_with_budget(home: &Home, minutes: u32) -> (Daemon, Connection, SessionView, i64) {
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let session = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let item = queued_item(&client, "Fix the login").await;
    request(
        &client,
        Command::TaskBudget {
            item,
            budget: cockpit_protocol::TaskBudgetView::Minutes(minutes),
        },
    )
    .await
    .unwrap();
    let dispatched = request(&client, Command::QueueDispatch).await;
    let Ok(Reply::QueueDispatched { placed, .. }) = dispatched else {
        panic!("queue-dispatch: {dispatched:?}");
    };
    assert_eq!(placed.len(), 1, "{placed:?}");
    (daemon, client, session, item)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_ends_past_the_items_budget_puts_the_spent_budget_on_the_feed() {
    let home = Home::new();
    let (_daemon, _client, session, item) = placed_with_budget(&home, 1).await;

    home.turn_ends_under_placement(&session.id).await;

    assert_eq!(
        home.budget_notices().await,
        [(
            item.to_string(),
            1,
            Some(serde_json::json!({
                "item": item,
                "project": "app",
                "title": "Fix the login",
                "budget_minutes": 1,
            }))
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_ends_within_the_items_budget_puts_nothing_on_the_feed() {
    let home = Home::new();
    let (_daemon, _client, session, _item) = placed_with_budget(&home, 60).await;

    home.turn_ends_under_placement(&session.id).await;

    assert_eq!(home.budget_notices().await, []);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_later_turn_past_the_same_budget_tells_nothing_new() {
    let home = Home::new();
    let (_daemon, _client, session, _item) = placed_with_budget(&home, 1).await;
    home.turn_ends_under_placement(&session.id).await;
    let told = home.budget_notices().await;

    home.turn_ends_under_placement(&session.id).await;

    assert_eq!(home.budget_notices().await, told);
}

/// Whether the item's `budget-spent` entity is off the feed after `decision`.
async fn is_taken_back_by(decision: impl FnOnce(i64) -> Command) -> bool {
    let home = Home::new();
    let (_daemon, client, session, item) = placed_with_budget(&home, 1).await;
    home.turn_ends_under_placement(&session.id).await;

    request(&client, decision(item)).await.unwrap();

    let notices = home.budget_notices().await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    notices[0].2.is_none()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_budget_takes_the_spent_budget_back() {
    assert!(
        is_taken_back_by(|item| Command::TaskBudget {
            item,
            budget: cockpit_protocol::TaskBudgetView::Minutes(31),
        })
        .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn finishing_the_item_takes_the_spent_budget_back() {
    assert!(
        is_taken_back_by(|item| Command::TaskDone {
            launch_token: None,
            item,
            summary: None,
        })
        .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_the_item_takes_the_spent_budget_back() {
    assert!(
        is_taken_back_by(|item| Command::TaskDelete {
            launch_token: None,
            item,
        })
        .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_after_the_items_placement_ended_tells_nothing() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let session = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    home.close_terminal_of(&session.id);
    let item = queued_item(&client, "Fix the login").await;
    request(
        &client,
        Command::TaskBudget {
            item,
            budget: cockpit_protocol::TaskBudgetView::Minutes(1),
        },
    )
    .await
    .unwrap();
    request(&client, Command::QueueDispatch).await.unwrap();
    home.turn_ends_under_placement(&session.id).await;
    // Nothing reaches the closed terminal: the third try gives the item back.
    for _ in 0..3 {
        request(&client, Command::QueueDispatch).await.unwrap();
    }
    assert_eq!(task_status(&client, item).await, TaskStatusView::Todo);
    request(
        &client,
        Command::TaskBudget {
            item,
            budget: cockpit_protocol::TaskBudgetView::Minutes(2),
        },
    )
    .await
    .unwrap();
    // The next turn starts in a later second than the placement ended.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    home.turn_ends(&session.id, common::unix_now()).await;

    let notices = home.budget_notices().await;
    assert_eq!(notices[0].2, None, "{notices:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_raised_budget_spent_again_is_news_again() {
    let home = Home::new();
    let (_daemon, client, session, item) = placed_with_budget(&home, 1).await;
    home.turn_ends_under_placement(&session.id).await;
    request(
        &client,
        Command::TaskBudget {
            item,
            budget: cockpit_protocol::TaskBudgetView::Minutes(2),
        },
    )
    .await
    .unwrap();

    home.turn_ends_under_placement(&session.id).await;

    let notices = home.budget_notices().await;
    assert_eq!(
        notices[0].1, 3,
        "published, retracted, published: {notices:?}"
    );
    assert_eq!(notices[0].2.as_ref().unwrap()["budget_minutes"], 2);
}
