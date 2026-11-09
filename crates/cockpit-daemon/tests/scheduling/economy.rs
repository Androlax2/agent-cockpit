//! Economy (#412, owner choice A): the owner marks an item economy, and the queue places it
//! only on a session of its project started with the project's `[usage.economy]` model or
//! effort. Without one it waits, and `queue explain` says what to start.
use super::*;

fn low_effort() -> std::collections::BTreeMap<String, cockpit_scheduling::Economy> {
    [(
        "app".to_owned(),
        cockpit_scheduling::Economy {
            model: None,
            effort: Some("low".to_owned()),
        },
    )]
    .into()
}

async fn economy_item(client: &Connection) -> i64 {
    let item = queued_item(client, "Fix the login").await;
    request(
        client,
        Command::TaskEconomy {
            item,
            economy: true,
        },
    )
    .await
    .unwrap();
    item
}

#[tokio::test(flavor = "multi_thread")]
async fn an_economy_item_is_placed_on_the_session_started_at_the_economy_effort() {
    let mut home = Home::new();
    home.economy = low_effort();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    home.session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let economy = home
        .session_at_effort(&client, &root, Some("low"), AgentState::Waiting)
        .await;
    economy_item(&client).await;

    let dispatched = request(&client, Command::QueueDispatch).await;

    let Ok(Reply::QueueDispatched { placed, .. }) = dispatched else {
        panic!("queue-dispatch: {dispatched:?}");
    };
    assert_eq!(placed.len(), 1, "{placed:?}");
    assert_eq!(placed[0].session, economy.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_explain_says_an_economy_item_waits_for_a_session_at_the_economy_effort() {
    let mut home = Home::new();
    home.economy = low_effort();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    home.session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let item = economy_item(&client).await;
    request(&client, Command::QueueDispatch).await.unwrap();

    let explained = request(&client, Command::QueueExplain { item: Some(item) }).await;

    let Ok(Reply::QueueExplanations { explanations }) = explained else {
        panic!("queue-explain: {explained:?}");
    };
    assert_eq!(explanations[0].reason, "economy");
    assert_eq!(
        explanations[0].action,
        "start one: `torlek session new DIR --effort low`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn session_show_names_the_effort_a_session_was_started_at() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;

    let started = new_session_at_effort(&client, &root, Some("low")).await;

    assert_eq!(started.effort.as_deref(), Some("low"));
    assert_eq!(started.model, None);
}
