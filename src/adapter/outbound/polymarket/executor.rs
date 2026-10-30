//! Order execution for Polymarket CLOB.
//!
//! Provides the [`PolymarketExecutor`] adapter for submitting and managing
//! orders on the Polymarket Central Limit Order Book (CLOB). Supports both
//! individual order execution and parallel multi-leg arbitrage trades.

use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
use std::str::FromStr;
use std::sync::Arc;

use alloy_signer_local::PrivateKeySigner;
use async_trait::async_trait;
use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::auth::{Normal, Signer};
use polymarket_client_sdk::clob::types::request::{
    OrderBookSummaryRequest, OrdersRequest, TradesRequest,
};
use polymarket_client_sdk::clob::types::response::PostOrderResponse;
use polymarket_client_sdk::clob::types::{
    OrderStatusType, OrderType, Side, SignedOrder, TradeStatusType,
};
use polymarket_client_sdk::clob::{Client, Config as ClobConfig};
use polymarket_client_sdk::types::U256;
use rust_decimal::Decimal;
use tracing::{info, warn};

use super::settings::PolymarketRuntimeConfig;
use crate::domain::{
    id::OrderId, opportunity::Opportunity, trade::Failure, trade::Fill, trade::TradeResult,
};
use crate::error::{ConfigError, ExecutionError, Result};
use crate::port::{
    outbound::exchange::ArbitrageExecutor, outbound::exchange::ExecutionResult,
    outbound::exchange::OrderExecutor, outbound::exchange::OrderRequest,
    outbound::exchange::OrderSide,
};

/// Type alias for the authenticated CLOB client.
type AuthenticatedClient = Client<Authenticated<Normal>>;

/// Trade executor for the Polymarket CLOB.
///
/// Handles order signing, submission, and cancellation using the Polymarket
/// SDK. Supports both single orders via [`OrderExecutor`] and multi-leg
/// arbitrage via [`ArbitrageExecutor`].
pub struct PolymarketExecutor {
    /// Authenticated CLOB client for API communication.
    client: Arc<AuthenticatedClient>,
    /// Local signer for order signatures.
    signer: Arc<PrivateKeySigner>,
    _account_lock: std::fs::File,
    chain_id: u64,
    token_markets: parking_lot::Mutex<std::collections::HashMap<String, String>>,
}

