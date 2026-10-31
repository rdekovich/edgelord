//! Opportunity types for arbitrage detection.
//!
//! An [`Opportunity`] represents a detected arbitrage situation where buying
//! all outcomes costs less than the guaranteed payout. Each opportunity has
//! multiple [`OpportunityLeg`]s representing the individual purchases needed.
//!
//! # Edge Calculation
//!
//! The "edge" is the profit per share: `payout - total_cost`.
//! For example, if YES costs $0.45 and NO costs $0.50 with a $1.00 payout,
//! the edge is $1.00 - $0.95 = $0.05 per share.
//!
//! # Examples
//!
//! Detecting a simple binary arbitrage:
//!
//! ```
//! use edgelord::domain::opportunity::{Opportunity, OpportunityLeg};
//! use edgelord::domain::id::{MarketId, TokenId};
//! use rust_decimal_macros::dec;
//!
//! let legs = vec![
//!     OpportunityLeg::new(TokenId::new("yes"), dec!(0.45)),
//!     OpportunityLeg::new(TokenId::new("no"), dec!(0.50)),
//! ];
//!
//! let opp = Opportunity::new(
//!     MarketId::new("market-1"),
//!     "Will it rain tomorrow?",
//!     legs,
//!     dec!(100),  // volume: 100 shares
//!     dec!(1.00), // payout: $1.00 per share
//! );
//!
//! assert_eq!(opp.total_cost(), dec!(0.95));
//! assert_eq!(opp.edge(), dec!(0.05));
//! assert_eq!(opp.expected_profit(), dec!(5.00)); // 100 * 0.05
//! ```

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::result::Result;

use super::error::DomainError;
use super::id::{MarketId, TokenId};
use super::money::Price;

/// A single leg of an opportunity representing one outcome to purchase.
///
/// Each leg captures the token ID and current ask price for one outcome
/// that must be purchased to complete the arbitrage.
///
/// # Examples
///
/// ```
/// use edgelord::domain::opportunity::OpportunityLeg;
/// use edgelord::domain::id::TokenId;
/// use rust_decimal_macros::dec;
///
/// let leg = OpportunityLeg::new(TokenId::new("yes-token"), dec!(0.45));
/// assert_eq!(leg.ask_price(), dec!(0.45));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpportunityLeg {
    /// Token ID of the outcome to purchase.
    token_id: TokenId,
    /// Current ask price for this outcome.
    ask_price: Price,
}

impl OpportunityLeg {
    /// Creates a new opportunity leg.
    #[must_use]
    pub fn new(token_id: TokenId, ask_price: Price) -> Self {
        Self {
            token_id,
            ask_price,
        }
    }

    /// Returns the token ID for this leg.
    #[must_use]
    pub fn token_id(&self) -> &TokenId {
        &self.token_id
    }

    /// Returns the ask price for this leg.
    #[must_use]
    pub fn ask_price(&self) -> Price {
        self.ask_price
    }
}

/// An arbitrage opportunity supporting any number of outcomes.
///
/// Represents a detected situation where buying all outcomes costs less
/// than the guaranteed payout. Uses market-provided payout instead of
/// assuming a hardcoded value.
///
/// Derived fields are calculated on access:
/// - [`total_cost`](Self::total_cost): sum of all leg prices
/// - [`edge`](Self::edge): payout minus total_cost (profit per share)
/// - [`expected_profit`](Self::expected_profit): edge times volume
///
/// # Examples
///
/// ```
/// use edgelord::domain::opportunity::{Opportunity, OpportunityLeg};
/// use edgelord::domain::id::{MarketId, TokenId};
/// use rust_decimal_macros::dec;
///
/// let legs = vec![
///     OpportunityLeg::new(TokenId::new("yes"), dec!(0.40)),
///     OpportunityLeg::new(TokenId::new("no"), dec!(0.50)),
/// ];
///
/// let opp = Opportunity::new(
///     MarketId::new("market-1"),
///     "Test market?",
///     legs,
///     dec!(100),
///     dec!(1.00),
/// );
///
/// assert_eq!(opp.edge(), dec!(0.10)); // 10 cent edge
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Opportunity {
    /// Market ID where the opportunity exists.
    market_id: MarketId,
    /// Human-readable market question.
    question: String,
    /// Individual legs to execute.
    legs: Vec<OpportunityLeg>,
    /// Number of shares to trade.
    volume: Decimal,
    /// Payout per share on resolution.
    payout: Decimal,
    /// Strategy that detected this opportunity.
    strategy: String,
}

