mod common;

use cockpit_client::{ClientError, CloseReason, Subscription};
use cockpit_protocol::{Capability, ErrorCode, FeedPosition, ServerFrame};
use common::{FakeDaemon, GENEROUS, batch, cursor, subscribed};

const FEED: &[Capability] = &[Capability::ChangeFeed];

#[tokio::test]
async fn a_new_subscription_starts_from_a_snapshot_and_advances_once_applied() {
    let daemon = FakeDaemon::start();
    let mut subscription = Subscription::new(daemon.socket_path.clone(), None, GENEROUS);
    let snapshot = batch(FeedPosition::Snapshot, cursor("e1", 5), &["t-1"]);
    let fake = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (id, from) = peer.read_subscribe().await;
        peer.reply(id, subscribed("e1", 5)).await;
        peer.send_batch(&snapshot).await;
        (from, peer)
    };

    let (received, (from, _peer)) = tokio::join!(subscription.next_batch(), fake);

    assert_eq!(from, FeedPosition::Snapshot);
    let pending = received.unwrap();
    assert_eq!(pending.batch(), &snapshot);
    pending.applied();
    assert_eq!(subscription.applied_cursor(), Some(&cursor("e1", 5)));
}

/// Acceptance for #21: a disconnect mid-batch loses only the partial batch; the client
/// resubscribes from the last batch it applied.
#[tokio::test]
async fn a_disconnect_mid_batch_resumes_from_the_last_applied_cursor() {
    let daemon = FakeDaemon::start();
    let mut subscription = Subscription::new(daemon.socket_path.clone(), None, GENEROUS);
    let first = batch(FeedPosition::Snapshot, cursor("e1", 5), &["t-1"]);
    let second = batch(
        FeedPosition::Cursor(cursor("e1", 5)),
        cursor("e1", 6),
        &["t-2"],
    );
    let fake = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (id, _) = peer.read_subscribe().await;
        peer.reply(id, subscribed("e1", 5)).await;
        peer.send_batch(&first).await;
        let whole = serde_json::to_vec(&ServerFrame::Changes(second.clone())).unwrap();
        peer.send_raw(&whole[..whole.len() / 2]).await;
        drop(peer);
    };

    let (cut_batch, ()) = tokio::join!(
        async {
            subscription.next_batch().await.unwrap().applied();
            subscription
                .next_batch()
                .await
                .map(|pending| pending.batch().clone())
        },
        fake
    );

    assert!(
        matches!(cut_batch, Err(ClientError::Closed(CloseReason::Truncated))),
        "{cut_batch:?}"
    );
    assert_eq!(subscription.applied_cursor(), Some(&cursor("e1", 5)));
    let resumed = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (id, from) = peer.read_subscribe().await;
        peer.reply(id, subscribed("e1", 6)).await;
        peer.send_batch(&second).await;
        (from, peer)
    };
    let (received, (from, _peer)) = tokio::join!(subscription.next_batch(), resumed);
    assert_eq!(from, FeedPosition::Cursor(cursor("e1", 5)));
    assert_eq!(received.unwrap().batch(), &second);
}

#[tokio::test]
async fn resnapshot_required_subscribes_again_from_a_snapshot() {
    let daemon = FakeDaemon::start();
    let stale = cursor("old-epoch", 40);
    let mut subscription =
        Subscription::new(daemon.socket_path.clone(), Some(stale.clone()), GENEROUS);
    let snapshot = batch(FeedPosition::Snapshot, cursor("new-epoch", 2), &["t-1"]);
    let fake = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (refused_id, refused_from) = peer.read_subscribe().await;
        peer.fail(refused_id, ErrorCode::ResnapshotRequired).await;
        let (id, from) = peer.read_subscribe().await;
        peer.reply(id, subscribed("new-epoch", 2)).await;
        peer.send_batch(&snapshot).await;
        ([refused_from, from], peer)
    };

    let (received, (positions, _peer)) = tokio::join!(subscription.next_batch(), fake);

    assert_eq!(
        positions,
        [FeedPosition::Cursor(stale), FeedPosition::Snapshot]
    );
    received.unwrap().applied();
    assert_eq!(subscription.applied_cursor(), Some(&cursor("new-epoch", 2)));
}

#[tokio::test]
async fn a_gap_fails_loud_and_the_next_call_resubscribes_from_the_applied_cursor() {
    let daemon = FakeDaemon::start();
    let applied = cursor("e1", 5);
    let mut subscription =
        Subscription::new(daemon.socket_path.clone(), Some(applied.clone()), GENEROUS);
    let skips_ahead = batch(FeedPosition::Cursor(cursor("e1", 7)), cursor("e1", 8), &[]);
    let fake = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (id, _) = peer.read_subscribe().await;
        peer.reply(id, subscribed("e1", 8)).await;
        peer.send_batch(&skips_ahead).await;
        peer.expect_closed().await;
    };

    let (gap, ()) = tokio::join!(async { subscription.next_batch().await.map(|_| ()) }, fake);

    match gap {
        Err(ClientError::FeedGap {
            applied: at,
            follows,
        }) => {
            assert_eq!(at, Some(applied.clone()));
            assert_eq!(follows, FeedPosition::Cursor(cursor("e1", 7)));
        }
        other => panic!("expected a gap, got {other:?}"),
    }
    let resubscribed = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (_, from) = peer.read_subscribe().await;
        from
    };
    let (_, from) = tokio::join!(subscription.next_batch(), resubscribed);
    assert_eq!(from, FeedPosition::Cursor(applied));
}

#[tokio::test]
async fn a_batch_dropped_unapplied_is_delivered_again() {
    let daemon = FakeDaemon::start();
    let mut subscription = Subscription::new(daemon.socket_path.clone(), None, GENEROUS);
    let snapshot = batch(FeedPosition::Snapshot, cursor("e1", 5), &["t-1"]);
    let next = batch(
        FeedPosition::Cursor(cursor("e1", 5)),
        cursor("e1", 6),
        &["t-2"],
    );
    let fake = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (id, _) = peer.read_subscribe().await;
        peer.reply(id, subscribed("e1", 6)).await;
        peer.send_batch(&snapshot).await;
        peer.send_batch(&next).await;
        peer.expect_closed().await;
    };

    let (second_call, ()) = tokio::join!(
        async {
            let dropped = subscription.next_batch().await.unwrap();
            drop(dropped);
            subscription.next_batch().await.map(|_| ())
        },
        fake
    );

    assert!(
        matches!(second_call, Err(ClientError::FeedGap { applied: None, .. })),
        "{second_call:?}"
    );
    let redelivered = async {
        let mut peer = daemon.accept_handshake(FEED).await;
        let (id, from) = peer.read_subscribe().await;
        peer.reply(id, subscribed("e1", 6)).await;
        peer.send_batch(&snapshot).await;
        (from, peer)
    };
    let (received, (from, _peer)) = tokio::join!(subscription.next_batch(), redelivered);
    assert_eq!(from, FeedPosition::Snapshot);
    assert_eq!(received.unwrap().batch(), &snapshot);
}

#[tokio::test]
async fn a_daemon_without_the_change_feed_is_refused() {
    let daemon = FakeDaemon::start();
    let mut subscription = Subscription::new(daemon.socket_path.clone(), None, GENEROUS);

    let (result, _peer) = tokio::join!(
        async { subscription.next_batch().await.map(|_| ()) },
        daemon.accept_handshake(&[])
    );

    assert!(
        matches!(result, Err(ClientError::NotGranted(Capability::ChangeFeed))),
        "{result:?}"
    );
}
