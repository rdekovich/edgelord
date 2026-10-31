//! Reconnecting wrapper for MarketDataStream.
//!
//! Provides automatic reconnection with exponential backoff and circuit breaker
//! protection for any [`MarketDataStream`] implementation. The wrapper
//! transparently handles disconnections and resubscribes to tracked tokens.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::domain::id::TokenId;
use crate::error::Error;
use crate::infrastructure::config::pool::ReconnectionConfig;
use crate::port::{outbound::exchange::MarketDataStream, outbound::exchange::MarketEvent};

/// Circuit breaker state for connection attempts.
///
/// Implements the circuit breaker pattern to prevent thundering herd problems
/// when the remote service is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitState {
    /// Normal operation; connections are allowed.
    Closed,
    /// Too many consecutive failures; connections blocked until cooldown expires.
    Open {
        /// Instant when the circuit breaker will transition back to Closed.
        until: Instant,
    },
}

/// Wrapper that adds automatic reconnection to any [`MarketDataStream`].
///
/// Transparently handles disconnections by:
/// 1. Waiting with exponential backoff
/// 2. Reconnecting to the WebSocket
/// 3. Resubscribing to all previously tracked tokens
///
/// A circuit breaker trips after too many consecutive failures to prevent
/// resource exhaustion.
pub struct ReconnectingDataStream<S: MarketDataStream> {
    /// The underlying data stream being wrapped.
    inner: S,
    /// Reconnection and backoff configuration.
    config: ReconnectionConfig,
    /// Token IDs to resubscribe after reconnection.
    subscribed_tokens: Vec<TokenId>,
    /// Current consecutive failure count.
    consecutive_failures: u32,
    /// Current backoff delay in milliseconds.
    current_delay_ms: u64,
    /// Circuit breaker state.
    circuit_state: CircuitState,
    /// Whether the stream is currently connected.
    connected: bool,
}

impl<S: MarketDataStream> ReconnectingDataStream<S> {
    /// Create a new reconnecting wrapper around a data stream.
    ///
    /// The wrapper starts in a disconnected state; call [`connect`](Self::connect)
    /// before reading events.
    pub fn new(inner: S, config: ReconnectionConfig) -> Self {
        let initial_delay = config.initial_delay_ms;
        Self {
            inner,
            config,
            subscribed_tokens: Vec::new(),
            consecutive_failures: 0,
            current_delay_ms: initial_delay,
            circuit_state: CircuitState::Closed,
            connected: false,
        }
    }

    /// Reset backoff state after a successful connection.
    ///
    /// Clears the failure count and resets the delay to the initial value.
    fn reset_backoff(&mut self) {
        self.consecutive_failures = 0;
        self.current_delay_ms = self.config.initial_delay_ms;
        self.circuit_state = CircuitState::Closed;
    }

    /// Calculate the next backoff delay using exponential backoff with jitter.
    ///
    /// Returns the current delay and advances the internal delay state for
    /// the next call.
    fn next_delay(&mut self) -> Duration {
        let base_delay = Duration::from_millis(self.current_delay_ms);
        let jitter_ms = self.jitter_ms(base_delay);
        let delay = base_delay + Duration::from_millis(jitter_ms);

        // Increase delay for next attempt
        let next_delay = (self.current_delay_ms as f64 * self.config.backoff_multiplier) as u64;
        self.current_delay_ms = next_delay.min(self.config.max_delay_ms);

        delay
    }

    /// Calculate jitter to add to the base delay.
    ///
    /// Adds up to 20% random jitter to prevent synchronized reconnection storms.
    fn jitter_ms(&self, base_delay: Duration) -> u64 {
        let jitter_range_ms = (base_delay.as_millis() as u64) / 5;
        if jitter_range_ms == 0 {
            return 0;
        }

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        (nanos as u64) % (jitter_range_ms + 1)
    }

