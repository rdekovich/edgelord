//! Strategy registry factory.
//!
//! Provides factory functions for constructing the strategy registry
//! with configured detection strategies.

use std::sync::Arc;

use tracing::warn;

use crate::application::cache::cluster::ClusterCache;
use crate::application::strategy::registry::StrategyRegistry;
use crate::infrastructure::config::settings::Config;

use super::solver::build_projection_solver;

/// Build the strategy registry from configuration.
///
/// Creates a registry containing all enabled detection strategies as
/// specified in the configuration. Unknown strategy names are logged
/// and skipped.
pub fn build_strategy_registry(
    config: &Config,
    _cluster_cache: Arc<ClusterCache>,
) -> StrategyRegistry {
    let cluster_cache = Arc::new(ClusterCache::new(chrono::Duration::days(365)));
    let relations = config
        .strategies
        .verified_relations
        .iter()
        .cloned()
        .map(|kind| {
            crate::domain::relation::Relation::new(kind, 1.0, "Operator-verified configuration")
                .with_ttl(chrono::Duration::days(365))
        })
        .collect();
    cluster_cache.put_relations(relations);
    let mut builder = StrategyRegistry::builder()
        .cluster_cache(cluster_cache)
        .projection_solver(build_projection_solver());

    for name in &config.strategies.enabled {
        let normalized = normalize_strategy_name(name);
        match normalized.as_str() {
            "single_condition" => {
                builder = builder.single_condition(config.strategies.single_condition.clone());
            }
            "market_rebalancing" => {
                builder = builder.market_rebalancing(config.strategies.market_rebalancing.clone());
            }
            "combinatorial" => {
                let mut strategy_config = config.strategies.combinatorial.clone();
                strategy_config.enabled = true;
                builder = builder.combinatorial(strategy_config);
            }
            unknown => {
                warn!(
                    strategy = name,
                    normalized_strategy = unknown,
                    "Unknown strategy in config, skipping"
                );
            }
        }
    }

    builder.build()
}

fn normalize_strategy_name(raw: &str) -> String {
    raw.trim().to_lowercase().replace('-', "_")
}
