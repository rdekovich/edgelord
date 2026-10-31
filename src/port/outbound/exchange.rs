//! Exchange integration ports for market data and order execution.
//!
//! Defines traits for interacting with prediction market exchanges. These are
//! the primary integration points for connecting to external trading platforms.
//!
//! # Overview
//!
//! - [`MarketFetcher`]: Fetch market listings from REST APIs
//! - [`MarketParser`]: Parse exchange-specific formats into domain types
//! - [`MarketDataStream`]: Real-time order book updates via WebSocket
//! - [`OrderExecutor`]: Submit and cancel orders
//! - [`ArbitrageExecutor`]: Execute multi-leg arbitrage trades

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::domain::{
    book::Book, id::MarketId, id::OrderId, id::TokenId, market::Market, market::Outcome,
    opportunity::Opportunity, trade::TradeResult,
};
use crate::error::Error;

/// Runtime statistics for a WebSocket connection pool.
///
/// Provides observability metrics for monitoring connection health and
/// performance. Used by operator interfaces such as the Telegram `/pool` command.
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    /// Number of currently active WebSocket connections.
    pub active_connections: usize,

    /// Total number of connection rotations triggered by TTL expiry.
    pub total_rotations: u64,

    /// Total number of connection restarts due to crashes or silence detection.
    pub total_restarts: u64,

    /// Total number of events dropped because the channel buffer was full.
    pub events_dropped: u64,
}

/// Result of attempting to execute an order on an exchange.
///
/// Represents the three possible outcomes: full fill, partial fill, or failure.
#[derive(Debug, Clone)]
pub enum ExecutionResult {
    /// Order was fully filled at or better than the limit price.
    Success {
        /// Unique order identifier returned by the exchange.
        order_id: OrderId,

        /// Total quantity filled.
        filled_amount: Decimal,

        /// Volume-weighted average execution price.
        average_price: Decimal,
    },

    /// Order was partially filled before expiring or being cancelled.
    PartialFill {
        /// Unique order identifier returned by the exchange.
        order_id: OrderId,

        /// Quantity that was filled.
        filled_amount: Decimal,

        /// Quantity remaining unfilled.
        remaining_amount: Decimal,

        /// Volume-weighted average execution price for the filled portion.
        average_price: Decimal,
    },

    /// Order failed to execute.
    Failed {
        /// Human-readable description of the failure reason.
        reason: String,
    },
}

impl ExecutionResult {
    /// Return `true` if the order was fully filled.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Return `true` if the order was partially filled.
    #[must_use]
    pub const fn is_partial(&self) -> bool {
        matches!(self, Self::PartialFill { .. })
    }

    /// Return `true` if the order failed to execute.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// Return the order identifier if the order was accepted by the exchange.
    ///
    /// Returns `None` for failed orders that were never assigned an ID.
    #[must_use]
    pub const fn order_id(&self) -> Option<&OrderId> {
        match self {
            Self::Success { order_id, .. } => Some(order_id),
            Self::PartialFill { order_id, .. } => Some(order_id),
            Self::Failed { .. } => None,
        }
    }
}

/// Request to place an order on an exchange.
///
/// Contains all parameters needed to submit a limit order.
#[derive(Debug, Clone)]
pub struct OrderRequest {
    /// Token identifier for the outcome to trade.
    pub token_id: String,

    /// Direction of the order (buy or sell).
    pub side: OrderSide,

    /// Quantity to trade in shares.
    pub size: Decimal,

    /// Maximum price for buys or minimum price for sells.
    pub price: Decimal,
}

/// Direction of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    /// Buy order (acquire shares).
    Buy,

    /// Sell order (dispose of shares).
    Sell,
}

/// Order executor for submitting and cancelling orders on an exchange.
///
/// # Thread Safety
///
/// Implementations must be thread-safe (`Send + Sync`) to support concurrent
/// order submission from multiple strategies.
///
/// # Errors
///
/// Methods return [`Error`] for network failures, authentication issues,
/// insufficient funds, or exchange-specific rejections.
#[async_trait]
pub trait OrderExecutor: Send + Sync {
    /// Submit an order to the exchange.
    ///
    /// # Arguments
    ///
    /// * `order` - Order parameters including token, side, size, and price.
    ///
    /// # Errors
    ///
    /// Returns an error if the order cannot be submitted due to network issues,
    /// authentication failures, or validation errors.
    async fn execute(&self, order: &OrderRequest) -> Result<ExecutionResult, Error>;