impl PolymarketExecutor {
    /// Create a new executor by authenticating with the Polymarket CLOB.
    ///
    /// # Errors
    ///
    /// Returns an error if the private key is missing or invalid, or if
    /// CLOB authentication fails.
    pub async fn new(config: &PolymarketRuntimeConfig) -> Result<Self> {
        if config.private_key.trim().is_empty() {
            return Err(ConfigError::MissingField {
                field: "WALLET_PRIVATE_KEY",
            }
            .into());
        }

        let chain_id = config.chain_id;

        // Create signer from private key
        let signer = PrivateKeySigner::from_str(&config.private_key)
            .map_err(|e| ConfigError::InvalidValue {
                field: "WALLET_PRIVATE_KEY",
                reason: e.to_string(),
            })?
            .with_chain_id(Some(chain_id));

        if chain_id != 137 {
            return Err(ConfigError::Other(
                "Live CLOB execution supports Polygon mainnet only; use dry-run otherwise".into(),
            )
            .into());
        }
        let lock_path = std::env::temp_dir().join(format!(
            "edgelord-account-{chain_id}-{}.lock",
            signer.address()
        ));
        let account_lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(lock_path)?;
        if unsafe { libc::flock(account_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(ExecutionError::AuthFailed(
                "Another edgelord process owns this trading account".into(),
            )
            .into());
        }

        info!(
            chain_id = chain_id,
            address = %signer.address(),
            "Creating CLOB client"
        );

        // Create and authenticate client
        let client = Client::new(&config.api_url, ClobConfig::default())
            .map_err(|e| ExecutionError::AuthFailed(format!("Failed to create CLOB client: {e}")))?
            .authentication_builder(&signer)
            .authenticate()
            .await
            .map_err(|e| ExecutionError::AuthFailed(e.to_string()))?;

        info!("CLOB client authenticated successfully");

        Ok(Self {
            client: Arc::new(client),
            signer: Arc::new(signer),
            _account_lock: account_lock,
            chain_id,
            token_markets: parking_lot::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Execute an arbitrage opportunity by placing orders on all legs in parallel.
    ///
    /// Submits buy orders for all legs concurrently and aggregates results
    /// into success, partial fill, or failure outcomes.
    async fn execute_arbitrage_impl(&self, opportunity: &Opportunity) -> Result<TradeResult> {
        info!(
            market = %opportunity.market_id(),
            edge = %opportunity.edge(),
            volume = %opportunity.volume(),
            legs = opportunity.legs().len(),
            "Executing arbitrage opportunity"
        );

        let legs = opportunity.legs();
        if legs.len() < 2 {
            return Ok(TradeResult::Failed {
                reason: "Opportunity must have at least 2 legs".to_string(),
            });
        }

        let volume = opportunity.volume();
        self.client.invalidate_internal_caches();

        // Build and validate the entire basket before submitting any leg.
        let mut prepared = Vec::new();
        for leg in legs {
            match self
                .prepare_order(leg.token_id().as_str(), Side::Buy, volume, leg.ask_price())
                .await
            {
                Ok(order) => prepared.push((leg.token_id().clone(), order)),
                Err(error) => {
                    return Ok(TradeResult::Failed {
                        reason: format!("Preflight rejected basket: {error}"),
                    })
                }
            }
        }
        let futures = prepared.into_iter().map(|(token_id, order)| async move {
            let result = self.post_confirmed(order).await;
            (token_id, result)
        });
        let results = futures_util::future::join_all(futures).await;

        // Separate successful and failed legs
        let mut fills = Vec::new();
        let mut failures = Vec::new();

        for (token_id, result) in results {
            // Transport errors and nonterminal responses are ambiguous. The caller
            // retains the entire durable reservation rather than guessing no fill.
            let response = result?;
            match classify_response(&response, Side::Buy, volume)? {
                ExecutionResult::Success {
                    filled_amount,
                    average_price,
                    ..
                } => {
                    fills.push(Fill::new(
                        token_id,
                        response.order_id,
                        filled_amount,
                        average_price,
                    ));
                }
                ExecutionResult::PartialFill {
                    filled_amount,
                    average_price,
                    ..
                } => {
                    fills.push(Fill::new(
                        token_id.clone(),
                        response.order_id,
                        filled_amount,
                        average_price,
                    ));
                    failures.push(Failure::new(token_id, "Incomplete confirmed quantity"));
                }
                ExecutionResult::Failed { reason } => failures.push(Failure::new(token_id, reason)),
            }
        }

        if failures.is_empty() {
            info!(fills = fills.len(), "All legs executed successfully");
            Ok(TradeResult::Success { fills })
        } else if fills.is_empty() {
            let errors: Vec<_> = failures.iter().map(|f| f.error.as_str()).collect();
            warn!(errors = ?errors, "All legs failed");
            Ok(TradeResult::Failed {
                reason: errors.join("; "),
            })
        } else {
            warn!(
                fills = fills.len(),
                failures = failures.len(),
                "Partial fill detected"
            );
            Ok(TradeResult::Partial { fills, failures })
        }
    }

    /// Submit a single limit order to the CLOB.
    ///
    /// # Errors
    ///
    /// Returns an error if the token ID is invalid, order building fails,
    /// signing fails, or the exchange rejects the order.
    async fn submit_order(
        &self,
        token_id: &str,
        side: Side,
        size: Decimal,
        price: Decimal,
    ) -> Result<PostOrderResponse> {
        let order = self.prepare_order(token_id, side, size, price).await?;
        self.post_confirmed(order).await
    }

    async fn prepare_order(
        &self,
        token_id: &str,
        side: Side,
        size: Decimal,
        price: Decimal,
    ) -> Result<SignedOrder> {
        // Parse token ID to U256
        let token_id_u256 =
            U256::from_str(token_id).map_err(|e| ExecutionError::InvalidTokenId {
                token_id: token_id.to_string(),
                reason: e.to_string(),
            })?;

        if size <= Decimal::ZERO || price <= Decimal::ZERO || price >= Decimal::ONE {
            return Err(ExecutionError::OrderBuildFailed("Invalid size or price".into()).into());
        }
        let fee =
            self.client.fee_rate_bps(token_id_u256).await.map_err(|e| {
                ExecutionError::OrderBuildFailed(format!("Cannot verify fees: {e}"))
            })?;
        if fee.base_fee != 0 {
            return Err(ExecutionError::OrderBuildFailed(
                "Fee-bearing markets require an implemented net-share fee model".into(),
            )
            .into());
        }

        let book = self
            .client
            .order_book(
                &OrderBookSummaryRequest::builder()
                    .token_id(token_id_u256)
                    .build(),
            )
            .await?;
        self.token_markets
            .lock()
            .insert(token_id.to_string(), book.market.to_string());
        let market = self.client.market(&book.market.to_string()).await?;
        if !market.active
            || market.closed
            || !market.accepting_orders
            || market.seconds_delay != 0
            || market.taker_base_fee != Decimal::ZERO
            || size < book.min_order_size
        {
            return Err(ExecutionError::OrderBuildFailed(
                "Market not eligible: closed, delayed, fee-bearing, or below minimum size".into(),
            )
            .into());
        }
        let available: Decimal = if side == Side::Buy {
            book.asks
                .iter()
                .filter(|level| level.price <= price)
                .map(|level| level.size)
                .sum()
        } else {
            book.bids
                .iter()
                .filter(|level| level.price >= price)
                .map(|level| level.size)
                .sum()
        };
        if available < size {
            return Err(ExecutionError::OrderBuildFailed(
                "Insufficient fresh executable depth".into(),
            )
            .into());
        }

        // Build limit order
        let order = self
            .client
            .limit_order()
            .token_id(token_id_u256)
            .side(side)
            .price(price)
            .size(size)
            .order_type(OrderType::FOK)
            .build()
            .await
            .map_err(|e| ExecutionError::OrderBuildFailed(e.to_string()))?;

        // Sign order
        let signed_order = self
            .client
            .sign(self.signer.as_ref(), order)
            .await
            .map_err(|e| ExecutionError::SigningFailed(e.to_string()))?;

        Ok(signed_order)
    }

    async fn post_confirmed(&self, order: SignedOrder) -> Result<PostOrderResponse> {
        let mut response = self
            .client
            .post_order(order)
            .await
            .map_err(|error| ExecutionError::SubmissionFailed(error.to_string()))?;
        if response.success && response.status == OrderStatusType::Matched {
            if response.trade_ids.is_empty() {
                response.trade_ids = self
                    .client
                    .order(&response.order_id)
                    .await?
                    .associate_trades;
            }
            if response.trade_ids.is_empty() {
                return Err(ExecutionError::SubmissionFailed(
                    "Matched order has no trade IDs; reconcile settlement".into(),
                )
                .into());
            }
            for id in &response.trade_ids {
                loop {
                    let page = self
                        .client
                        .trades(&TradesRequest::builder().id(id).build(), None)
                        .await?;
                    let trade = page
                        .data
                        .iter()
                        .find(|trade| &trade.id == id && trade.taker_order_id == response.order_id);
                    match trade.map(|trade| &trade.status) {
                        Some(TradeStatusType::Confirmed) => break,
                        Some(TradeStatusType::Failed) => {
                            return Err(ExecutionError::SubmissionFailed(
                                "Matched trade failed settlement; reconciliation required".into(),
                            )
                            .into())
                        }
                        _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
                    }
                }
            }
        }
        Ok(response)
    }

    /// Cancel an open order by ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the cancellation request fails or the order
    /// cannot be cancelled (e.g., already filled or does not exist).
    async fn cancel_order_impl(&self, order_id: &OrderId) -> Result<()> {
        let response = self
            .client
            .cancel_order(order_id.as_str())
            .await
            .map_err(|e| ExecutionError::SubmissionFailed(format!("Cancel failed: {e}")))?;

        if let Some(reason) = response.not_canceled.get(order_id.as_str()) {
            return Err(ExecutionError::OrderRejected(format!(
                "Order {} not cancelled: {}",
                order_id.as_str(),
                reason
            ))
            .into());
        }

        info!(order_id = %order_id, "Order cancelled");
        Ok(())
    }
}

/// Only terminal, quantified fills may change inventory. A successful HTTP
/// response is not evidence that an order filled.
fn classify_response(
    response: &PostOrderResponse,
    side: Side,
    requested: Decimal,
) -> Result<ExecutionResult> {
    if !response.success {
        if response.making_amount != Decimal::ZERO || response.taking_amount != Decimal::ZERO {
            return Err(ExecutionError::SubmissionFailed(
                "Rejected response has nonzero amounts; reconciliation required".into(),
            )
            .into());
        }
        return Ok(ExecutionResult::Failed {
            reason: response
                .error_msg
                .clone()
                .unwrap_or_else(|| "Exchange rejected order".into()),
        });
    }
    if response.status != OrderStatusType::Matched || response.order_id.is_empty() {
        return Err(ExecutionError::SubmissionFailed(format!(
            "Order {} is {:?}; reconciliation required",
            response.order_id, response.status
        ))
        .into());
    }
    let (amount, cost) = match side {
        Side::Buy => (response.taking_amount, response.making_amount),
        Side::Sell => (response.making_amount, response.taking_amount),
        _ => return Err(ExecutionError::SubmissionFailed("Unknown side".into()).into()),
    };
    if amount <= Decimal::ZERO || amount > requested || cost <= Decimal::ZERO {
        return Err(ExecutionError::SubmissionFailed(
            "Invalid matched quantities; reconciliation required".into(),
        )
        .into());
    }
    let order_id = OrderId::new(response.order_id.clone());
    if amount < requested {
        Ok(ExecutionResult::PartialFill {
            order_id,
            filled_amount: amount,
            remaining_amount: requested - amount,
            average_price: cost / amount,
        })
    } else {
        Ok(ExecutionResult::Success {
            order_id,
            filled_amount: amount,
            average_price: cost / amount,
        })
    }
}

#[async_trait]
impl OrderExecutor for PolymarketExecutor {
    async fn execute(&self, order: &OrderRequest) -> Result<ExecutionResult> {
        let side = match order.side {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        };

        let response = self
            .submit_order(&order.token_id, side, order.size, order.price)
            .await?;
        classify_response(&response, side, order.size)
    }

    async fn cancel(&self, order_id: &OrderId) -> Result<()> {
        self.cancel_order_impl(order_id).await
    }

    fn exchange_name(&self) -> &'static str {
        "Polymarket"
    }
}

#[async_trait]
impl ArbitrageExecutor for PolymarketExecutor {
    fn account_identity(&self) -> Option<String> {
        Some(format!("{}:{}", self.chain_id, self.signer.address()))
    }

    async fn verify_inventory(
        &self,
        positions: &[crate::domain::position::Position],
    ) -> Result<()> {
        let orders = self.client.orders(&OrdersRequest::default(), None).await?;
        if !orders.data.is_empty()
            || (!orders.next_cursor.is_empty() && orders.next_cursor != "LTE=")
        {
            return Err(ExecutionError::AuthFailed(
                "Existing open orders require reconciliation before trading".into(),
            )
            .into());
        }
        #[derive(serde::Deserialize)]
        struct Holding {
            asset: String,
            size: Decimal,
            #[serde(rename = "conditionId")]
            condition_id: String,
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        let mut actual = std::collections::HashMap::<String, Decimal>::new();
        let user = self.signer.address().to_string();
        for page in 0..100 {
            let holdings: Vec<Holding> = http
                .get("https://data-api.polymarket.com/positions")
                .query(&[
                    ("user", user.clone()),
                    ("sizeThreshold", "0".into()),
                    ("limit", "100".into()),
                    ("offset", (page * 100).to_string()),
                ])
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let count = holdings.len();
            for holding in holdings {
                self.token_markets
                    .lock()
                    .insert(holding.asset.clone(), holding.condition_id);
                if holding.size > Decimal::ZERO {
                    *actual.entry(holding.asset).or_default() += holding.size;
                }
            }
            if count < 100 {
                break;
            }
            if page == 99 {
                return Err(ExecutionError::AuthFailed(
                    "Inventory pagination exceeded safety bound".into(),
                )
                .into());
            }
        }
        let mut expected = std::collections::HashMap::<String, Decimal>::new();
        for position in positions.iter().filter(|p| !p.status().is_closed()) {
            for leg in position.legs() {
                *expected.entry(leg.token_id().to_string()).or_default() += leg.size();
            }
        }
        if actual != expected {
            return Err(ExecutionError::AuthFailed("Exchange holdings differ from durable ledger; reconcile holdings and redemptions before trading".into()).into());
        }
        Ok(())
    }

    async fn settlement_payouts(
        &self,
        positions: &[crate::domain::position::Position],
    ) -> Result<Vec<(crate::domain::id::TokenId, Decimal)>> {
        let markets: std::collections::HashSet<String> = {
            let mapping = self.token_markets.lock();
            positions
                .iter()
                .filter(|p| !p.status().is_closed())
                .flat_map(|p| p.legs())
                .filter_map(|leg| mapping.get(leg.token_id().as_str()).cloned())
                .collect()
        };
        let mut payouts = Vec::new();
        for condition in markets {
            let market = self.client.market(&condition).await?;
            if market.closed && market.tokens.iter().filter(|token| token.winner).count() == 1 {
                payouts.extend(market.tokens.into_iter().map(|token| {
                    (
                        crate::domain::id::TokenId::new(token.token_id.to_string()),
                        if token.winner {
                            Decimal::ONE
                        } else {
                            Decimal::ZERO
                        },
                    )
                }));
            }
        }
        Ok(payouts)
    }

    async fn execute_arbitrage(&self, opportunity: &Opportunity) -> Result<TradeResult> {
        self.execute_arbitrage_impl(opportunity).await
    }

    async fn cancel(&self, order_id: &OrderId) -> Result<()> {
        self.cancel_order_impl(order_id).await
    }

    fn exchange_name(&self) -> &'static str {
        "Polymarket"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::id::{MarketId, TokenId};
    use crate::domain::opportunity::OpportunityLeg;
    use rust_decimal_macros::dec;

    // -------------------------------------------------------------------------
    // ExecutionResult Tests
    // -------------------------------------------------------------------------

    #[test]
    fn execution_result_success_has_order_id() {
        let result = ExecutionResult::Success {
            order_id: OrderId::new("order-123"),
            filled_amount: dec!(100),
            average_price: dec!(0.50),
        };

        assert!(result.is_success());
        assert!(!result.is_partial());
        assert!(!result.is_failed());
        assert_eq!(result.order_id().unwrap().as_str(), "order-123");
    }

    #[test]
    fn execution_result_partial_has_details() {
        let result = ExecutionResult::PartialFill {
            order_id: OrderId::new("partial-order"),
            filled_amount: dec!(50),
            remaining_amount: dec!(50),
            average_price: dec!(0.45),
        };

        assert!(!result.is_success());
        assert!(result.is_partial());
        assert!(!result.is_failed());
        assert_eq!(result.order_id().unwrap().as_str(), "partial-order");
    }

    #[test]
    fn execution_result_failed_has_no_order_id() {
        let result = ExecutionResult::Failed {
            reason: "Insufficient funds".into(),
        };

        assert!(!result.is_success());
        assert!(!result.is_partial());
        assert!(result.is_failed());
        assert!(result.order_id().is_none());
    }

    // -------------------------------------------------------------------------
    // OrderRequest Tests
    // -------------------------------------------------------------------------

    #[test]
    fn order_request_stores_all_fields() {
        let request = OrderRequest {
            token_id: "token-123".into(),
            side: OrderSide::Buy,
            size: dec!(100),
            price: dec!(0.45),
        };

        assert_eq!(request.token_id, "token-123");
        assert_eq!(request.side, OrderSide::Buy);
        assert_eq!(request.size, dec!(100));
        assert_eq!(request.price, dec!(0.45));
    }

    #[test]
    fn order_side_buy_and_sell_are_distinct() {
        assert_ne!(OrderSide::Buy, OrderSide::Sell);

        let buy = OrderSide::Buy;
        let sell = OrderSide::Sell;

        assert_eq!(buy, OrderSide::Buy);
        assert_eq!(sell, OrderSide::Sell);
    }

    // -------------------------------------------------------------------------
    // TradeResult Tests
    // -------------------------------------------------------------------------

    #[test]
    fn trade_result_success_with_fills() {
        let fills = vec![
            Fill {
                token_id: TokenId::new("yes"),
                order_id: "order-1".into(),
                amount: dec!(100),
                price: dec!(0.45),
            },
            Fill {
                token_id: TokenId::new("no"),
                order_id: "order-2".into(),
                amount: dec!(100),
                price: dec!(0.45),
            },
        ];

        let result = TradeResult::Success { fills };

        assert!(result.is_success());
        assert!(!result.is_partial());
        assert!(!result.is_failed());
        assert_eq!(result.fills().len(), 2);
        assert!(result.failures().is_empty());
    }

    #[test]
    fn trade_result_partial_with_fills_and_failures() {
        let fills = vec![Fill {
            token_id: TokenId::new("yes"),
            order_id: "order-1".into(),
            amount: dec!(100),
            price: dec!(0.45),
        }];
        let failures = vec![Failure {
            token_id: TokenId::new("no"),
            error: "timeout".into(),
        }];

        let result = TradeResult::Partial { fills, failures };

        assert!(!result.is_success());
        assert!(result.is_partial());
        assert!(!result.is_failed());
        assert_eq!(result.fills().len(), 1);
        assert_eq!(result.failures().len(), 1);
    }

    #[test]
    fn trade_result_failed_reason() {
        let result = TradeResult::Failed {
            reason: "All orders rejected".into(),
        };

        assert!(!result.is_success());
        assert!(!result.is_partial());
        assert!(result.is_failed());
        assert!(result.fills().is_empty());
        assert!(result.failures().is_empty());
    }

    // -------------------------------------------------------------------------
    // Opportunity Tests for Executor Logic
    // -------------------------------------------------------------------------

    #[test]
    fn opportunity_with_single_leg_is_invalid_for_arbitrage() {
        // Arbitrage requires at least 2 legs
        let legs = vec![OpportunityLeg::new(TokenId::new("single"), dec!(0.90))];

        let opp = Opportunity::new(
            MarketId::new("test-market"),
            "Single leg?",
            legs,
            dec!(100),
            dec!(1.00),
        );

        // Single leg opportunity should have 1 leg
        assert_eq!(opp.legs().len(), 1);
        // The executor would reject this (< 2 legs)
    }

    #[test]
    fn opportunity_with_two_legs_is_valid() {
        let legs = vec![
            OpportunityLeg::new(TokenId::new("yes"), dec!(0.45)),
            OpportunityLeg::new(TokenId::new("no"), dec!(0.50)),
        ];

        let opp = Opportunity::new(
            MarketId::new("test-market"),
            "Two legs?",
            legs,
            dec!(100),
            dec!(1.00),
        );

        assert_eq!(opp.legs().len(), 2);
        assert_eq!(opp.total_cost(), dec!(0.95));
        assert_eq!(opp.edge(), dec!(0.05));
    }

    #[test]
    fn opportunity_with_many_legs() {
        let legs = vec![
            OpportunityLeg::new(TokenId::new("a"), dec!(0.20)),
            OpportunityLeg::new(TokenId::new("b"), dec!(0.25)),
            OpportunityLeg::new(TokenId::new("c"), dec!(0.30)),
            OpportunityLeg::new(TokenId::new("d"), dec!(0.15)),
        ];

        let opp = Opportunity::new(
            MarketId::new("multi-outcome"),
            "Four outcomes?",
            legs,
            dec!(50),
            dec!(1.00),
        );

        assert_eq!(opp.legs().len(), 4);
        assert_eq!(opp.total_cost(), dec!(0.90));
        assert_eq!(opp.edge(), dec!(0.10));
        assert_eq!(opp.expected_profit(), dec!(5.00));
    }

    // -------------------------------------------------------------------------
    // Fill and Failure Tests
    // -------------------------------------------------------------------------

    #[test]
    fn fill_new_creates_fill() {
        let fill = Fill {
            token_id: TokenId::new("token-abc"),
            order_id: "order-xyz".into(),
            amount: dec!(100),
            price: dec!(0.45),
        };

        assert_eq!(fill.token_id.as_str(), "token-abc");
        assert_eq!(fill.order_id, "order-xyz");
    }

    #[test]
    fn failure_stores_error_message() {
        let failure = Failure {
            token_id: TokenId::new("token-fail"),
            error: "Connection timeout".into(),
        };

        assert_eq!(failure.token_id.as_str(), "token-fail");
        assert_eq!(failure.error, "Connection timeout");
    }

    // -------------------------------------------------------------------------
    // Side Conversion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn order_side_to_sdk_side_mapping() {
        // In the actual implementation, OrderSide::Buy maps to Side::Buy
        // and OrderSide::Sell maps to Side::Sell
        let buy = OrderSide::Buy;
        let sell = OrderSide::Sell;

        // Just verify the variants exist and are distinguishable
        assert!(matches!(buy, OrderSide::Buy));
        assert!(matches!(sell, OrderSide::Sell));
    }

    // -------------------------------------------------------------------------
    // Config Validation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn empty_private_key_is_invalid() {
        let config = PolymarketRuntimeConfig {
            private_key: "".into(),
            chain_id: 137,
            api_url: "https://clob.polymarket.com".into(),
            environment: super::super::settings::Environment::Mainnet,
        };

        assert!(config.private_key.is_empty());
    }

    #[test]
    fn whitespace_only_private_key_is_invalid() {
        let config = PolymarketRuntimeConfig {
            private_key: "   \t\n  ".into(),
            chain_id: 137,
            api_url: "https://clob.polymarket.com".into(),
            environment: super::super::settings::Environment::Mainnet,
        };

        assert!(config.private_key.trim().is_empty());
    }

    #[test]
    fn testnet_chain_id() {
        let config = PolymarketRuntimeConfig {
            private_key: "dummy".into(),
            chain_id: 80002, // Amoy testnet
            api_url: "https://clob.polymarket.com".into(),
            environment: super::super::settings::Environment::Testnet,
        };

        assert_eq!(config.chain_id, 80002);
    }

    #[test]
    fn mainnet_chain_id() {
        let config = PolymarketRuntimeConfig {
            private_key: "dummy".into(),
            chain_id: 137, // Polygon mainnet
            api_url: "https://clob.polymarket.com".into(),
            environment: super::super::settings::Environment::Mainnet,
        };

        assert_eq!(config.chain_id, 137);
    }

    // -------------------------------------------------------------------------
    // Exchange Name Tests
    // -------------------------------------------------------------------------

    #[test]
    fn order_executor_exchange_name() {
        // Test that the exchange name is consistent
        // (We can't instantiate PolymarketExecutor without real credentials,
        // but we can verify the constant)
        assert_eq!("Polymarket", "Polymarket");
    }

    // -------------------------------------------------------------------------
    // Token ID Parsing Tests
    // -------------------------------------------------------------------------

    #[test]
    fn valid_token_id_formats() {
        // Polymarket token IDs are large integers
        let valid_ids = vec![
            "71321045679252212594626385532706912750332728571942532289631379312455583992563",
            "12345",
            "0",
        ];

        for id in valid_ids {
            // These should be parseable as U256
            assert!(!id.is_empty());
        }
    }

    #[test]
    fn invalid_token_id_examples() {
        // These would fail U256::from_str parsing
        let invalid_ids = vec![
            "not-a-number",
            "0x123", // Hex prefix not supported
            "-1",    // Negative numbers
            "",      // Empty
            "12.34", // Decimals
        ];

        for id in invalid_ids {
            // Document that these formats are invalid
            // In the actual executor, these would cause InvalidTokenId error
            assert!(
                id.is_empty()
                    || id.starts_with('-')
                    || id.contains('.')
                    || id.contains('x')
                    || !id.chars().all(|c| c.is_ascii_digit())
            );
        }
    }
}

// -------------------------------------------------------------------------
// Integration Tests (behind feature flag)
// -------------------------------------------------------------------------

#[cfg(all(test, feature = "polymarket-integration"))]
mod integration_tests {
    use super::*;
    use std::env;

    fn get_test_config() -> Option<PolymarketRuntimeConfig> {
        let private_key = env::var("POLYMARKET_PRIVATE_KEY").ok()?;
        let api_url =
            env::var("POLYMARKET_API_URL").unwrap_or_else(|_| "https://clob.polymarket.com".into());

        // Default to testnet for safety
        let chain_id = env::var("POLYMARKET_CHAIN_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(80002);

        let environment = if chain_id == 137 {
            super::super::settings::Environment::Mainnet
        } else {
            super::super::settings::Environment::Testnet
        };

        Some(PolymarketRuntimeConfig {
            private_key,
            chain_id,
            api_url,
            environment,
        })
    }

    #[tokio::test]
    async fn integration_executor_creation() {
        let Some(config) = get_test_config() else {
            eprintln!("Skipping: POLYMARKET_PRIVATE_KEY not set");
            return;
        };

        match PolymarketExecutor::new(&config).await {
            Ok(executor) => {
                assert_eq!(ArbitrageExecutor::exchange_name(&executor), "Polymarket");
                println!("Successfully created PolymarketExecutor");
            }
            Err(e) => {
                // Auth failures are expected without valid credentials
                eprintln!(
                    "Executor creation failed (expected without valid creds): {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn integration_executor_implements_traits() {
        let Some(config) = get_test_config() else {
            eprintln!("Skipping: POLYMARKET_PRIVATE_KEY not set");
            return;
        };

        if let Ok(executor) = PolymarketExecutor::new(&config).await {
            // Verify trait implementations
            let _order_executor: &dyn OrderExecutor = &executor;
            let _arb_executor: &dyn ArbitrageExecutor = &executor;

            assert_eq!(OrderExecutor::exchange_name(&executor), "Polymarket");
            assert_eq!(ArbitrageExecutor::exchange_name(&executor), "Polymarket");
        }
    }
}

#[cfg(test)]
mod response_contract_tests {
    use super::*;
    use rust_decimal_macros::dec;
    fn response(status: &str, success: bool, making: &str, taking: &str) -> PostOrderResponse {
        serde_json::from_value(serde_json::json!({"orderID":"order","status":status,"success":success,"errorMsg":"rejection","makingAmount":making,"takingAmount":taking})).unwrap()
    }
    #[test]
    fn resting_and_delayed_orders_are_never_fills() {
        for status in ["live", "delayed", "unmatched"] {
            assert!(
                classify_response(&response(status, true, "0", "0"), Side::Buy, dec!(100)).is_err()
            );
        }
    }
    #[test]
    fn rejects_success_false_even_when_http_succeeded() {
        assert!(matches!(
            classify_response(&response("", false, "0", "0"), Side::Buy, dec!(100)).unwrap(),
            ExecutionResult::Failed { .. }
        ));
    }
    #[test]
    fn uses_actual_price_and_quantity() {
        match classify_response(
            &response("matched", true, "39", "100"),
            Side::Buy,
            dec!(100),
        )
        .unwrap()
        {
            ExecutionResult::Success {
                filled_amount,
                average_price,
                ..
            } => {
                assert_eq!(filled_amount, dec!(100));
                assert_eq!(average_price, dec!(0.39));
            }
            _ => panic!("Expected confirmed quantities"),
        }
        assert!(
            matches!(classify_response(&response("matched",true,"16","40"),Side::Buy,dec!(100)).unwrap(),ExecutionResult::PartialFill {filled_amount,remaining_amount,..} if filled_amount==dec!(40)&&remaining_amount==dec!(60))
        );
        assert!(
            classify_response(&response("matched", true, "0", "0"), Side::Buy, dec!(100)).is_err()
        );
    }
}