impl Opportunity {
    /// Creates a new opportunity without validation.
    ///
    /// Use [`Opportunity::try_new`] for validated construction.
    #[must_use]
    pub fn new(
        market_id: MarketId,
        question: impl Into<String>,
        legs: Vec<OpportunityLeg>,
        volume: Decimal,
        payout: Decimal,
    ) -> Self {
        Self {
            market_id,
            question: question.into(),
            legs,
            volume,
            payout,
            strategy: String::new(),
        }
    }

    /// Creates a new opportunity with a strategy name.
    #[must_use]
    pub fn with_strategy(
        market_id: MarketId,
        question: impl Into<String>,
        legs: Vec<OpportunityLeg>,
        volume: Decimal,
        payout: Decimal,
        strategy: impl Into<String>,
    ) -> Self {
        Self {
            market_id,
            question: question.into(),
            legs,
            volume,
            payout,
            strategy: strategy.into(),
        }
    }

    /// Creates a new opportunity with domain invariant validation.
    ///
    /// # Domain Invariants
    ///
    /// - `legs` must not be empty
    /// - `volume` must be positive (greater than 0)
    /// - `payout` must be greater than the total cost of all legs
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::EmptyLegs`] if legs is empty.
    /// Returns [`DomainError::NonPositiveVolume`] if volume is zero or negative.
    /// Returns [`DomainError::PayoutNotGreaterThanCost`] if no edge exists.
    pub fn try_new(
        market_id: MarketId,
        question: impl Into<String>,
        legs: Vec<OpportunityLeg>,
        volume: Decimal,
        payout: Decimal,
    ) -> Result<Self, DomainError> {
        if legs.is_empty() {
            return Err(DomainError::EmptyLegs);
        }

        if volume <= Decimal::ZERO {
            return Err(DomainError::NonPositiveVolume { volume });
        }

        let total_cost: Decimal = legs.iter().map(|leg| leg.ask_price).sum();

        if payout <= total_cost {
            return Err(DomainError::PayoutNotGreaterThanCost {
                payout,
                cost: total_cost,
            });
        }

        Ok(Self {
            market_id,
            question: question.into(),
            legs,
            volume,
            payout,
            strategy: String::new(),
        })
    }

    /// Resize a basket after liquidity and risk validation.
    pub fn set_volume(&mut self, volume: Decimal) {
        self.volume = volume;
    }

    /// Returns the strategy name that detected this opportunity.
    #[must_use]
    pub fn strategy(&self) -> &str {
        &self.strategy
    }

    /// Returns the market ID.
    #[must_use]
    pub fn market_id(&self) -> &MarketId {
        &self.market_id
    }

    /// Returns the market question text.
    #[must_use]
    pub fn question(&self) -> &str {
        &self.question
    }

    /// Returns all legs of this opportunity.
    #[must_use]
    pub fn legs(&self) -> &[OpportunityLeg] {
        &self.legs
    }

    /// Returns the target volume in shares.
    #[must_use]
    pub fn volume(&self) -> Decimal {
        self.volume
    }

    /// Returns the payout per share on resolution.
    #[must_use]
    pub fn payout(&self) -> Decimal {
        self.payout
    }

    /// Calculates the total cost (sum of all leg prices).
    #[must_use]
    pub fn total_cost(&self) -> Decimal {
        self.legs.iter().map(|leg| leg.ask_price).sum()
    }

    /// Calculates the edge (payout minus total cost per share).
    #[must_use]
    pub fn edge(&self) -> Decimal {
        self.payout - self.total_cost()
    }

    /// Calculates the expected profit (edge times volume).
    #[must_use]
    pub fn expected_profit(&self) -> Decimal {
        self.edge() * self.volume
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_market_id() -> MarketId {
        MarketId::from("test-market")
    }

    fn make_token_id(name: &str) -> TokenId {
        TokenId::from(name)
    }

    #[test]
    fn leg_stores_token_and_price() {
        let leg = OpportunityLeg::new(make_token_id("outcome-a"), dec!(0.45));

        assert_eq!(leg.token_id().as_str(), "outcome-a");
        assert_eq!(leg.ask_price(), dec!(0.45));
    }

    #[test]
    fn two_legs_calculates_total_cost() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.40)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];

        let opp = Opportunity::new(
            make_market_id(),
            "Will it rain?",
            legs,
            dec!(100),
            dec!(1.0),
        );

        assert_eq!(opp.total_cost(), dec!(0.90));
    }

    #[test]
    fn edge_uses_payout_not_hardcoded_one() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.40)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];

        let opp = Opportunity::new(
            make_market_id(),
            "Will it rain?",
            legs,
            dec!(100),
            dec!(1.0),
        );

        assert_eq!(opp.edge(), dec!(0.10));
    }

    #[test]
    fn expected_profit_is_edge_times_volume() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.40)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];

        let opp = Opportunity::new(
            make_market_id(),
            "Will it rain?",
            legs,
            dec!(100),
            dec!(1.0),
        );

        assert_eq!(opp.expected_profit(), dec!(10.00));
    }

    #[test]
    fn custom_payout_affects_edge() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.80)),
            OpportunityLeg::new(make_token_id("no"), dec!(1.00)),
        ];

        let opp = Opportunity::new(
            make_market_id(),
            "Special market",
            legs,
            dec!(50),
            dec!(2.0),
        );

        assert_eq!(opp.total_cost(), dec!(1.80));
        assert_eq!(opp.edge(), dec!(0.20));
        assert_eq!(opp.expected_profit(), dec!(10.00));
    }

    #[test]
    fn three_outcome_market() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("candidate-a"), dec!(0.30)),
            OpportunityLeg::new(make_token_id("candidate-b"), dec!(0.35)),
            OpportunityLeg::new(make_token_id("candidate-c"), dec!(0.25)),
        ];

        let opp = Opportunity::new(
            make_market_id(),
            "Who will win?",
            legs,
            dec!(100),
            dec!(1.0),
        );

        assert_eq!(opp.total_cost(), dec!(0.90));
        assert_eq!(opp.edge(), dec!(0.10));
        assert_eq!(opp.expected_profit(), dec!(10.00));
    }

    #[test]
    fn negative_edge_when_overpriced() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.60)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];

        let opp = Opportunity::new(make_market_id(), "Overpriced", legs, dec!(100), dec!(1.0));

        assert_eq!(opp.total_cost(), dec!(1.10));
        assert_eq!(opp.edge(), dec!(-0.10));
        assert_eq!(opp.expected_profit(), dec!(-10.00));
    }

    #[test]
    fn accessors_return_correct_values() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.40)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];

        let opp = Opportunity::new(
            make_market_id(),
            "Will it rain?",
            legs,
            dec!(100),
            dec!(1.0),
        );

        assert_eq!(opp.market_id().as_str(), "test-market");
        assert_eq!(opp.question(), "Will it rain?");
        assert_eq!(opp.legs().len(), 2);
        assert_eq!(opp.volume(), dec!(100));
        assert_eq!(opp.payout(), dec!(1.0));
    }

    #[test]
    fn opportunity_try_new_accepts_valid_inputs() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.40)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];

        let opp = Opportunity::try_new(make_market_id(), "Valid", legs, dec!(100), dec!(1.00));
        assert!(opp.is_ok());
    }

    #[test]
    fn opportunity_rejects_empty_legs() {
        let result = Opportunity::try_new(make_market_id(), "Test", vec![], dec!(100), dec!(1.0));
        assert!(matches!(result, Err(DomainError::EmptyLegs)));
    }

    #[test]
    fn opportunity_rejects_non_positive_volume() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.40)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];

        // Zero volume should fail
        let result =
            Opportunity::try_new(make_market_id(), "Test", legs.clone(), dec!(0), dec!(1.0));
        assert!(result.is_err());

        // Negative volume should fail
        let result = Opportunity::try_new(make_market_id(), "Test", legs, dec!(-10), dec!(1.0));
        assert!(result.is_err());
    }

    #[test]
    fn opportunity_rejects_payout_not_greater_than_cost() {
        let legs = vec![
            OpportunityLeg::new(make_token_id("yes"), dec!(0.40)),
            OpportunityLeg::new(make_token_id("no"), dec!(0.50)),
        ];
        // Total cost is 0.90, payout must be > 0.90

        // Payout equal to cost should fail
        let result = Opportunity::try_new(
            make_market_id(),
            "Test",
            legs.clone(),
            dec!(100),
            dec!(0.90),
        );
        assert!(result.is_err());

        // Payout less than cost should fail
        let result = Opportunity::try_new(make_market_id(), "Test", legs, dec!(100), dec!(0.80));
        assert!(result.is_err());
    }
}