    /// Cancel an existing order by its identifier.
    ///
    /// # Arguments
    ///
    /// * `order_id` - Identifier of the order to cancel.
    ///
    /// # Errors
    ///
    /// Returns an error if the order cannot be found or cancellation fails.
    async fn cancel(&self, order_id: &OrderId) -> Result<(), Error>;

    /// Return the exchange name for logging and debugging.
    fn exchange_name(&self) -> &'static str;
}

/// Exchange-agnostic market information.
///
/// Represents the minimal information needed to identify and trade a market
/// across different prediction market exchanges. Used as an intermediate
/// representation before conversion to domain types.
#[derive(Debug, Clone)]
pub struct MarketInfo {
    /// Unique identifier for this market on the exchange.
    pub id: String,

    /// Human-readable market question or title.
    pub question: String,

    /// Outcome information including token identifiers and names.
    pub outcomes: Vec<OutcomeInfo>,

    /// Whether the market is currently accepting orders.
    pub active: bool,

    /// Trading volume over the last 24 hours in USD.
    ///
    /// `None` if volume data is not available from the exchange API.
    pub volume_24h: Option<f64>,

    /// Current liquidity depth in USD.
    ///
    /// `None` if liquidity data is not available from the exchange API.
    pub liquidity: Option<f64>,
}

/// Information about a single outcome in a market.
#[derive(Debug, Clone)]
pub struct OutcomeInfo {
    /// Token identifier for this outcome.
    pub token_id: String,

    /// Human-readable outcome name (e.g., "Yes", "No", "Trump", "Biden").
    pub name: String,

    /// Current price for this outcome as a probability (0.0 to 1.0).
    ///
    /// `None` if price data is not available from the REST API.
    pub price: Option<f64>,
}

impl MarketInfo {
    /// Return all token identifiers for this market.
    #[must_use]
    pub fn token_ids(&self) -> Vec<&str> {
        self.outcomes.iter().map(|o| o.token_id.as_str()).collect()
    }

    /// Return `true` if this is a binary (two-outcome) market.
    #[must_use]
    pub fn is_binary(&self) -> bool {
        self.outcomes.len() == 2
    }
}

/// Parser for converting exchange-specific market data into domain types.
///
/// Implementations handle the idiosyncrasies of each exchange's data format
/// and naming conventions.
///
/// # Thread Safety
///
/// Implementations must be thread-safe (`Send + Sync`).
pub trait MarketParser: Send + Sync {
    /// Return the exchange name for logging and adapter selection.
    fn name(&self) -> &'static str;

    /// Return the default payout amount for winning outcomes.
    ///
    /// Typically `1.00` for prediction markets where shares pay $1 on resolution.
    fn default_payout(&self) -> Decimal;

    /// Return the canonical outcome names for binary markets.
    ///
    /// Returns a tuple of `(positive, negative)` names, e.g., `("Yes", "No")`.
    fn binary_outcome_names(&self) -> (&'static str, &'static str);

    /// Return `true` if the outcome name represents the positive side.
    ///
    /// # Arguments
    ///
    /// * `name` - Outcome name from exchange data.
    ///
    /// Default implementation performs case-insensitive comparison.
    fn is_positive_outcome(&self, name: &str) -> bool {
        let (positive, _) = self.binary_outcome_names();
        name.eq_ignore_ascii_case(positive)
    }

    /// Return `true` if the outcome name represents the negative side.
    ///
    /// # Arguments
    ///
    /// * `name` - Outcome name from exchange data.
    ///
    /// Default implementation performs case-insensitive comparison.
    fn is_negative_outcome(&self, name: &str) -> bool {
        let (_, negative) = self.binary_outcome_names();
        name.eq_ignore_ascii_case(negative)
    }

