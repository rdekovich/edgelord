//! Polymarket WebSocket message types.
//!
//! Polymarket WebSocket messages are sent as JSON arrays containing one or more
//! book snapshot objects. Each object contains order book data for a single token.
//!
//! Example message format:
//! ```json
//! [{"market":"0x...","asset_id":"123...","timestamp":"1234","hash":"abc","bids":[...],"asks":[...]}]
//! ```

use serde::{Deserialize, Serialize};

use crate::domain::{book::Book, book::PriceLevel, id::TokenId};

/// Subscription request sent to Polymarket WebSocket
#[derive(Debug, Serialize)]
pub struct PolymarketSubscribeMessage {
    pub assets_ids: Vec<String>,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub custom_feature_enabled: bool,
}

impl PolymarketSubscribeMessage {
    pub fn new(asset_ids: Vec<String>) -> Self {
        Self {
            assets_ids: asset_ids,
            msg_type: "market".into(),
            custom_feature_enabled: true,
        }
    }
}

/// Messages received from Polymarket WebSocket.
///
/// Messages arrive as a JSON array of book snapshots. Each snapshot contains
/// the full order book state for a single token.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PolymarketWsMessage {
    /// Array of book snapshots (primary message format).
    Books(Vec<PolymarketBookMessage>),

    /// Single book snapshot.
    Book(PolymarketBookMessage),

    /// Absolute size changes at individual price levels.
    PriceChanges(PolymarketPriceChanges),

    /// Lifecycle updates requested with custom_feature_enabled.
    Lifecycle(PolymarketLifecycle),

    /// Unknown or unparseable message.
    Unknown(serde_json::Value),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event_type")]
