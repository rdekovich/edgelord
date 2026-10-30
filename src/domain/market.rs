//! Market-related domain types.
//!
//! This module provides the core market representation used throughout the system:
//!
//! - [`Market`] - A prediction market with N outcomes and configurable payout
//! - [`Outcome`] - A single tradeable outcome within a market
//! - [`MarketRegistry`] - Index of markets by token ID and market ID
//!
//! # Examples
//!
//! Creating a binary market:
//!
//! ```
//! use edgelord::domain::market::{Market, Outcome};
//! use edgelord::domain::id::{MarketId, TokenId};
//! use rust_decimal_macros::dec;
//!
//! let outcomes = vec![
//!     Outcome::new(TokenId::new("yes-token"), "Yes"),
//!     Outcome::new(TokenId::new("no-token"), "No"),
//! ];
//!
//! let market = Market::new(
//!     MarketId::new("will-it-rain"),
//!     "Will it rain tomorrow?",
//!     outcomes,
//!     dec!(1.00),
//! );
//!
//! assert!(market.is_binary());
//! assert_eq!(market.outcome_count(), 2);
//! ```

use std::collections::HashMap;
use std::result::Result;

use rust_decimal::Decimal;

use super::error::DomainError;
use super::id::{MarketId, TokenId};

/// A single tradeable outcome within a prediction market.
///
/// Each outcome has a unique token ID used for trading operations and a
/// human-readable name for display. For binary markets, typical names are
/// "Yes" and "No". For multi-outcome markets, names might be candidate names,
/// team names, or other descriptive labels.
///
/// # Examples
///
/// ```
/// use edgelord::domain::market::Outcome;
/// use edgelord::domain::id::TokenId;
///
/// let outcome = Outcome::new(TokenId::new("candidate-a"), "Candidate A");
/// assert_eq!(outcome.name(), "Candidate A");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The unique token identifier for trading this outcome.
    token_id: TokenId,
    /// Human-readable name for display purposes.
    name: String,
}

impl Outcome {
    /// Creates a new outcome with the given token ID and name.
    pub fn new(token_id: TokenId, name: impl Into<String>) -> Self {
        Self {
            token_id,
            name: name.into(),
        }
    }

    /// Returns the token ID for this outcome.
    #[must_use]
    pub const fn token_id(&self) -> &TokenId {
        &self.token_id
    }

    /// Returns the human-readable name of this outcome.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A prediction market supporting N outcomes with configurable payout.
///
/// This is the core market representation used throughout the system. It supports:
///
/// - **Binary markets** (2 outcomes, e.g., Yes/No)
/// - **Multi-outcome markets** (3+ outcomes, e.g., election candidates)
///
/// The `payout` field specifies how much one share pays out on resolution.
/// For Polymarket, this is $1.00. Other exchanges may use different values.
///
/// # Examples
///
/// Creating a binary market:
///
/// ```
/// use edgelord::domain::market::{Market, Outcome};
/// use edgelord::domain::id::{MarketId, TokenId};
/// use rust_decimal_macros::dec;
///
/// let market = Market::new(
///     MarketId::new("market-123"),
///     "Will it rain tomorrow?",
///     vec![
///         Outcome::new(TokenId::new("yes-token"), "Yes"),
///         Outcome::new(TokenId::new("no-token"), "No"),
///     ],
///     dec!(1.00),
/// );
///
/// assert!(market.is_binary());
/// ```
///
/// Creating a multi-outcome market:
///
/// ```
/// use edgelord::domain::market::{Market, Outcome};
/// use edgelord::domain::id::{MarketId, TokenId};
/// use rust_decimal_macros::dec;
///
/// let market = Market::new(
///     MarketId::new("election-2024"),
///     "Who will win the election?",
///     vec![
///         Outcome::new(TokenId::new("candidate-a"), "Candidate A"),
///         Outcome::new(TokenId::new("candidate-b"), "Candidate B"),
///         Outcome::new(TokenId::new("candidate-c"), "Candidate C"),
///     ],
///     dec!(1.00),
/// );
///
/// assert!(!market.is_binary());
/// assert_eq!(market.outcome_count(), 3);
/// ```
#[derive(Debug, Clone)]
pub struct Market {
    /// Unique identifier for this market.
    market_id: MarketId,
    /// The question this market is predicting.
    question: String,
    /// All possible outcomes for this market.
    outcomes: Vec<Outcome>,
    /// Amount paid out per share on correct resolution.
    payout: Decimal,
}

impl Market {
    /// Creates a new market with the given parameters.
    ///
    /// This constructor does not validate invariants. Use [`Market::try_new`]
    /// for validated construction.
    pub fn new(
        market_id: MarketId,
        question: impl Into<String>,
        outcomes: Vec<Outcome>,
        payout: Decimal,
    ) -> Self {
        Self {
            market_id,
            question: question.into(),
            outcomes,
            payout,
        }
    }