    /// Check if the circuit breaker allows connection attempts.
    ///
    /// Returns true if the circuit is closed or has cooled down.
    /// Resets the circuit to closed if the cooldown has expired.
    fn circuit_allows_connection(&mut self) -> bool {
        match self.circuit_state {
            CircuitState::Closed => true,
            CircuitState::Open { until } => {
                if Instant::now() >= until {
                    info!("Circuit breaker cooldown expired, allowing reconnection");
                    self.circuit_state = CircuitState::Closed;
                    self.reset_backoff();
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a connection failure.
    ///
    /// Increments the failure count and trips the circuit breaker if the
    /// maximum consecutive failures threshold is exceeded.
    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.connected = false;

        if self.consecutive_failures >= self.config.max_consecutive_failures {
            let cooldown = Duration::from_millis(self.config.circuit_breaker_cooldown_ms);
            let until = Instant::now() + cooldown;
            self.circuit_state = CircuitState::Open { until };
            error!(
                failures = self.consecutive_failures,
                cooldown_secs = cooldown.as_secs(),
                "Circuit breaker tripped, pausing reconnection attempts"
            );
        }
    }

    /// Attempt to reconnect with backoff.
    ///
    /// Waits for the backoff delay, then attempts to reconnect and resubscribe
    /// to all previously tracked tokens.
    ///
    /// # Errors
    ///
    /// Returns an error if connection or resubscription fails.
    async fn reconnect(&mut self) -> Result<(), Error> {
        if !self.circuit_allows_connection() {
            // Circuit is open, wait for cooldown
            if let CircuitState::Open { until } = self.circuit_state {
                let remaining = until.saturating_duration_since(Instant::now());
                warn!(
                    remaining_secs = remaining.as_secs(),
                    "Circuit breaker open, waiting for cooldown"
                );
                sleep(remaining).await;
                // After cooldown, circuit should allow connection
                self.circuit_state = CircuitState::Closed;
                self.reset_backoff();
            }
        }

        let delay = self.next_delay();
        info!(
            delay_ms = delay.as_millis(),
            attempt = self.consecutive_failures + 1,
            "Reconnecting after delay"
        );
        sleep(delay).await;

        match self.inner.connect().await {
            Ok(()) => {
                info!("Reconnected successfully");
                self.connected = true;

                // Resubscribe to tokens
                if !self.subscribed_tokens.is_empty() {
                    debug!(
                        tokens = self.subscribed_tokens.len(),
                        "Resubscribing to tokens"
                    );
                    if let Err(err) = self.inner.subscribe(&self.subscribed_tokens).await {
                        error!(error = %err, "Resubscribe failed after reconnect");
                        self.connected = false;
                        self.record_failure();
                        return Err(err);
                    }
                }

                self.reset_backoff();
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "Reconnection failed");
                self.record_failure();
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<S: MarketDataStream + Send> MarketDataStream for ReconnectingDataStream<S> {
    async fn connect(&mut self) -> Result<(), Error> {
        let result = self.inner.connect().await;
        if result.is_ok() {
            self.connected = true;
            self.reset_backoff();
        }
        result
    }

    async fn subscribe(&mut self, token_ids: &[TokenId]) -> Result<(), Error> {
        // Store tokens for resubscription after reconnect
        self.subscribed_tokens = token_ids.to_vec();
        self.inner.subscribe(token_ids).await
    }

    async fn next_event(&mut self) -> Option<MarketEvent> {
        loop {
            // If not connected, try to reconnect
            if !self.connected {
                if let Err(e) = self.reconnect().await {
                    warn!(error = %e, "Reconnection attempt failed, will retry");
                    continue;
                }
            }

            // Get next event from inner stream
            match self.inner.next_event().await {
                Some(MarketEvent::Disconnected { reason }) => {
                    warn!(reason = %reason, "Connection lost, will reconnect");
                    self.connected = false;
                    self.record_failure();
                    // Invalidate downstream caches before reconnecting.
                    return Some(MarketEvent::Disconnected { reason });
                }
                Some(event) => {
                    // Reset failure count on successful event
                    if self.consecutive_failures > 0 {
                        debug!("Received event after reconnection, resetting failure count");
                        self.reset_backoff();
                    }
                    return Some(event);
                }
                None => {
                    // Stream ended unexpectedly
                    warn!("Data stream ended unexpectedly, will reconnect");
                    self.connected = false;
                    self.record_failure();
                    return Some(MarketEvent::Disconnected {
                        reason: "Stream ended".into(),
                    });
                }
            }
        }
    }

    fn exchange_name(&self) -> &'static str {
        self.inner.exchange_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    use crate::error::Error;
    use crate::testkit;
    use crate::testkit::stream::ScriptedStream;

    /// Reconnection config with non-zero delays for testing backoff behavior.
    fn backoff_config() -> ReconnectionConfig {
        ReconnectionConfig {
            initial_delay_ms: 10,
            max_delay_ms: 100,
            backoff_multiplier: 2.0,
            max_consecutive_failures: 3,
            circuit_breaker_cooldown_ms: 50,
        }
    }

    /// Config with minimal delays for faster tests.
    fn fast_config() -> ReconnectionConfig {
        ReconnectionConfig {
            initial_delay_ms: 1,
            max_delay_ms: 10,
            backoff_multiplier: 2.0,
            max_consecutive_failures: 3,
            circuit_breaker_cooldown_ms: 10,
        }
    }

    #[tokio::test]
    async fn test_successful_connection() {
        let mock = ScriptedStream::new()
            .with_events(vec![Some(testkit::domain::snapshot_event("token1"))]);

        let mut stream = ReconnectingDataStream::new(mock, backoff_config());
        stream.connect().await.unwrap();

        let event = stream.next_event().await;
        assert!(matches!(event, Some(MarketEvent::BookSnapshot { .. })));
    }

    #[tokio::test]
    async fn test_reconnect_after_disconnect() {
        let mock = ScriptedStream::new().with_events(vec![
            Some(testkit::domain::disconnect_event("test")),
            Some(testkit::domain::snapshot_event("token1")),
        ]);
        let (connect_count, subscribe_count) = mock.counts();

        let mut stream = ReconnectingDataStream::new(mock, backoff_config());
        stream.connect().await.unwrap();
        stream
            .subscribe(&[testkit::domain::token("token1")])
            .await
            .unwrap();

        // First call triggers reconnect, second returns snapshot
        assert!(matches!(
            stream.next_event().await,
            Some(MarketEvent::Disconnected { .. })
        ));
        let event = stream.next_event().await;
        assert!(matches!(event, Some(MarketEvent::BookSnapshot { .. })));

        assert!(connect_count.load(Ordering::SeqCst) >= 2);
        assert!(subscribe_count.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_exponential_backoff() {
        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), backoff_config());

        let assert_delay_in_range = |delay: Duration, base_ms: u64| {
            let max_ms = base_ms + (base_ms / 5);
            assert!(
                (base_ms..=max_ms).contains(&(delay.as_millis() as u64)),
                "delay {delay:?} not within {base_ms}..={max_ms} ms"
            );
        };

        assert_delay_in_range(stream.next_delay(), 10);
        assert_delay_in_range(stream.next_delay(), 20);
        assert_delay_in_range(stream.next_delay(), 40);
        assert_delay_in_range(stream.next_delay(), 80);
        assert_delay_in_range(stream.next_delay(), 100); // Capped at max
    }

    #[tokio::test]
    async fn test_circuit_breaker_trips() {
        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), backoff_config());

        for _ in 0..3 {
            stream.record_failure();
        }

        assert!(matches!(stream.circuit_state, CircuitState::Open { .. }));
        assert!(!stream.circuit_allows_connection());
    }

    #[tokio::test]
    async fn test_reset_backoff() {
        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), backoff_config());

        stream.consecutive_failures = 5;
        stream.current_delay_ms = 1000;
        stream.reset_backoff();

        assert_eq!(stream.consecutive_failures, 0);
        assert_eq!(stream.current_delay_ms, 10);
        assert!(matches!(stream.circuit_state, CircuitState::Closed));
    }

    // -----------------------------------------------------------------------
    // Additional Circuit Breaker Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_circuit_breaker_cooldown_expires() {
        let config = ReconnectionConfig {
            initial_delay_ms: 1,
            max_delay_ms: 10,
            backoff_multiplier: 1.0,
            max_consecutive_failures: 2,
            circuit_breaker_cooldown_ms: 10, // 10ms cooldown
        };

        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), config);

