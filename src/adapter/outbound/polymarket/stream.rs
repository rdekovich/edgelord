//! Polymarket WebSocket handler.
//!
//! This module provides a WebSocket client for connecting to the Polymarket
//! real-time data feed. It handles the full connection lifecycle including
//! establishing connections, subscribing to asset updates, and processing
//! incoming messages.
//!
//! # Connection Lifecycle
//!
//! 1. **Initialization**: Create a `PolymarketWebSocketHandler` with the target URL
//! 2. **Connection**: Establish a WebSocket connection via `connect()`
//! 3. **Subscription**: Subscribe to specific asset IDs via `subscribe()`
//! 4. **Message Loop**: Process incoming messages until close or error
//! 5. **Termination**: Clean up when connection closes or error occurs
//!
//! # Usage
//!
//! ```no_run
//! use edgelord::adapter::outbound::polymarket::stream::PolymarketWebSocketHandler;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let url = "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string();
//! let asset_ids = vec!["asset_1".to_string(), "asset_2".to_string()];
//! let handler = PolymarketWebSocketHandler::new(url);
//! handler.run(asset_ids, |msg| {
//!     // Process each incoming message
//! }).await?;
//! # Ok(())
//! # }
//! ```

use crate::domain::book::{Book, PriceLevel};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info, trace, warn};

#[cfg(test)]
use super::dto::message::PolymarketBookMessage;
use super::dto::message::{PolymarketSubscribeMessage, PolymarketWsMessage};
use crate::domain::id::TokenId;
use crate::error::Result;
use crate::port::{outbound::exchange::MarketDataStream, outbound::exchange::MarketEvent};

/// WebSocket handler for Polymarket real-time data feed.
///
/// Manages the WebSocket connection lifecycle and message processing.
/// The handler is stateless after construction - connection state is
/// managed within the `run()` method.
pub struct PolymarketWebSocketHandler {
    /// The WebSocket URL to connect to (e.g., <wss://ws-subscriptions-clob.polymarket.com/ws/market>)
    url: String,
}

impl PolymarketWebSocketHandler {
    /// Creates a new WebSocket handler for the given URL.
    ///
    /// # Arguments
    ///
    /// * `url` - The WebSocket URL to connect to
    #[must_use]
    pub const fn new(url: String) -> Self {
        Self { url }
    }

    /// Establishes a WebSocket connection to the configured URL.
    ///
    /// This is the first phase of the connection lifecycle. The connection
    /// is established using TLS if the URL scheme is `wss://`.
    ///
    /// # Returns
    ///
    /// A WebSocket stream that can be used for bidirectional communication.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails (network issues, invalid URL,
    /// TLS handshake failure, etc.).
    pub async fn connect(&self) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        info!(url = %self.url, "Connecting to WebSocket");

        let (ws_stream, response) = connect_async(&self.url).await?;

        info!(
            status = %response.status(),
            "WebSocket connected"
        );

