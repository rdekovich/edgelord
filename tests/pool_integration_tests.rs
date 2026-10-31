//! Integration tests for the connection pool.
//!
//! These tests use mock streams to verify pool behavior end-to-end:
//! event delivery, multi-connection merging, backpressure,
//! TTL rotation, and reconnection.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use edgelord::infrastructure::exchange::pool::{ConnectionPool, StreamFactory};
use edgelord::port::{outbound::exchange::MarketDataStream, outbound::exchange::MarketEvent};
use edgelord::testkit;
use edgelord::testkit::stream::{channel_stream, ChannelStreamHandle, CyclingStream};

/// Collect handles created by the factory for inspection.
fn tracked_channel_factory() -> (
    StreamFactory,
    Arc<std::sync::Mutex<Vec<ChannelStreamHandle>>>,
) {
    let handles: Arc<std::sync::Mutex<Vec<ChannelStreamHandle>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let h = handles.clone();
    let factory: StreamFactory = Arc::new(move || {
        let (stream, handle) = channel_stream(64);
        h.lock().unwrap().push(handle);
        Box::new(stream)
    });
    (factory, handles)
}

// ---------------------------------------------------------------------------
// Test 1: Single connection delivers events end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_connection_delivers_events() {
    let (factory, handles) = tracked_channel_factory();

    let mut pool = ConnectionPool::new(
        testkit::config::pool(10, 500),
        testkit::config::reconnection(),
        factory,
        "test",
    )
    .unwrap();
    pool.connect().await.unwrap();
    pool.subscribe(&testkit::domain::make_tokens(3))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Extract sender before awaiting to avoid holding MutexGuard across await
    let sender = {
        let h = handles.lock().unwrap();
        assert_eq!(h[0].connect_count(), 1);
        assert_eq!(h[0].subscribe_count(), 1);
        assert_eq!(h[0].subscribed_tokens().len(), 3);
        h[0].sender()
    };

    sender
        .send(Some(testkit::domain::snapshot_event("t0")))
        .await
        .unwrap();
    sender
        .send(Some(testkit::domain::snapshot_event("t1")))
        .await
        .unwrap();

    let e1 = pool.next_event().await.unwrap();
    let e2 = pool.next_event().await.unwrap();

    let ids: Vec<_> = [e1, e2]
        .iter()
        .map(|e| match e {
            MarketEvent::BookSnapshot { token_id, .. } => token_id.to_string(),
            _ => panic!("unexpected event type"),
        })
        .collect();
    assert!(ids.contains(&"t0".to_string()));
    assert!(ids.contains(&"t1".to_string()));
}

// ---------------------------------------------------------------------------
// Test 2: Multiple connections merge into single stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_connection_merges_events() {
    let (factory, handles) = tracked_channel_factory();

    let mut pool = ConnectionPool::new(
        testkit::config::pool(10, 2),
        testkit::config::reconnection(),
        factory,
        "test",
    )
    .unwrap();
    pool.connect().await.unwrap();
    pool.subscribe(&testkit::domain::make_tokens(4))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Extract senders before awaiting to avoid holding MutexGuard across await
    let (sender0, sender1) = {
        let h = handles.lock().unwrap();
        assert_eq!(
            h.len(),
            2,
            "Expected 2 connections for 4 tokens with subs_per_conn=2"
        );
        (h[0].sender(), h[1].sender())
    };

    sender0
        .send(Some(testkit::domain::snapshot_event("conn0-a")))
        .await
        .unwrap();
    sender1
        .send(Some(testkit::domain::snapshot_event("conn1-a")))
        .await
        .unwrap();
    sender0
        .send(Some(testkit::domain::snapshot_event("conn0-b")))
        .await
        .unwrap();
    sender1
        .send(Some(testkit::domain::snapshot_event("conn1-b")))
        .await
        .unwrap();

    let mut received = HashSet::new();
    for _ in 0..4 {
        match tokio::time::timeout(Duration::from_secs(2), pool.next_event()).await {
            Ok(Some(MarketEvent::BookSnapshot { token_id, .. })) => {
                received.insert(token_id.to_string());
            }
            other => panic!("Expected snapshot event, got {:?}", other),
        }
    }

    assert_eq!(received.len(), 4);
    for id in ["conn0-a", "conn0-b", "conn1-a", "conn1-b"] {
        assert!(received.contains(id), "Missing event: {id}");
    }
}

// ---------------------------------------------------------------------------
// Test 3: Backpressure — full channel drops events and increments counter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backpressure_preserves_every_event() {
    let (factory, handles) = tracked_channel_factory();

    let mut cfg = testkit::config::pool(10, 500);
    cfg.channel_capacity = 5;

    let mut pool =
        ConnectionPool::new(cfg, testkit::config::reconnection(), factory, "test").unwrap();
    pool.connect().await.unwrap();
    pool.subscribe(&testkit::domain::make_tokens(1))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Extract sender before awaiting to avoid holding MutexGuard across await
    let sender = {
        let h = handles.lock().unwrap();
        h[0].sender()
    };
    for i in 0..20 {
        sender
            .send(Some(testkit::domain::snapshot_event(&format!("flood-{i}"))))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let stats = pool.stats();
    assert_eq!(stats.events_dropped, 0);

    let mut read = 0;
    while let Ok(Some(_)) =
        tokio::time::timeout(Duration::from_millis(100), pool.next_event()).await
    {
        read += 1;
    }
    assert_eq!(read, 20, "All events must reach the consumer");
    assert!(
        read + stats.events_dropped as usize >= 20,
        "read ({read}) + dropped ({}) should account for all 20 events",
        stats.events_dropped
    );
}