pub enum PolymarketLifecycle {
    #[serde(rename = "market_resolved")]
    Resolved {
        market: String,
        winning_asset_id: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct PolymarketPriceChanges {
    pub price_changes: Vec<PolymarketPriceChange>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PolymarketPriceChange {
    pub asset_id: String,
    pub price: rust_decimal::Decimal,
    pub size: rust_decimal::Decimal,
    pub side: String,
}

#[derive(Debug, Deserialize)]
pub struct PolymarketBookMessage {
    pub asset_id: String,
    pub market: Option<String>,
    pub bids: Vec<PolymarketWsPriceLevel>,
    pub asks: Vec<PolymarketWsPriceLevel>,
    pub timestamp: Option<String>,
    pub hash: Option<String>,
}

impl PolymarketBookMessage {
    /// Convert this WebSocket message to a domain `Book`
    #[must_use]
    pub fn to_orderbook(&self) -> Book {
        let token_id = TokenId::from(self.asset_id.clone());
        let bids = Self::parse_levels(&self.bids);
        let asks = Self::parse_levels(&self.asks);
        Book::with_levels(token_id, bids, asks)
            .with_source_timestamp(self.timestamp.as_deref().and_then(|v| v.parse().ok()))
    }

    fn parse_levels(levels: &[PolymarketWsPriceLevel]) -> Vec<PriceLevel> {
        levels
            .iter()
            .filter_map(|pl| {
                Some(PriceLevel::new(
                    pl.price.parse().ok()?,
                    pl.size.parse().ok()?,
                ))
            })
            .collect()
    }
}

/// Price level as received from WebSocket (strings, not decimals)
#[derive(Debug, Clone, Deserialize)]
pub struct PolymarketWsPriceLevel {
    pub price: String,
    pub size: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // -------------------------------------------------------------------------
    // PolymarketSubscribeMessage Tests
    // -------------------------------------------------------------------------

    #[test]
    fn subscribe_message_new_sets_type_to_market() {
        let msg = PolymarketSubscribeMessage::new(vec!["asset1".into()]);
        assert_eq!(msg.msg_type, "market");
    }

    #[test]
    fn subscribe_message_new_stores_asset_ids() {
        let ids = vec!["asset1".into(), "asset2".into(), "asset3".into()];
        let msg = PolymarketSubscribeMessage::new(ids.clone());
        assert_eq!(msg.assets_ids, ids);
    }

    #[test]
    fn subscribe_message_serializes_correctly() {
        let msg = PolymarketSubscribeMessage::new(vec!["token-123".into()]);
        let json = serde_json::to_string(&msg).unwrap();

        assert!(json.contains(r#""assets_ids":["token-123"]"#));
        assert!(json.contains(r#""type":"market""#));
    }

    #[test]
    fn subscribe_message_empty_assets() {
        let msg = PolymarketSubscribeMessage::new(vec![]);
        assert!(msg.assets_ids.is_empty());
        assert_eq!(msg.msg_type, "market");
    }

    // -------------------------------------------------------------------------
    // PolymarketWsMessage Parsing Tests
    // -------------------------------------------------------------------------

    #[test]
    fn ws_message_parses_books_array() {
        let json = r#"[
            {
                "asset_id": "token-yes",
                "market": "0xmarket",
                "bids": [{"price": "0.45", "size": "100"}],
                "asks": [{"price": "0.55", "size": "200"}],
                "timestamp": "1234567890",
                "hash": "abc123"
            }
        ]"#;

        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            PolymarketWsMessage::Books(books) => {
                assert_eq!(books.len(), 1);
                assert_eq!(books[0].asset_id, "token-yes");
                assert_eq!(books[0].market, Some("0xmarket".into()));
            }
            _ => panic!("Expected Books variant"),
        }
    }

    #[test]
    fn ws_message_parses_multiple_books() {
        let json = r#"[
            {"asset_id": "token-1", "bids": [], "asks": []},
            {"asset_id": "token-2", "bids": [], "asks": []}
        ]"#;

        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            PolymarketWsMessage::Books(books) => {
                assert_eq!(books.len(), 2);
                assert_eq!(books[0].asset_id, "token-1");
                assert_eq!(books[1].asset_id, "token-2");
            }
            _ => panic!("Expected Books variant"),
        }
    }

    #[test]
    fn ws_message_parses_empty_array() {
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
    fn ws_message_unknown_falls_back_gracefully() {
        // A completely different structure should parse as Unknown
        let json = r#"{"type": "error", "message": "invalid subscription"}"#;
        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            PolymarketWsMessage::Unknown(value) => {
                assert!(value.get("type").is_some());
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    // -------------------------------------------------------------------------
    // PolymarketBookMessage Tests
    // -------------------------------------------------------------------------

    #[test]
    fn book_message_parses_with_all_fields() {
        let json = r#"{
            "asset_id": "token-abc",
            "market": "0xmarket123",
            "bids": [{"price": "0.40", "size": "500"}, {"price": "0.39", "size": "300"}],
            "asks": [{"price": "0.60", "size": "400"}, {"price": "0.61", "size": "250"}],
            "timestamp": "1700000000000",
            "hash": "deadbeef"
        }"#;

        let book: PolymarketBookMessage = serde_json::from_str(json).unwrap();
        assert_eq!(book.asset_id, "token-abc");
        assert_eq!(book.market, Some("0xmarket123".into()));
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.asks.len(), 2);
        assert_eq!(book.timestamp, Some("1700000000000".into()));
        assert_eq!(book.hash, Some("deadbeef".into()));
    }

    #[test]
    fn book_message_parses_with_minimal_fields() {
        let json = r#"{
            "asset_id": "token-minimal",
            "bids": [],
            "asks": []
        }"#;

        let book: PolymarketBookMessage = serde_json::from_str(json).unwrap();
        assert_eq!(book.asset_id, "token-minimal");
        assert!(book.market.is_none());
        assert!(book.timestamp.is_none());
        assert!(book.hash.is_none());
    }

    #[test]
    fn book_message_to_orderbook_converts_bids() {
        let json = r#"{
            "asset_id": "token-test",
            "bids": [{"price": "0.45", "size": "100"}, {"price": "0.44", "size": "200"}],
            "asks": []
        }"#;

        let book: PolymarketBookMessage = serde_json::from_str(json).unwrap();
        let orderbook = book.to_orderbook();

        assert_eq!(orderbook.token_id().as_str(), "token-test");
        assert_eq!(orderbook.bids().len(), 2);
        assert!(orderbook.asks().is_empty());

        assert_eq!(orderbook.bids()[0].price(), dec!(0.45));
        assert_eq!(orderbook.bids()[0].size(), dec!(100));
        assert_eq!(orderbook.bids()[1].price(), dec!(0.44));
        assert_eq!(orderbook.bids()[1].size(), dec!(200));
    }

    #[test]
    fn book_message_to_orderbook_converts_asks() {
        let json = r#"{
            "asset_id": "token-test",
            "bids": [],
            "asks": [{"price": "0.55", "size": "150"}, {"price": "0.56", "size": "250"}]
        }"#;

        let book: PolymarketBookMessage = serde_json::from_str(json).unwrap();
        let orderbook = book.to_orderbook();

        assert!(orderbook.bids().is_empty());
        assert_eq!(orderbook.asks().len(), 2);

        assert_eq!(orderbook.asks()[0].price(), dec!(0.55));
        assert_eq!(orderbook.asks()[0].size(), dec!(150));
    }

    #[test]
    fn book_message_to_orderbook_filters_invalid_prices() {
        let json = r#"{
            "asset_id": "token-test",
            "bids": [{"price": "0.45", "size": "100"}, {"price": "invalid", "size": "50"}],
            "asks": [{"price": "abc", "size": "100"}]
        }"#;

        let book: PolymarketBookMessage = serde_json::from_str(json).unwrap();
        let orderbook = book.to_orderbook();

        // Invalid prices should be filtered out
        assert_eq!(orderbook.bids().len(), 1);
        assert!(orderbook.asks().is_empty());
    }