        // Trip the circuit breaker
        stream.record_failure();
        stream.record_failure();

        assert!(matches!(stream.circuit_state, CircuitState::Open { .. }));
        assert!(!stream.circuit_allows_connection());

        // Wait for cooldown to expire
        tokio::time::sleep(Duration::from_millis(15)).await;

        // Now connection should be allowed and circuit reset
        assert!(stream.circuit_allows_connection());
        assert!(matches!(stream.circuit_state, CircuitState::Closed));
    }

    #[tokio::test]
    async fn test_circuit_breaker_does_not_trip_below_threshold() {
        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), backoff_config());

        // Record failures below threshold
        stream.record_failure();
        stream.record_failure();

        // Should still be closed
        assert!(matches!(stream.circuit_state, CircuitState::Closed));
        assert!(stream.circuit_allows_connection());
    }

    #[tokio::test]
    async fn test_failure_count_increments() {
        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), backoff_config());

        assert_eq!(stream.consecutive_failures, 0);

        stream.record_failure();
        assert_eq!(stream.consecutive_failures, 1);

        stream.record_failure();
        assert_eq!(stream.consecutive_failures, 2);
    }

    #[tokio::test]
    async fn test_connected_flag_set_on_connect() {
        let mock = ScriptedStream::new();
        let mut stream = ReconnectingDataStream::new(mock, backoff_config());

        assert!(!stream.connected);

        stream.connect().await.unwrap();

        assert!(stream.connected);
    }

    #[tokio::test]
    async fn test_connected_flag_cleared_on_failure() {
        let mock = ScriptedStream::new();
        let mut stream = ReconnectingDataStream::new(mock, backoff_config());

        stream.connected = true;
        stream.record_failure();

        assert!(!stream.connected);
    }

    // -----------------------------------------------------------------------
    // Backoff Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_backoff_caps_at_max_delay() {
        let config = ReconnectionConfig {
            initial_delay_ms: 50,
            max_delay_ms: 100,
            backoff_multiplier: 10.0, // Large multiplier
            max_consecutive_failures: 10,
            circuit_breaker_cooldown_ms: 1000,
        };

        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), config);

        // First delay should be ~50ms (plus jitter)
        let delay1 = stream.next_delay();
        assert!(delay1.as_millis() <= 60); // 50 + 20% jitter

        // Second delay would be 500ms but capped at 100ms
        let delay2 = stream.next_delay();
        assert!(delay2.as_millis() <= 120); // 100 + 20% jitter
    }

    #[tokio::test]
    async fn test_jitter_is_bounded() {
        let config = ReconnectionConfig {
            initial_delay_ms: 100,
            max_delay_ms: 1000,
            backoff_multiplier: 1.0, // No increase to isolate jitter testing
            max_consecutive_failures: 10,
            circuit_breaker_cooldown_ms: 1000,
        };

        let mut stream = ReconnectingDataStream::new(ScriptedStream::new(), config);

        // Collect several delays to verify jitter bounds
        for _ in 0..10 {
            let delay = stream.next_delay();
            let delay_ms = delay.as_millis() as u64;
            // Should be between 100 and 120 (base + up to 20% jitter)
            assert!((100..=120).contains(&delay_ms), "delay was {delay_ms}ms");
        }
    }

    #[tokio::test]
    async fn test_zero_base_delay_zero_jitter() {
        // When base delay is 0, jitter should also be 0
        let config = ReconnectionConfig {
            initial_delay_ms: 0,
            max_delay_ms: 0,
            backoff_multiplier: 2.0,
            max_consecutive_failures: 10,
            circuit_breaker_cooldown_ms: 1000,
        };

        let stream = ReconnectingDataStream::new(ScriptedStream::new(), config);
        let jitter = stream.jitter_ms(Duration::from_millis(0));
        assert_eq!(jitter, 0);
    }

    // -----------------------------------------------------------------------
    // Subscribe Token Tracking Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_subscribe_stores_tokens() {
        let mock = ScriptedStream::new();
        let mut stream = ReconnectingDataStream::new(mock, backoff_config());

        let tokens = vec![
            testkit::domain::token("token1"),
            testkit::domain::token("token2"),
        ];

        stream.subscribe(&tokens).await.unwrap();

        assert_eq!(stream.subscribed_tokens.len(), 2);
        assert_eq!(stream.subscribed_tokens[0].as_str(), "token1");
        assert_eq!(stream.subscribed_tokens[1].as_str(), "token2");
    }

    #[tokio::test]
    async fn test_subscribe_replaces_previous_tokens() {
        let mock = ScriptedStream::new();
        let mut stream = ReconnectingDataStream::new(mock, backoff_config());

        stream
            .subscribe(&[testkit::domain::token("old_token")])
            .await
            .unwrap();
        stream
            .subscribe(&[testkit::domain::token("new_token")])
            .await
            .unwrap();

        assert_eq!(stream.subscribed_tokens.len(), 1);
        assert_eq!(stream.subscribed_tokens[0].as_str(), "new_token");
    }

    // -----------------------------------------------------------------------
    // Connection Error Handling Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_connect_failure_does_not_set_connected() {
        let mock = ScriptedStream::new()
            .with_connect_results(vec![Err(Error::Connection("test failure".to_string()))]);

        let mut stream = ReconnectingDataStream::new(mock, backoff_config());

        let result = stream.connect().await;
        assert!(result.is_err());
        assert!(!stream.connected);
    }

    #[tokio::test]
    async fn test_exchange_name_delegates_to_inner() {
        let mock = ScriptedStream::new();
        let stream = ReconnectingDataStream::new(mock, backoff_config());

        assert_eq!(stream.exchange_name(), "mock");
    }

    // -----------------------------------------------------------------------
    // Initial State Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_stream_initial_state() {
        let config = backoff_config();
        let stream = ReconnectingDataStream::new(ScriptedStream::new(), config.clone());

        assert!(!stream.connected);
        assert_eq!(stream.consecutive_failures, 0);
        assert_eq!(stream.current_delay_ms, config.initial_delay_ms);
        assert!(stream.subscribed_tokens.is_empty());
        assert!(matches!(stream.circuit_state, CircuitState::Closed));
    }

    // -----------------------------------------------------------------------
    // Reconnection Behavior Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reconnect_resubscribes_tokens() {
        // Setup: disconnect then reconnect with events
        let mock = ScriptedStream::new().with_events(vec![
            Some(testkit::domain::disconnect_event("connection lost")),
            Some(testkit::domain::snapshot_event("token1")),
        ]);
        let (_, subscribe_count) = mock.counts();

        let mut stream = ReconnectingDataStream::new(mock, fast_config());
        stream.connect().await.unwrap();

        // Subscribe to tokens before disconnect
        stream
            .subscribe(&[
                testkit::domain::token("token1"),
                testkit::domain::token("token2"),
            ])
            .await
            .unwrap();

        // This should trigger reconnect and resubscribe
        assert!(matches!(
            stream.next_event().await,
            Some(MarketEvent::Disconnected { .. })
        ));
        let event = stream.next_event().await;
        assert!(matches!(event, Some(MarketEvent::BookSnapshot { .. })));

        // Should have subscribed at least twice (initial + resubscribe)
        assert!(subscribe_count.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_success_after_reconnect_resets_failures() {
        let mock = ScriptedStream::new().with_events(vec![
            Some(testkit::domain::disconnect_event("test")),
            Some(testkit::domain::snapshot_event("token1")),
        ]);

        let mut stream = ReconnectingDataStream::new(mock, fast_config());
        stream.connect().await.unwrap();

        // Simulate some previous failures
        stream.consecutive_failures = 2;

        // Get event (triggers reconnect)
        assert!(matches!(
            stream.next_event().await,
            Some(MarketEvent::Disconnected { .. })
        ));
        let event = stream.next_event().await;
        assert!(matches!(event, Some(MarketEvent::BookSnapshot { .. })));

        // Failures should be reset after successful event
        assert_eq!(stream.consecutive_failures, 0);
    }
}