// ---------------------------------------------------------------------------
// Test 4: Server disconnect triggers reconnect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disconnect_triggers_reconnect() {
    let connect_count = Arc::new(AtomicU32::new(0));
    let cc = connect_count.clone();

    let factory: StreamFactory = Arc::new(move || {
        Box::new(CyclingStream::new(
            vec![testkit::domain::snapshot_event("t0")],
            Duration::from_millis(10),
            cc.clone(),
        ))
    });

    let mut cfg = testkit::config::pool(10, 500);
    cfg.max_silent_secs = 2;
    cfg.health_check_interval_secs = 1;

    let mut pool =
        ConnectionPool::new(cfg, testkit::config::reconnection(), factory, "test").unwrap();
    pool.connect().await.unwrap();
    pool.subscribe(&testkit::domain::make_tokens(1))
        .await
        .unwrap();

    for _ in 0..3 {
        let event = tokio::time::timeout(Duration::from_secs(2), pool.next_event())
            .await
            .expect("timeout waiting for event");
        assert!(event.is_some());
    }

    assert!(
        connect_count.load(Ordering::SeqCst) >= 1,
        "Expected at least 1 connection"
    );
}

// ---------------------------------------------------------------------------
// Test 5: TTL rotation replaces connections under continuous load
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ttl_rotation_under_load() {
    let connect_count = Arc::new(AtomicU32::new(0));
    let cc = connect_count.clone();

    let factory: StreamFactory = Arc::new(move || {
        Box::new(CyclingStream::new(
            vec![testkit::domain::snapshot_event("t0")],
            Duration::from_millis(20),
            cc.clone(),
        ))
    });

    let mut cfg = testkit::config::pool(10, 500);
    cfg.connection_ttl_secs = 3;
    cfg.preemptive_reconnect_secs = 2;
    cfg.health_check_interval_secs = 1;

    let mut pool =
        ConnectionPool::new(cfg, testkit::config::reconnection(), factory, "test").unwrap();
    pool.connect().await.unwrap();
    pool.subscribe(&testkit::domain::make_tokens(1))
        .await
        .unwrap();

    let drain = tokio::spawn(async move {
        let mut count = 0u64;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while let Ok(Some(_)) = tokio::time::timeout_at(deadline, pool.next_event()).await {
            count += 1;
        }
        (pool, count)
    });

    let (pool, events_received) = drain.await.unwrap();

    assert!(
        events_received > 10,
        "Expected >10 events over 5s, got {events_received}"
    );

    let stats = pool.stats();
    assert!(
        stats.total_rotations > 0,
        "Expected at least 1 TTL rotation, got {}",
        stats.total_rotations
    );

    assert!(
        connect_count.load(Ordering::SeqCst) > 1,
        "Expected more than 1 factory call after rotation"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Pool stats reflect reality
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pool_stats_reflect_connections() {
    let (factory, _handles) = tracked_channel_factory();

    let mut pool = ConnectionPool::new(
        testkit::config::pool(10, 2),
        testkit::config::reconnection(),
        factory,
        "test",
    )
    .unwrap();

    assert_eq!(pool.stats().active_connections, 0);

    pool.connect().await.unwrap();
    pool.subscribe(&testkit::domain::make_tokens(6))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(pool.stats().active_connections, 3);
    assert_eq!(pool.stats().total_rotations, 0);
    assert_eq!(pool.stats().total_restarts, 0);
    assert_eq!(pool.stats().events_dropped, 0);
}

// ---------------------------------------------------------------------------
// Test 7: Resubscribe tears down old connections cleanly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resubscribe_replaces_all_connections() {
    let (factory, handles) = tracked_channel_factory();

    let mut pool = ConnectionPool::new(
        testkit::config::pool(10, 2),
        testkit::config::reconnection(),
        factory,
        "test",
    )
    .unwrap();
    pool.connect().await.unwrap();

    pool.subscribe(&testkit::domain::make_tokens(4))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(pool.stats().active_connections, 2);

    pool.subscribe(&testkit::domain::make_tokens(6))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(pool.stats().active_connections, 3);

    // Handles 0-1 are dead (first subscribe), 2-4 are live (second subscribe).
    // Extract sender before awaiting to avoid holding MutexGuard across await
    let sender = {
        let h = handles.lock().unwrap();
        h[2].sender()
    };
    sender
        .send(Some(testkit::domain::snapshot_event("new-t0")))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), pool.next_event())
        .await
        .expect("timeout")
        .expect("no event");

    match event {
        MarketEvent::BookSnapshot { token_id, .. } => {
            assert_eq!(token_id.to_string(), "new-t0");
        }
        _ => panic!("unexpected event type"),
    }
}

// ---------------------------------------------------------------------------
// Test 8: Exchange name propagates correctly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exchange_name_propagates() {
    let (factory, _) = tracked_channel_factory();

    let pool = ConnectionPool::new(
        testkit::config::pool(10, 500),
        testkit::config::reconnection(),
        factory,
        "polymarket",
    )
    .unwrap();
    assert_eq!(pool.exchange_name(), "polymarket");
}