    #[test]
    fn book_message_to_orderbook_filters_invalid_sizes() {
        let json = r#"{
            "asset_id": "token-test",
            "bids": [{"price": "0.45", "size": "not-a-number"}],
            "asks": [{"price": "0.55", "size": "100"}]
        }"#;

        let book: PolymarketBookMessage = serde_json::from_str(json).unwrap();
        let orderbook = book.to_orderbook();

        // Invalid sizes should be filtered out
        assert!(orderbook.bids().is_empty());
        assert_eq!(orderbook.asks().len(), 1);
    }

    #[test]
    fn book_message_handles_decimal_precision() {
        let json = r#"{
            "asset_id": "token-precise",
            "bids": [{"price": "0.123456789", "size": "1000.50"}],
            "asks": []
        }"#;

        let book: PolymarketBookMessage = serde_json::from_str(json).unwrap();
        let orderbook = book.to_orderbook();

        assert_eq!(orderbook.bids()[0].price(), dec!(0.123456789));
        assert_eq!(orderbook.bids()[0].size(), dec!(1000.50));
    }

    // -------------------------------------------------------------------------
    // PolymarketWsPriceLevel Tests
    // -------------------------------------------------------------------------

    #[test]
    fn price_level_deserializes() {
        let json = r#"{"price": "0.50", "size": "1000"}"#;
        let level: PolymarketWsPriceLevel = serde_json::from_str(json).unwrap();

        assert_eq!(level.price, "0.50");
        assert_eq!(level.size, "1000");
    }

    #[test]
    fn price_level_handles_integer_strings() {
        let json = r#"{"price": "1", "size": "500"}"#;
        let level: PolymarketWsPriceLevel = serde_json::from_str(json).unwrap();

        assert_eq!(level.price, "1");
        assert_eq!(level.size, "500");
    }

    #[test]
    fn price_level_handles_zero_values() {
        let json = r#"{"price": "0", "size": "0"}"#;
        let level: PolymarketWsPriceLevel = serde_json::from_str(json).unwrap();

        assert_eq!(level.price, "0");
        assert_eq!(level.size, "0");
    }

    // -------------------------------------------------------------------------
    // Real-world Message Examples
    // -------------------------------------------------------------------------

    #[test]
    fn parses_realistic_polymarket_message() {
        // Simulated real Polymarket WebSocket message format
        let json = r#"[{
            "asset_id": "71321045679252212594626385532706912750332728571942532289631379312455583992563",
            "market": "0x00123456789abcdef",
            "bids": [
                {"price": "0.451", "size": "2500.00"},
                {"price": "0.450", "size": "5000.00"},
                {"price": "0.449", "size": "10000.00"}
            ],
            "asks": [
                {"price": "0.461", "size": "1800.00"},
                {"price": "0.462", "size": "3500.00"}
            ],
            "timestamp": "1703001234567",
            "hash": "a1b2c3d4e5f6"
        }]"#;

        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            PolymarketWsMessage::Books(books) => {
                assert_eq!(books.len(), 1);
                let book = &books[0];

                // Verify the long token ID is preserved
                assert!(book.asset_id.len() > 50);

                // Verify order book structure
                assert_eq!(book.bids.len(), 3);
                assert_eq!(book.asks.len(), 2);

                // Convert to domain orderbook
                let orderbook = book.to_orderbook();
                assert_eq!(orderbook.best_bid().unwrap().price(), dec!(0.451));
                assert_eq!(orderbook.best_ask().unwrap().price(), dec!(0.461));
            }
            _ => panic!("Expected Books variant"),
        }
    }

    #[test]
    fn handles_unicode_and_special_chars_in_ids() {
        // Edge case: unusual but valid asset IDs
        let json = r#"[{
            "asset_id": "token_with-special.chars123",
            "bids": [],
            "asks": []
        }]"#;

        let msg: PolymarketWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            PolymarketWsMessage::Books(books) => {
                assert_eq!(books[0].asset_id, "token_with-special.chars123");
            }
            _ => panic!("Expected Books variant"),
        }
    }
}
