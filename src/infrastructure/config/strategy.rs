//! Strategy configuration for detection strategies.
//!
//! Provides configuration for enabling and tuning the various arbitrage
//! detection strategies.

use serde::Deserialize;

use crate::application::strategy::combinatorial::CombinatorialConfig;
use crate::application::strategy::market_rebalancing::MarketRebalancingConfig;
use crate::application::strategy::single_condition::SingleConditionConfig;

/// Configuration for all detection strategies.
///
/// Controls which strategies are active and their individual parameters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StrategiesConfig {
    /// List of enabled strategy names.
    ///
    /// Valid names: "single_condition", "market_rebalancing", "combinatorial".
    /// Defaults to ["single_condition"].
    #[serde(default = "default_enabled_strategies")]
    pub enabled: Vec<String>,

    /// Operator-verified resolution relationships. LLM guesses are not execution authority.
    #[serde(default)]
    pub verified_relations: Vec<crate::domain::relation::RelationKind>,

    /// Complete, mutually exclusive and exhaustive event groups (condition IDs).
    /// All members must be discovered; partial groups fail startup.
    #[serde(default)]
    pub exhaustive_groups: Vec<Vec<crate::domain::id::MarketId>>,

    /// Single-condition strategy configuration.
    ///
    /// Detects simple YES+NO < 1 arbitrage opportunities.
    #[serde(default)]
    pub single_condition: SingleConditionConfig,

    /// Market rebalancing strategy configuration.
    ///
    /// Detects opportunities from multi-outcome market mispricing.
    #[serde(default)]
    pub market_rebalancing: MarketRebalancingConfig,

    /// Combinatorial strategy configuration.
    ///
    /// Enumerates baskets against operator-verified resolution constraints.
    #[serde(default)]
    pub combinatorial: CombinatorialConfig,
}

fn default_enabled_strategies() -> Vec<String> {
    vec!["single_condition".to_string()]
}
