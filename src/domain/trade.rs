//! Trade execution result types.
//!
//! This module provides types for representing the outcome of executing
//! multi-leg arbitrage trades:
//!
//! - [`TradeResult`] - Overall outcome of a trade execution
//! - [`Fill`] - A successfully executed leg
//! - [`Failure`] - A failed leg with error information
//!
//! # Trade Outcomes
//!
//! Multi-leg trades can have three outcomes:
//! - **Success**: All legs filled, arbitrage position established
//! - **Partial**: Some legs filled, risk exposure exists
//! - **Failed**: No legs filled, no position opened
//!
//! # Examples
//!
//! Handling trade results:
//!
//! ```
//! use edgelord::domain::trade::{TradeResult, Fill, Failure};
//! use edgelord::domain::id::TokenId;
//!
//! // Successful trade
//! let result = TradeResult::Success {
//!     fills: vec![
//!         Fill::new(TokenId::new("yes"), "order-1", rust_decimal::Decimal::from(100), rust_decimal::Decimal::new(45, 2)),
//!         Fill::new(TokenId::new("no"), "order-2", rust_decimal::Decimal::from(100), rust_decimal::Decimal::new(45, 2)),
//!     ],
//! };
//!
//! assert!(result.is_success());
//! assert_eq!(result.fills().len(), 2);
//!
//! // Partial fill (risk exposure)
//! let partial = TradeResult::Partial {
//!     fills: vec![Fill::new(TokenId::new("yes"), "order-1", rust_decimal::Decimal::from(100), rust_decimal::Decimal::new(45, 2))],
//!     failures: vec![Failure::new(TokenId::new("no"), "insufficient liquidity")],
//! };
//!
//! assert!(partial.is_partial());
//! ```

use super::id::TokenId;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A successfully executed leg in a multi-leg trade.
///
/// Contains the token ID and exchange-assigned order ID for tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    /// Token ID of the filled outcome.
    pub token_id: TokenId,
    /// Order ID assigned by the exchange.
    pub order_id: String,
    /// Confirmed quantity acquired.
    pub amount: Decimal,
    /// Actual average execution price.
    pub price: Decimal,
}

impl Fill {
    /// Creates a new fill record.
    pub fn new(
        token_id: TokenId,
        order_id: impl Into<String>,
        amount: Decimal,
        price: Decimal,
    ) -> Self {
        Self {
            token_id,
            order_id: order_id.into(),
            amount,
            price,
        }
    }
}

/// A failed leg in a multi-leg trade.
///
/// Contains the token ID and error message describing the failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    /// Token ID of the outcome that failed to fill.
    pub token_id: TokenId,
    /// Human-readable error message.
    pub error: String,
}

impl Failure {
    /// Creates a new failure record.
    pub fn new(token_id: TokenId, error: impl Into<String>) -> Self {
        Self {
            token_id,
            error: error.into(),
        }
    }
}

/// Result of executing a multi-leg arbitrage trade.
///
/// Captures the outcome of attempting to fill all legs of an arbitrage trade.
/// Partial fills create risk exposure that may need to be hedged or unwound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TradeResult {
    /// All legs executed successfully, arbitrage position established.
    Success {
        /// All successfully filled legs.
        fills: Vec<Fill>,
    },
    /// Some legs executed but not all, creating directional exposure.
    Partial {
        /// Legs that were successfully filled.
        fills: Vec<Fill>,
        /// Legs that failed to execute.
        failures: Vec<Failure>,
    },
    /// All legs failed, no position opened.
    Failed {
        /// Description of why the trade failed.
        reason: String,
    },
}

impl TradeResult {
    /// Validate exchange results before changing inventory or replaying a ledger.
    pub fn is_consistent_with(&self, opportunity: &super::opportunity::Opportunity) -> bool {
        let mut tokens = std::collections::HashSet::new();
        let valid = self.fills().iter().all(|fill| {
            tokens.insert(fill.token_id.clone())
                && !fill.order_id.is_empty()
                && fill.amount > Decimal::ZERO
                && fill.amount <= opportunity.volume()
                && opportunity.legs().iter().any(|leg| {
                    leg.token_id() == &fill.token_id
                        && fill.price > Decimal::ZERO
                        && fill.price <= leg.ask_price()
                })
        });
        valid
            && (!self.is_success()
                || (self.fills().len() == opportunity.legs().len()
                    && self
                        .fills()
                        .iter()
                        .all(|fill| fill.amount == opportunity.volume())))
            && (!self.is_partial() || !self.fills().is_empty())
    }

    /// Returns true if all legs were successfully filled.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Returns true if some but not all legs were filled.
    #[must_use]
    pub const fn is_partial(&self) -> bool {
        matches!(self, Self::Partial { .. })
    }

    /// Returns true if all legs failed to execute.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// Returns all successful fills, or an empty slice if none.
    #[must_use]
    pub fn fills(&self) -> &[Fill] {
        match self {
            Self::Success { fills } => fills,
            Self::Partial { fills, .. } => fills,
            Self::Failed { .. } => &[],
        }
    }

    /// Returns all failures, or an empty slice if none.
    #[must_use]
    pub fn failures(&self) -> &[Failure] {
        match self {
            Self::Success { .. } => &[],
            Self::Partial { failures, .. } => failures,
            Self::Failed { .. } => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_new() {
        let fill = Fill::new(
            TokenId::from("token-1"),
            "order-1",
            rust_decimal::Decimal::from(100),
            rust_decimal::Decimal::new(45, 2),
        );
        assert_eq!(fill.token_id.as_str(), "token-1");
        assert_eq!(fill.order_id, "order-1");
    }

    #[test]
    fn failure_new() {
        let failure = Failure::new(TokenId::from("token-2"), "insufficient funds");
        assert_eq!(failure.token_id.as_str(), "token-2");
        assert_eq!(failure.error, "insufficient funds");
    }

    #[test]
    fn result_success() {
        let result = TradeResult::Success {
            fills: vec![Fill::new(
                TokenId::from("t1"),
                "o1",
                rust_decimal::Decimal::from(100),
                rust_decimal::Decimal::new(45, 2),
            )],
        };
        assert!(result.is_success());
        assert!(!result.is_partial());
        assert!(!result.is_failed());
        assert_eq!(result.fills().len(), 1);
        assert!(result.failures().is_empty());
    }

    #[test]
    fn result_partial() {
        let result = TradeResult::Partial {
            fills: vec![Fill::new(
                TokenId::from("t1"),
                "o1",
                rust_decimal::Decimal::from(100),
                rust_decimal::Decimal::new(45, 2),
            )],
            failures: vec![Failure::new(TokenId::from("t2"), "error")],
        };
        assert!(!result.is_success());
        assert!(result.is_partial());
        assert!(!result.is_failed());
        assert_eq!(result.fills().len(), 1);
        assert_eq!(result.failures().len(), 1);
    }

    #[test]
    fn result_failed() {
        let result = TradeResult::Failed {
            reason: "all legs failed".to_string(),
        };
        assert!(!result.is_success());
        assert!(!result.is_partial());
        assert!(result.is_failed());
        assert!(result.fills().is_empty());
        assert!(result.failures().is_empty());
    }
}
