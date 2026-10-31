//! Order book types for market depth representation.
//!
//! This module provides types for representing order book state:
//!
//! - [`PriceLevel`] - A single price level with size
//! - [`Book`] - Complete order book for a single token
//!
//! # Order Book Structure
//!
//! An order book has two sides:
//! - **Bids**: Buy orders, sorted by price descending (best bid first)
//! - **Asks**: Sell orders, sorted by price ascending (best ask first)
//!
//! # Examples
//!
//! Creating an order book:
//!
//! ```
//! use edgelord::domain::book::{Book, PriceLevel};
//! use edgelord::domain::id::TokenId;
//! use rust_decimal_macros::dec;
//!
//! let bids = vec![
//!     PriceLevel::new(dec!(0.45), dec!(100)),
//!     PriceLevel::new(dec!(0.44), dec!(200)),
//! ];
//! let asks = vec![
//!     PriceLevel::new(dec!(0.46), dec!(150)),
//!     PriceLevel::new(dec!(0.47), dec!(300)),
//! ];
//!
//! let book = Book::with_levels(TokenId::new("yes-token"), bids, asks);
//!
//! assert_eq!(book.best_bid().unwrap().price(), dec!(0.45));
//! assert_eq!(book.best_ask().unwrap().price(), dec!(0.46));
//! ```

use super::id::TokenId;
use super::money::{Price, Volume};

/// A single price level in an order book.
///
/// Represents aggregated orders at a specific price point.
#[derive(Debug, Clone)]
pub struct PriceLevel {
    /// The price at this level.
    price: Price,
    /// Total volume available at this price.
    size: Volume,
}

impl PriceLevel {
    /// Creates a new price level.
    #[must_use]
    pub const fn new(price: Price, size: Volume) -> Self {
        Self { price, size }
    }

    /// Returns the price at this level.
    #[must_use]
    pub const fn price(&self) -> Price {
        self.price
    }

    /// Returns the total volume available at this level.
    #[must_use]
    pub const fn size(&self) -> Volume {
        self.size
    }
}

/// Order book for a single tradeable token.
///
/// Contains bid and ask price levels sorted by price (best prices first).
/// Bids are sorted descending, asks are sorted ascending.
#[derive(Debug, Clone)]
pub struct Book {
    /// Token ID this book represents.
    token_id: TokenId,
    /// Bid (buy) levels, sorted by price descending.
    bids: Vec<PriceLevel>,
    /// Ask (sell) levels, sorted by price ascending.
    asks: Vec<PriceLevel>,
    observed_at: std::time::Instant,
    source_timestamp: Option<i64>,
}

impl Book {
    /// Creates a new empty order book.
    #[must_use]
    pub fn new(token_id: TokenId) -> Self {
        Self {
            token_id,
            bids: Vec::new(),
            asks: Vec::new(),
            observed_at: std::time::Instant::now(),
            source_timestamp: None,
        }
    }

    /// Creates a book with initial price levels.
    ///
    /// Bids should be sorted by price descending, asks by price ascending.
    #[must_use]
    pub fn with_levels(
        token_id: TokenId,
        mut bids: Vec<PriceLevel>,
        mut asks: Vec<PriceLevel>,
    ) -> Self {
        bids.retain(|l| l.price > Price::ZERO && l.size > Volume::ZERO);
        asks.retain(|l| l.price > Price::ZERO && l.size > Volume::ZERO);
        bids.sort_by_key(|level| std::cmp::Reverse(level.price));
        asks.sort_by_key(|level| level.price);
        Self {
            token_id,
            bids,
            asks,
            observed_at: std::time::Instant::now(),
            source_timestamp: None,
        }
    }

    /// Source time orders updates across redundant connections.
    pub fn with_source_timestamp(mut self, timestamp: Option<i64>) -> Self {
        self.source_timestamp = timestamp;
        self
    }
    pub fn source_timestamp(&self) -> Option<i64> {
        self.source_timestamp
    }
    pub fn is_fresh(&self) -> bool {
        self.observed_at.elapsed() <= std::time::Duration::from_secs(5)
    }

    /// Returns the token ID for this book.
    #[must_use]
    pub const fn token_id(&self) -> &TokenId {
        &self.token_id
    }

    /// Returns all bid levels (sorted by price descending).
    #[must_use]
    pub fn bids(&self) -> &[PriceLevel] {
        &self.bids
    }

    /// Returns all ask levels (sorted by price ascending).
    #[must_use]
    pub fn asks(&self) -> &[PriceLevel] {
        &self.asks
    }

    /// Returns the best bid (highest buy price).
    #[must_use]
    pub fn best_bid(&self) -> Option<&PriceLevel> {
        self.bids.first()
    }

    /// Returns the best ask (lowest sell price).
    #[must_use]
    pub fn best_ask(&self) -> Option<&PriceLevel> {
        self.asks.first()
    }
}
