//! Connection spawning utilities.
//!
//! Provides functions for spawning new connection tasks and creating
//! connection state objects.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::domain::id::TokenId;
use crate::infrastructure::config::pool::ReconnectionConfig;
use crate::infrastructure::exchange::reconnecting::ReconnectingDataStream;
use crate::port::outbound::exchange::{MarketDataStream, MarketEvent};

use super::state::{epoch_millis, ConnectionState, SharedCounters};
use super::StreamFactory;

/// Spawn a connection task that reads events and forwards them to `event_tx`.
///
/// Creates a tokio task that:
/// 1. Connects to the WebSocket
/// 2. Subscribes to the specified tokens
/// 3. Forwards received events to the shared channel
///
/// This is a free function (not a method) so both the pool and the management
/// task can call it without borrow conflicts.
fn spawn_connection(
    factory: &StreamFactory,
    reconnection_config: ReconnectionConfig,
    tokens: Vec<TokenId>,
    event_tx: mpsc::Sender<MarketEvent>,
    connection_id: u64,
    last_event_at: Arc<AtomicU64>,
    _counters: Arc<SharedCounters>,
) -> tokio::task::JoinHandle<()> {
    let stream = factory();
    let mut stream = ReconnectingDataStream::new(stream, reconnection_config);

    tokio::spawn(async move {
        let token_count = tokens.len();
        debug!(
            connection_id,
            tokens = token_count,
            "Connection task starting"
        );

        if let Err(e) = stream.connect().await {
            error!(connection_id, error = %e, "Failed to connect");
            return;
        }
        if let Err(e) = stream.subscribe(&tokens).await {
            error!(connection_id, error = %e, "Failed to subscribe");
            return;
        }

        debug!(connection_id, tokens = token_count, "Subscribed");

        loop {
            match stream.next_event().await {
                Some(event) => {
                    last_event_at.store(epoch_millis(), Ordering::Relaxed);
                    if event_tx.send(event).await.is_err() {
                        debug!(connection_id, "Channel closed, terminating");
                        break;
                    }
                }
                None => {
                    warn!(connection_id, "Stream ended");
                    break;
                }
            }
        }

        debug!(connection_id, "Task terminated");
    })
}

/// Build a fresh [`ConnectionState`] and spawn its task immediately.
///
/// Initializes `last_event_at` to the current timestamp so that the
/// silent-death detector does not flag a brand-new connection that has not
/// yet received its first event.
pub(super) fn new_connection(
    id: u64,
    tokens: Vec<TokenId>,
    factory: &StreamFactory,
    reconnection_config: ReconnectionConfig,
    event_tx: mpsc::Sender<MarketEvent>,
    counters: Arc<SharedCounters>,
) -> ConnectionState {
    let last_event_at = Arc::new(AtomicU64::new(epoch_millis()));
    let handle = spawn_connection(
        factory,
        reconnection_config,
        tokens.clone(),
        event_tx,
        id,
        last_event_at.clone(),
        counters,
    );
    ConnectionState {
        id,
        tokens,
        spawned_at: Instant::now(),
        last_event_at,
        handle,
    }
}