    /// Convert exchange-agnostic market information into domain markets.
    ///
    /// # Arguments
    ///
    /// * `market_infos` - Raw market data from the exchange.
    ///
    /// Filters to binary markets only and maps outcome names to canonical forms.
    /// Returns an empty vector if no valid markets are found.
    fn parse_markets(&self, market_infos: &[MarketInfo]) -> Vec<Market> {
        let mut markets = Vec::new();
        let (positive_name, negative_name) = self.binary_outcome_names();
        let payout = self.default_payout();

        for info in market_infos {
            if info.outcomes.len() != 2 {
                continue;
            }

            let positive = info
                .outcomes
                .iter()
                .find(|outcome| self.is_positive_outcome(&outcome.name));
            let negative = info
                .outcomes
                .iter()
                .find(|outcome| self.is_negative_outcome(&outcome.name));

            if let (Some(pos), Some(neg)) = (positive, negative) {
                if pos.token_id.is_empty()
                    || neg.token_id.is_empty()
                    || pos.token_id == neg.token_id
                {
                    continue;
                }
                let outcomes = vec![
                    Outcome::new(TokenId::from(pos.token_id.clone()), positive_name),
                    Outcome::new(TokenId::from(neg.token_id.clone()), negative_name),
                ];
                let market = Market::new(
                    MarketId::from(info.id.clone()),
                    info.question.clone(),
                    outcomes,
                    payout,
                );
                markets.push(market);
            }
        }

        markets
    }
}

/// Fetcher for retrieving market listings from an exchange REST API.
///
/// # Thread Safety
///
/// Implementations must be thread-safe (`Send + Sync`).
///
/// # Errors
///
/// Methods return [`Error`] for network failures or API errors.
#[async_trait]
pub trait MarketFetcher: Send + Sync {
    /// Fetch active markets from the exchange.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of markets to return.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or returns invalid data.
    async fn get_markets(&self, limit: usize) -> Result<Vec<MarketInfo>, Error>;

    /// Return the exchange name for logging and debugging.
    fn exchange_name(&self) -> &'static str;
}

/// Event received from a real-time market data stream.
///
/// Represents the different types of updates that can arrive from an exchange
/// WebSocket connection.
#[derive(Debug, Clone)]
pub enum MarketEvent {
    /// Full order book snapshot for a token.
    ///
    /// Received on initial subscription or after reconnection. Contains the
    /// complete order book state.
    BookSnapshot {
        /// Token identifier this order book belongs to.
        token_id: TokenId,

        /// Complete order book state.
        book: Book,
    },

    /// Incremental order book update.
    ///
    /// Contains only the changes since the last update. Apply these deltas
    /// to maintain a local order book copy.
    BookDelta {
        /// Token identifier this update applies to.
        token_id: TokenId,

        /// Order book changes (price levels to update or remove).
        book: Book,
    },

    /// Market settlement notification.
    ///
    /// Indicates that a prediction market has resolved and shares are being
    /// paid out.
    MarketSettled {
        /// Identifier of the settled market.
        market_id: crate::domain::id::MarketId,

        /// Name of the winning outcome.
        winning_outcome: String,

        /// Payout amount per winning share.
        payout_per_share: Decimal,
    },

    /// Connection successfully established.
    Connected,

    /// Connection lost.
    ///
    /// The stream may attempt automatic reconnection depending on
    /// implementation.
    Disconnected {
        /// Human-readable description of why the connection was lost.
        reason: String,
    },
}

impl MarketEvent {
    /// Return the token identifier if this event contains market data.
    ///
    /// Returns `Some` for `BookSnapshot` and `BookDelta` events, `None` otherwise.
    #[must_use]
    pub fn token_id(&self) -> Option<&TokenId> {
        match self {
            Self::BookSnapshot { token_id, .. } => Some(token_id),
            Self::BookDelta { token_id, .. } => Some(token_id),
            _ => None,
        }
    }

    /// Return the order book if this event contains one.
    ///
    /// Returns `Some` for `BookSnapshot` and `BookDelta` events, `None` otherwise.
    #[must_use]
    pub fn order_book(&self) -> Option<&Book> {
        match self {
            Self::BookSnapshot { book, .. } => Some(book),
            Self::BookDelta { book, .. } => Some(book),
            _ => None,
        }
    }
}