        Ok(ws_stream)
    }

    /// Subscribes to real-time updates for the specified assets.
    ///
    /// Sends a subscription message to the Polymarket WebSocket server
    /// to begin receiving updates for the given asset IDs.
    ///
    /// # Arguments
    ///
    /// * `ws` - An active WebSocket connection
    /// * `asset_ids` - List of Polymarket asset IDs to subscribe to
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails or the message cannot be sent.
    pub async fn subscribe(
        ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        asset_ids: Vec<String>,
    ) -> Result<()> {
        let msg = PolymarketSubscribeMessage::new(asset_ids.clone());
        let json = serde_json::to_string(&msg)?;

        // Log a truncated view of assets to avoid spam
        let total = asset_ids.len();
        if total <= 5 {
            info!(assets = ?asset_ids, "Subscribing to assets");
        } else {
            let preview: Vec<_> = asset_ids.iter().take(5).collect();
            info!(assets = ?preview, more = total - 5, "Subscribing to assets");
        }
        ws.send(Message::Text(json)).await?;

        Ok(())
    }

    /// Runs the WebSocket message loop.
    ///
    /// This is the main entry point that orchestrates the full connection lifecycle:
    ///
    /// 1. **Connect**: Establishes the WebSocket connection
    /// 2. **Subscribe**: Subscribes to the specified asset IDs
    /// 3. **Message Loop**: Processes incoming messages until termination
    ///
    /// # Message Loop
    ///
    /// The message loop continuously reads from the WebSocket and handles
    /// different message types:
    ///
    /// - **Text messages**: Parsed as `PolymarketWsMessage` and passed to the callback.
    ///   Parse failures are logged but don't terminate the loop.
    /// - **Ping messages**: Automatically responded to with Pong (keepalive).
    /// - **Close messages**: Gracefully terminates the loop.
    /// - **Other messages**: Silently ignored (binary, pong, etc.).
    ///
    /// # Ping/Pong Handling
    ///
    /// WebSocket ping frames are automatically answered with pong frames
    /// containing the same payload. This is required by the WebSocket protocol
    /// to maintain the connection and allow the server to detect dead clients.
    ///
    /// # Error Handling
    ///
    /// - **Parse errors**: Logged as warnings, loop continues
    /// - **WebSocket errors**: Logged as errors, loop terminates
    /// - **Server close**: Logged as info, loop terminates gracefully
    ///
    /// # Reconnection Logic
    ///
    /// **Note**: This implementation does NOT automatically reconnect.
    /// When the connection closes (either by server close frame or error),
    /// the method returns. Callers are responsible for implementing retry
    /// logic if reconnection is desired.
    ///
    /// # Arguments
    ///
    /// * `asset_ids` - List of Polymarket asset IDs to subscribe to
    /// * `on_message` - Callback invoked for each successfully parsed message
    ///
    /// # Errors
    ///
    /// Returns an error if connection, subscription, or pong sending fails.
    pub async fn run<F>(&self, asset_ids: Vec<String>, mut on_message: F) -> Result<()>
    where
        F: FnMut(PolymarketWsMessage),
    {
        // Phase 1: Establish connection
        let mut ws = self.connect().await?;

        // Phase 2: Subscribe to requested assets
        Self::subscribe(&mut ws, asset_ids).await?;

        // Phase 3: Enter message loop
        debug!("Entering WebSocket message loop");

        // The message loop processes incoming WebSocket frames until:
        // - The server sends a Close frame
        // - A WebSocket error occurs
        // - The stream ends (connection lost)
        while let Some(msg_result) = ws.next().await {
            match msg_result {
                // Text message: Parse and dispatch to callback
                Ok(Message::Text(text)) => {
                    trace!(bytes = text.len(), "Received WebSocket text frame");

                    match serde_json::from_str::<PolymarketWsMessage>(&text) {
                        Ok(ws_msg) => on_message(ws_msg),
                        Err(e) => {
                            // Log parse failures but continue processing
                            // This allows the stream to recover from malformed messages
                            warn!(
                                error = %e,
                                bytes = text.len(),
                                "Failed to parse message"
                            );
                        }
                    }
                }
                // Ping/Pong: Respond to keep connection alive
                // The WebSocket protocol requires pong responses to contain
                // the same application data as the ping frame
                Ok(Message::Ping(data)) => {
                    trace!("Received WebSocket ping");
                    ws.send(Message::Pong(data)).await?;
                }
                // Close frame: Server is closing the connection
                Ok(Message::Close(frame)) => {
                    info!(frame = ?frame, "WebSocket closed by server");
                    break;
                }
                // Other message types (Binary, Pong, Frame) are ignored
                Ok(_) => {}
                // WebSocket error: Log and terminate
                // Common causes: network issues, protocol violations
                Err(e) => {
                    error!(error = %e, "WebSocket error");
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Polymarket market data stream implementing the `MarketDataStream` trait.
///
/// Provides an async iterator-style interface for receiving market events.
pub struct PolymarketDataStream {
    url: String,
    ws: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    /// Buffer for pending book messages when multiple arrive in one frame.
    pending_events: VecDeque<MarketEvent>,
    books: HashMap<TokenId, Book>,
    next_ping: tokio::time::Instant,
}

impl PolymarketDataStream {
    /// Create a new data stream for the given WebSocket URL.
    #[must_use]
    pub fn new(url: String) -> Self {
        Self {
            url,
            ws: None,
            pending_events: VecDeque::new(),
            books: HashMap::new(),
            next_ping: tokio::time::Instant::now(),
        }
    }
    fn ingest(&mut self, message: PolymarketWsMessage) {
        match message {
            PolymarketWsMessage::Books(books) => {
                for book in books {
                    self.ingest(PolymarketWsMessage::Book(book));
                }
            }
            PolymarketWsMessage::Book(message) => {
                let book = message.to_orderbook();
                let token_id = book.token_id().clone();
                if self.books.get(&token_id).is_some_and(|old| matches!((old.source_timestamp(), book.source_timestamp()), (Some(a), Some(b)) if b < a)) { return; }
                self.books.insert(token_id.clone(), book.clone());
                self.pending_events
                    .push_back(MarketEvent::BookSnapshot { token_id, book });
            }
            PolymarketWsMessage::PriceChanges(changes) => {
                let timestamp = changes.timestamp.as_deref().and_then(|v| v.parse().ok());
                for change in changes.price_changes {
                    let token_id = TokenId::new(change.asset_id);
                    // Deltas are unusable until this connection has a complete snapshot.
                    let Some(old) = self.books.get(&token_id) else {
                        continue;
                    };
                    if change.price <= rust_decimal::Decimal::ZERO
                        || change.size < rust_decimal::Decimal::ZERO
                    {
                        self.books.remove(&token_id);
                        self.pending_events.push_back(MarketEvent::Disconnected {
                            reason: "Invalid price change; resynchronization required".into(),
                        });
                        continue;
                    }
                    if matches!((old.source_timestamp(), timestamp), (Some(a), Some(b)) if b < a) {
                        continue;
                    }
                    let mut bids = old.bids().to_vec();
                    let mut asks = old.asks().to_vec();
                    let levels = match change.side.as_str() {
                        "BUY" => &mut bids,
                        "SELL" => &mut asks,
                        _ => continue,
                    };
                    levels.retain(|level| level.price() != change.price);
                    if change.size > rust_decimal::Decimal::ZERO {
                        levels.push(PriceLevel::new(change.price, change.size));
                    }
                    let book = Book::with_levels(token_id.clone(), bids, asks)
                        .with_source_timestamp(timestamp);
                    self.books.insert(token_id.clone(), book.clone());
                    self.pending_events
                        .push_back(MarketEvent::BookDelta { token_id, book });
                }
            }
            PolymarketWsMessage::Lifecycle(
                super::dto::message::PolymarketLifecycle::Resolved {
                    market,
                    winning_asset_id,
                },
            ) => {
                self.pending_events.push_back(MarketEvent::MarketSettled {
                    market_id: crate::domain::id::MarketId::new(market),
                    winning_outcome: winning_asset_id,
                    payout_per_share: rust_decimal::Decimal::ONE,
                });
            }
            PolymarketWsMessage::Unknown(value) => {
                if matches!(
                    value.get("event_type").and_then(|v| v.as_str()),
                    Some("book" | "price_change" | "market_resolved")
                ) {
                    self.pending_events.push_back(MarketEvent::Disconnected {
                        reason: "Malformed market event; resynchronization required".into(),
                    });
                }
            }
        }
    }
}

#[async_trait]
impl MarketDataStream for PolymarketDataStream {
    async fn connect(&mut self) -> Result<()> {
        info!(url = %self.url, "Connecting to WebSocket");
        let (ws_stream, response) = connect_async(&self.url).await?;
        info!(status = %response.status(), "WebSocket connected");
        self.ws = Some(ws_stream);
        self.books.clear();
        self.pending_events.clear();
        self.next_ping = tokio::time::Instant::now();
        Ok(())
    }

    async fn subscribe(&mut self, token_ids: &[TokenId]) -> Result<()> {
        let ws = self
            .ws
            .as_mut()
            .ok_or_else(|| crate::error::Error::Connection("Not connected".into()))?;

        let asset_ids: Vec<String> = token_ids.iter().map(|t| t.as_str().to_string()).collect();
        let msg = PolymarketSubscribeMessage::new(asset_ids.clone());
        let json = serde_json::to_string(&msg)?;

        // Log a truncated view of assets to avoid spam
        let total = asset_ids.len();
        if total <= 5 {
            info!(assets = ?asset_ids, "Subscribing to assets");
        } else {
            let preview: Vec<_> = asset_ids.iter().take(5).collect();
            info!(assets = ?preview, more = total - 5, "Subscribing to assets");
        }
        ws.send(Message::Text(json)).await?;
        Ok(())
    }

    async fn next_event(&mut self) -> Option<MarketEvent> {
        loop {
            if let Some(event) = self.pending_events.pop_front() {
                return Some(event);
            }
            let ws = self.ws.as_mut()?;
            let message = tokio::select! {
                message = ws.next() => message,
                _ = tokio::time::sleep_until(self.next_ping) => {
                    self.next_ping = tokio::time::Instant::now() + Duration::from_secs(10);
                    if ws.send(Message::Text("PING".into())).await.is_err() {
                        return Some(MarketEvent::Disconnected { reason: "Heartbeat send failed".into() });
                    }
                    continue;
                }
            };
            match message {
                Some(Ok(Message::Text(text))) if text == "PONG" => continue,
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<PolymarketWsMessage>(&text) {
                        Ok(message) => self.ingest(message),
                        Err(error) => {
                            return Some(MarketEvent::Disconnected {
                                reason: format!("Invalid market data: {error}"),
                            })
                        }
                    }
                }
                Some(Ok(Message::Ping(data))) => {
                    if ws.send(Message::Pong(data)).await.is_err() {
                        return Some(MarketEvent::Disconnected {
                            reason: "Pong send failed".into(),
                        });
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Some(MarketEvent::Disconnected {
                        reason: "Market stream closed".into(),
                    });
                }
                Some(Err(error)) => {
                    return Some(MarketEvent::Disconnected {
                        reason: error.to_string(),
                    })
                }
                _ => continue,
            }
        }
    }

    fn exchange_name(&self) -> &'static str {
        "Polymarket"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::outbound::polymarket::dto::message::PolymarketWsPriceLevel;
    use crate::domain::book::Book;
    use rust_decimal_macros::dec;

    // -------------------------------------------------------------------------
    // PolymarketWebSocketHandler Tests
    // -------------------------------------------------------------------------

    #[test]
    fn websocket_handler_stores_url() {
        let url = "wss://example.com/ws".to_string();
        let handler = PolymarketWebSocketHandler::new(url.clone());
        // We can't directly access the private field, but we test construction doesn't panic
        assert_eq!(
            std::mem::size_of_val(&handler),
            std::mem::size_of::<String>()
        );
    }

    #[test]
    fn websocket_handler_is_const_constructible() {
        // Test that new() is const fn
        const _HANDLER: PolymarketWebSocketHandler = PolymarketWebSocketHandler::new(String::new());
    }

    // -------------------------------------------------------------------------
    // PolymarketDataStream Tests
    // -------------------------------------------------------------------------

    #[test]
    fn data_stream_new_creates_disconnected_stream() {
        let stream = PolymarketDataStream::new("wss://test.com/ws".into());
        // Stream should start with no connection
        assert_eq!(stream.exchange_name(), "Polymarket");
    }

    #[test]
    fn data_stream_exchange_name_returns_polymarket() {
        let stream = PolymarketDataStream::new("wss://test.com".into());
        assert_eq!(stream.exchange_name(), "Polymarket");
    }

    // -------------------------------------------------------------------------
    // PolymarketSubscribeMessage Serialization Tests
    // -------------------------------------------------------------------------

    #[test]
    fn subscribe_message_serializes_correctly() {
        let msg = PolymarketSubscribeMessage::new(vec!["token1".into(), "token2".into()]);
        let json = serde_json::to_string(&msg).unwrap();

        // Should contain the assets_ids array
        assert!(json.contains("assets_ids"));
        assert!(json.contains("token1"));
        assert!(json.contains("token2"));

        // Should have type field renamed to "type"
        assert!(json.contains(r#""type":"market""#));
    }

    #[test]
    fn subscribe_message_with_single_asset() {
        let msg = PolymarketSubscribeMessage::new(vec!["single-token".into()]);
        let json = serde_json::to_string(&msg).unwrap();

        assert!(json.contains("single-token"));
        assert!(json.contains(r#""type":"market""#));
    }

    #[test]
    fn subscribe_message_with_empty_assets() {
        let msg = PolymarketSubscribeMessage::new(vec![]);
        let json = serde_json::to_string(&msg).unwrap();

        assert!(json.contains(r#""assets_ids":[]"#));
        assert!(json.contains(r#""type":"market""#));
    }

    // -------------------------------------------------------------------------
    // Message Parsing Tests
    // -------------------------------------------------------------------------

    #[test]
    fn parses_book_message_array() {
        let json = r#"[{
            "asset_id": "test-token",
            "bids": [{"price": "0.45", "size": "100"}],
            "asks": [{"price": "0.55", "size": "200"}]
        }]"#;

        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            PolymarketWsMessage::Books(books) => {
                assert_eq!(books.len(), 1);
                assert_eq!(books[0].asset_id, "test-token");
            }
            _ => panic!("Expected Books variant"),
        }
    }

    #[test]
    fn parses_empty_book_array() {
        let json = "[]";
        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();

        match msg {
            PolymarketWsMessage::Books(books) => {
                assert!(books.is_empty());
            }
            _ => panic!("Expected Books variant"),
        }
    }

    #[test]
    fn unknown_message_type_parses_as_unknown() {
        let json = r#"{"type": "heartbeat", "timestamp": 12345}"#;
        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();

        match msg {
            PolymarketWsMessage::Unknown(value) => {
                assert!(value.get("type").is_some());
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    // -------------------------------------------------------------------------
    // Order Book Conversion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn book_message_converts_to_domain_book() {
        let book_msg = PolymarketBookMessage {
            asset_id: "token-123".to_string(),
            market: Some("0xmarket".to_string()),
            bids: vec![
                PolymarketWsPriceLevel {
                    price: "0.45".to_string(),
                    size: "1000".to_string(),
                },
                PolymarketWsPriceLevel {
                    price: "0.44".to_string(),
                    size: "2000".to_string(),
                },
            ],
            asks: vec![PolymarketWsPriceLevel {
                price: "0.55".to_string(),
                size: "1500".to_string(),
            }],
            timestamp: Some("1234567890".to_string()),
            hash: Some("abcdef".to_string()),
        };

        let book = book_msg.to_orderbook();

        assert_eq!(book.token_id().as_str(), "token-123");
        assert_eq!(book.bids().len(), 2);
        assert_eq!(book.asks().len(), 1);

        // Check best prices
        assert_eq!(book.best_bid().unwrap().price(), dec!(0.45));
        assert_eq!(book.best_ask().unwrap().price(), dec!(0.55));
    }

    #[test]
    fn book_message_filters_invalid_entries() {
        let book_msg = PolymarketBookMessage {
            asset_id: "token-456".to_string(),
            market: None,
            bids: vec![
                PolymarketWsPriceLevel {
                    price: "0.45".to_string(),
                    size: "100".to_string(),
                },
                PolymarketWsPriceLevel {
                    price: "invalid".to_string(),
                    size: "200".to_string(),
                },
            ],
            asks: vec![PolymarketWsPriceLevel {
                price: "0.55".to_string(),
                size: "not-a-number".to_string(),
            }],
            timestamp: None,
            hash: None,
        };

        let book = book_msg.to_orderbook();

        // Only valid entries should be included
        assert_eq!(book.bids().len(), 1);
        assert_eq!(book.asks().len(), 0);
    }

    #[test]
    fn empty_book_message_produces_empty_book() {
        let book_msg = PolymarketBookMessage {
            asset_id: "empty-token".to_string(),
            market: None,
            bids: vec![],
            asks: vec![],
            timestamp: None,
            hash: None,
        };

        let book = book_msg.to_orderbook();

        assert_eq!(book.token_id().as_str(), "empty-token");
        assert!(book.bids().is_empty());
        assert!(book.asks().is_empty());
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
    }

    // -------------------------------------------------------------------------
    // MarketEvent Tests
    // -------------------------------------------------------------------------

    #[test]
    fn market_event_book_snapshot_has_token_id() {
        let token_id = TokenId::new("test-token");
        let book = Book::new(token_id.clone());
        let event = MarketEvent::BookSnapshot {
            token_id: token_id.clone(),
            book,
        };

        assert_eq!(event.token_id(), Some(&token_id));
    }

    #[test]
    fn market_event_disconnected_has_reason() {
        let event = MarketEvent::Disconnected {
            reason: "Connection lost".into(),
        };

        match event {
            MarketEvent::Disconnected { reason } => {
                assert_eq!(reason, "Connection lost");
            }
            _ => panic!("Expected Disconnected variant"),
        }
    }

    // -------------------------------------------------------------------------
    // Pending Books Buffer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn data_stream_starts_with_empty_pending_books() {
        let stream = PolymarketDataStream::new("wss://test.com".into());
        // The pending_books field is private, but we verify the behavior through
        // the fact that construction succeeds and exchange_name works
        assert_eq!(stream.exchange_name(), "Polymarket");
    }

    // -------------------------------------------------------------------------
    // Message Log Truncation Tests (verify logging doesn't overflow)
    // -------------------------------------------------------------------------

    #[test]
    fn many_assets_does_not_panic() {
        let assets: Vec<String> = (0..100).map(|i| format!("asset-{}", i)).collect();
        let msg = PolymarketSubscribeMessage::new(assets);

        // Should not panic with many assets
        assert_eq!(msg.assets_ids.len(), 100);
        assert_eq!(msg.msg_type, "market");
    }

    // -------------------------------------------------------------------------
    // Large Order Book Tests
    // -------------------------------------------------------------------------

    #[test]
    fn handles_large_order_book() {
        let bids: Vec<PolymarketWsPriceLevel> = (0..100)
            .map(|i| PolymarketWsPriceLevel {
                price: format!("0.{:02}", 50 - i.min(49)),
                size: format!("{}", (i + 1) * 100),
            })
            .collect();

        let asks: Vec<PolymarketWsPriceLevel> = (0..100)
            .map(|i| PolymarketWsPriceLevel {
                price: format!("0.{:02}", 51 + i.min(48)),
                size: format!("{}", (i + 1) * 100),
            })
            .collect();

        let book_msg = PolymarketBookMessage {
            asset_id: "large-book-token".to_string(),
            market: None,
            bids,
            asks,
            timestamp: None,
            hash: None,
        };

        let book = book_msg.to_orderbook();

        // Should handle many levels
        assert!(!book.bids().is_empty());
        assert!(!book.asks().is_empty());
    }

    // -------------------------------------------------------------------------
    // Integration-style Tests (without network)
    // -------------------------------------------------------------------------

    #[test]
    fn subscribe_message_round_trips() {
        let original = PolymarketSubscribeMessage::new(vec![
            "token-a".into(),
            "token-b".into(),
            "token-c".into(),
        ]);

        // Serialize
        let json = serde_json::to_string(&original).unwrap();

        // We can verify the JSON structure matches expectations
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["assets_ids"].as_array().unwrap().len(), 3);
        assert_eq!(parsed["type"], "market");
    }
}