    /// Creates a new market with domain invariant validation.
    ///
    /// # Domain Invariants
    ///
    /// - `outcomes` must not be empty
    /// - `payout` must be positive (greater than 0)
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::EmptyOutcomes`] if outcomes is empty.
    /// Returns [`DomainError::NonPositivePayout`] if payout is zero or negative.
    pub fn try_new(
        market_id: MarketId,
        question: impl Into<String>,
        outcomes: Vec<Outcome>,
        payout: Decimal,
    ) -> Result<Self, DomainError> {
        if outcomes.is_empty() {
            return Err(DomainError::EmptyOutcomes);
        }

        if payout <= Decimal::ZERO {
            return Err(DomainError::NonPositivePayout { payout });
        }

        Ok(Self {
            market_id,
            question: question.into(),
            outcomes,
            payout,
        })
    }

    /// Returns the market ID.
    #[must_use]
    pub const fn market_id(&self) -> &MarketId {
        &self.market_id
    }

    /// Returns the market question text.
    #[must_use]
    pub fn question(&self) -> &str {
        &self.question
    }

    /// Returns the payout amount per share.
    #[must_use]
    pub const fn payout(&self) -> Decimal {
        self.payout
    }

    /// Returns all outcomes for this market.
    #[must_use]
    pub fn outcomes(&self) -> &[Outcome] {
        &self.outcomes
    }

    /// Returns true if this is a binary (two-outcome) market.
    #[must_use]
    pub fn is_binary(&self) -> bool {
        self.outcomes.len() == 2
    }

    /// Returns the number of outcomes in this market.
    #[must_use]
    pub fn outcome_count(&self) -> usize {
        self.outcomes.len()
    }

    /// Finds an outcome by name using case-insensitive matching.
    ///
    /// Returns `None` if no outcome matches the given name.
    #[must_use]
    pub fn outcome_by_name(&self, name: &str) -> Option<&Outcome> {
        let name_lower = name.to_lowercase();
        self.outcomes
            .iter()
            .find(|o| o.name.to_lowercase() == name_lower)
    }

    /// Returns all token IDs in outcome order.
    #[must_use]
    pub fn token_ids(&self) -> Vec<&TokenId> {
        self.outcomes.iter().map(|o| &o.token_id).collect()
    }
}

/// Index of markets by token ID and market ID for efficient lookups.
///
/// The registry maintains multiple indices to support fast lookups from
/// different starting points (token ID from order book events, market ID
/// from API responses, etc.).
///
/// # Examples
///
/// ```
/// use edgelord::domain::market::{Market, MarketRegistry, Outcome};
/// use edgelord::domain::id::{MarketId, TokenId};
/// use rust_decimal_macros::dec;
///
/// let mut registry = MarketRegistry::new();
///
/// let market = Market::new(
///     MarketId::new("market-1"),
///     "Test market?",
///     vec![
///         Outcome::new(TokenId::new("yes"), "Yes"),
///         Outcome::new(TokenId::new("no"), "No"),
///     ],
///     dec!(1.00),
/// );
///
/// registry.add(market);
///
/// // Look up by token ID (from order book event)
/// let found = registry.get_by_token(&TokenId::new("yes"));
/// assert!(found.is_some());
///
/// // Look up by market ID
/// let found = registry.get_by_market_id(&MarketId::new("market-1"));
/// assert!(found.is_some());
/// ```
#[derive(Debug, Default)]
pub struct MarketRegistry {
    /// Index from token ID to containing market.
    token_to_market: HashMap<TokenId, Market>,
    /// Index from market ID to market.
    market_id_to_market: HashMap<MarketId, Market>,
    /// All markets in registration order.
    markets: Vec<Market>,
}

impl MarketRegistry {
    /// Creates an empty market registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token_to_market: HashMap::new(),
            market_id_to_market: HashMap::new(),
            markets: Vec::new(),
        }
    }

    /// Adds a market to the registry, indexing all its token IDs.
    pub fn add(&mut self, market: Market) {
        self.market_id_to_market
            .insert(market.market_id().clone(), market.clone());
        for outcome in market.outcomes() {
            self.token_to_market
                .insert(outcome.token_id().clone(), market.clone());
        }
        self.markets.push(market);
    }