/// Real-time market data stream from an exchange via WebSocket.
///
/// Implementations handle connection lifecycle management, subscription
/// management, and protocol-specific message parsing.
///
/// # Lifecycle
///
/// 1. Call [`connect`](Self::connect) to establish the connection
/// 2. Call [`subscribe`](Self::subscribe) to register for market updates
/// 3. Call [`next_event`](Self::next_event) in a loop to receive updates
///
/// # Errors
///
/// Connection and subscription methods return [`Error`] for network failures
/// or protocol errors.
#[async_trait]
pub trait MarketDataStream: Send {
    /// Establish a connection to the exchange's real-time data feed.
    ///
    /// Must be called before subscribing to markets.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established.
    async fn connect(&mut self) -> Result<(), Error>;

    /// Subscribe to market data for the specified tokens.
    ///
    /// # Arguments
    ///
    /// * `token_ids` - Token identifiers to subscribe to.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription request fails.
    async fn subscribe(&mut self, token_ids: &[TokenId]) -> Result<(), Error>;

    /// Receive the next market event from the stream.
    ///
    /// Blocks asynchronously until an event is available or the connection
    /// closes. Returns `None` when the stream is permanently closed.
    async fn next_event(&mut self) -> Option<MarketEvent>;

    /// Return the exchange name for logging and debugging.
    fn exchange_name(&self) -> &'static str;

    /// Return connection pool statistics if this stream uses connection pooling.
    ///
    /// Returns `None` for single-connection streams.
    fn pool_stats(&self) -> Option<PoolStats> {
        None
    }
}

/// Blanket implementation of [`MarketDataStream`] for boxed trait objects.
///
/// Enables use of `Box<dyn MarketDataStream>` with generic wrappers and
/// collection types.
#[async_trait]
impl MarketDataStream for Box<dyn MarketDataStream> {
    async fn connect(&mut self) -> Result<(), Error> {
        (**self).connect().await
    }

    async fn subscribe(&mut self, token_ids: &[TokenId]) -> Result<(), Error> {
        (**self).subscribe(token_ids).await
    }

    async fn next_event(&mut self) -> Option<MarketEvent> {
        (**self).next_event().await
    }

    fn exchange_name(&self) -> &'static str {
        (**self).exchange_name()
    }

    fn pool_stats(&self) -> Option<PoolStats> {
        (**self).pool_stats()
    }
}

/// Executor for multi-leg arbitrage opportunities.
///
/// Handles the complexity of placing multiple coordinated orders to capture
/// an arbitrage opportunity.
///
/// # Thread Safety
///
/// Implementations must be thread-safe (`Send + Sync`) to support concurrent
/// opportunity execution.
///
/// # Errors
///
/// Methods return [`Error`] for network failures, partial fills, or exchange
/// rejections.
#[async_trait]
pub trait ArbitrageExecutor: Send + Sync {
    /// Authoritative token payouts for markets that resolved while disconnected.
    async fn settlement_payouts(
        &self,
        _positions: &[crate::domain::position::Position],
    ) -> Result<Vec<(TokenId, Decimal)>, Error> {
        Ok(Vec::new())
    }

    /// Stable chain and signer identity for durable account binding.
    fn account_identity(&self) -> Option<String> {
        None
    }

    /// Verify account inventory against the restored ledger before enabling trading.
    async fn verify_inventory(
        &self,
        _positions: &[crate::domain::position::Position],
    ) -> Result<(), Error> {
        Err(Error::Parse(
            "Executor does not implement account reconciliation".into(),
        ))
    }

    /// Execute an arbitrage opportunity by placing orders on all legs.
    ///
    /// # Arguments
    ///
    /// * `opportunity` - The detected arbitrage opportunity to execute.
    ///
    /// # Errors
    ///
    /// Returns an error if any leg fails to execute. Partial execution results
    /// are captured in [`TradeResult::Partial`].
    async fn execute_arbitrage(&self, opportunity: &Opportunity) -> Result<TradeResult, Error>;

    /// Cancel an order by its identifier.
    ///
    /// # Arguments
    ///
    /// * `order_id` - Identifier of the order to cancel.
    ///
    /// # Errors
    ///
    /// Returns an error if the order cannot be found or cancellation fails.
    async fn cancel(&self, order_id: &OrderId) -> Result<(), Error>;

    /// Return the exchange name for logging and debugging.
    fn exchange_name(&self) -> &'static str;
}