    /// Looks up a market by its market ID.
    ///
    /// Returns `None` if no market with the given ID is registered.
    #[must_use]
    pub fn get_by_market_id(&self, market_id: &MarketId) -> Option<&Market> {
        self.market_id_to_market.get(market_id)
    }

    /// Looks up a market by one of its token IDs.
    ///
    /// Returns `None` if no market contains the given token ID.
    #[must_use]
    pub fn get_by_token(&self, token_id: &TokenId) -> Option<&Market> {
        self.token_to_market.get(token_id)
    }

    /// All markets affected by a token, including configured event groups.
    pub fn get_all_by_token<'a>(
        &'a self,
        token_id: &'a TokenId,
    ) -> impl Iterator<Item = &'a Market> {
        self.markets
            .iter()
            .filter(move |market| market.outcomes().iter().any(|o| o.token_id() == token_id))
    }

    /// Returns all registered markets.
    #[must_use]
    pub fn markets(&self) -> &[Market] {
        &self.markets
    }

    /// Returns an iterator over binary (two-outcome) markets only.
    pub fn binary_markets(&self) -> impl Iterator<Item = &Market> {
        self.markets.iter().filter(|m| m.outcome_count() == 2)
    }

    /// Returns an iterator over multi-outcome (3+) markets only.
    pub fn multi_outcome_markets(&self) -> impl Iterator<Item = &Market> {
        self.markets.iter().filter(|m| m.outcome_count() >= 3)
    }

    /// Returns an iterator over all token IDs across all registered markets.
    pub fn all_token_ids(&self) -> impl Iterator<Item = &TokenId> {
        self.token_to_market.keys()
    }

    /// Returns the number of registered markets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.markets.len()
    }

    /// Returns true if no markets are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.markets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // --- Market tests ---

    fn create_binary_market() -> Market {
        let outcomes = vec![
            Outcome::new(TokenId::from("yes-token"), "Yes"),
            Outcome::new(TokenId::from("no-token"), "No"),
        ];
        Market::new(
            MarketId::from("market-1"),
            "Will it rain tomorrow?",
            outcomes,
            dec!(1.00),
        )
    }

    fn create_multi_outcome_market() -> Market {
        let outcomes = vec![
            Outcome::new(TokenId::from("red-token"), "Red"),
            Outcome::new(TokenId::from("blue-token"), "Blue"),
            Outcome::new(TokenId::from("green-token"), "Green"),
        ];
        Market::new(
            MarketId::from("market-2"),
            "What color will win?",
            outcomes,
            dec!(1.00),
        )
    }

    #[test]
    fn outcome_new_and_accessors() {
        let outcome = Outcome::new(TokenId::from("token-123"), "Yes");
        assert_eq!(outcome.token_id().as_str(), "token-123");
        assert_eq!(outcome.name(), "Yes");
    }

    #[test]
    fn market_new_and_accessors() {
        let market = create_binary_market();
        assert_eq!(market.market_id().as_str(), "market-1");
        assert_eq!(market.question(), "Will it rain tomorrow?");
        assert_eq!(market.payout(), dec!(1.00));
        assert_eq!(market.outcomes().len(), 2);
    }

    #[test]
    fn is_binary_returns_true_for_two_outcomes() {
        let market = create_binary_market();
        assert!(market.is_binary());
    }

    #[test]
    fn is_binary_returns_false_for_more_than_two_outcomes() {
        let market = create_multi_outcome_market();
        assert!(!market.is_binary());
    }

    #[test]
    fn outcome_count_returns_correct_count() {
        let binary = create_binary_market();
        assert_eq!(binary.outcome_count(), 2);

        let multi = create_multi_outcome_market();
        assert_eq!(multi.outcome_count(), 3);
    }

    #[test]
    fn outcome_by_name_finds_exact_match() {
        let market = create_binary_market();
        let outcome = market.outcome_by_name("Yes");
        assert!(outcome.is_some());
        assert_eq!(outcome.unwrap().token_id().as_str(), "yes-token");
    }

    #[test]
    fn outcome_by_name_is_case_insensitive() {
        let market = create_binary_market();

        // lowercase
        let outcome = market.outcome_by_name("yes");
        assert!(outcome.is_some());
        assert_eq!(outcome.unwrap().name(), "Yes");

        // uppercase
        let outcome = market.outcome_by_name("YES");
        assert!(outcome.is_some());
        assert_eq!(outcome.unwrap().name(), "Yes");

        // mixed case
        let outcome = market.outcome_by_name("yEs");
        assert!(outcome.is_some());
        assert_eq!(outcome.unwrap().name(), "Yes");
    }

    #[test]
    fn outcome_by_name_returns_none_for_nonexistent() {
        let market = create_binary_market();
        let outcome = market.outcome_by_name("Maybe");
        assert!(outcome.is_none());
    }

    #[test]
    fn token_ids_returns_all_token_ids() {
        let market = create_binary_market();
        let ids = market.token_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().any(|id| id.as_str() == "yes-token"));
        assert!(ids.iter().any(|id| id.as_str() == "no-token"));
    }

    #[test]
    fn token_ids_order_matches_outcomes() {
        let market = create_multi_outcome_market();
        let ids = market.token_ids();
        assert_eq!(ids[0].as_str(), "red-token");
        assert_eq!(ids[1].as_str(), "blue-token");
        assert_eq!(ids[2].as_str(), "green-token");
    }

    #[test]
    fn market_try_new_accepts_valid_inputs() {
        let outcomes = vec![
            Outcome::new(TokenId::from("yes-token"), "Yes"),
            Outcome::new(TokenId::from("no-token"), "No"),
        ];

        let result = Market::try_new(
            MarketId::from("market-1"),
            "Test question",
            outcomes,
            dec!(1.00),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn market_rejects_empty_outcomes() {
        // Empty outcomes should fail
        let result = Market::try_new(
            MarketId::from("market-1"),
            "Test question",
            vec![],
            dec!(1.00),
        );
        assert!(result.is_err());
    }

    #[test]
    fn market_rejects_non_positive_payout() {
        let outcomes = vec![
            Outcome::new(TokenId::from("yes-token"), "Yes"),
            Outcome::new(TokenId::from("no-token"), "No"),
        ];

        // Zero payout should fail
        let result = Market::try_new(
            MarketId::from("market-1"),
            "Test question",
            outcomes.clone(),
            dec!(0),
        );
        assert!(result.is_err());

        // Negative payout should fail
        let result = Market::try_new(
            MarketId::from("market-1"),
            "Test question",
            outcomes,
            dec!(-1),
        );
        assert!(result.is_err());
    }

    // --- MarketRegistry tests ---

    fn create_binary_market_with_id(id: &str, yes_token: &str, no_token: &str) -> Market {
        let outcomes = vec![
            Outcome::new(TokenId::from(yes_token), "Yes"),
            Outcome::new(TokenId::from(no_token), "No"),
        ];
        Market::new(
            MarketId::from(id),
            format!("Market {id}?"),
            outcomes,
            dec!(1.00),
        )
    }

    #[test]
    fn registry_new_creates_empty() {
        let registry = MarketRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn registry_add_indexes_all_tokens() {
        let mut registry = MarketRegistry::new();
        registry.add(create_binary_market_with_id("m1", "yes-1", "no-1"));

        assert_eq!(registry.len(), 1);
        assert!(registry.get_by_token(&TokenId::from("yes-1")).is_some());
        assert!(registry.get_by_token(&TokenId::from("no-1")).is_some());
    }

    #[test]
    fn registry_get_by_market_id() {
        let mut registry = MarketRegistry::new();
        registry.add(create_binary_market_with_id("m1", "yes-1", "no-1"));

        let market = registry.get_by_market_id(&MarketId::from("m1"));
        assert!(market.is_some());
        assert_eq!(market.unwrap().market_id().as_str(), "m1");
    }

    #[test]
    fn registry_get_by_token_returns_correct_market() {
        let mut registry = MarketRegistry::new();
        registry.add(create_binary_market_with_id("m1", "yes-1", "no-1"));
        registry.add(create_binary_market_with_id("m2", "yes-2", "no-2"));

        let market = registry.get_by_token(&TokenId::from("yes-1")).unwrap();
        assert_eq!(market.market_id().as_str(), "m1");

        let market = registry.get_by_token(&TokenId::from("no-2")).unwrap();
        assert_eq!(market.market_id().as_str(), "m2");
    }

    #[test]
    fn registry_binary_markets_filters_correctly() {
        let mut registry = MarketRegistry::new();
        registry.add(create_binary_market_with_id("b1", "yes-1", "no-1"));
        registry.add(create_multi_outcome_market());

        let binary: Vec<_> = registry.binary_markets().collect();
        assert_eq!(binary.len(), 1);
        assert!(binary[0].is_binary());
    }
}
