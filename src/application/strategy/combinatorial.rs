//! Bounded combinatorial arbitrage using executable YES/NO baskets.
//!
//! Enumerates resolution scenarios for operator-verified clusters of 2–8
//! binary markets. Minimum payout is compared with real ask cost and liquidity.
//! Retained projection APIs are research infrastructure, not trade sizing.

use std::sync::Arc;

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::application::cache::cluster::ClusterCache;
use crate::application::cluster::detector::ClusterDetector;
use crate::application::cluster::service::ClusterDetectionConfig;
use crate::application::solver::frank_wolfe::{FrankWolfe, FrankWolfeConfig};
use crate::domain::{market::MarketRegistry, opportunity::Opportunity};
use crate::port::{
    inbound::strategy::DetectionContext, inbound::strategy::MarketContext,
    inbound::strategy::Strategy, outbound::solver::ProjectionSolver,
};

/// Configuration for the combinatorial arbitrage strategy.
#[derive(Debug, Clone, Deserialize)]
pub struct CombinatorialConfig {
    /// Legacy research solver iteration setting (not used for execution).
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,

    /// Legacy research solver tolerance (not used for execution).
    #[serde(default = "default_tolerance")]
    pub tolerance: Decimal,

    /// Minimum arbitrage gap required to generate an opportunity.
    /// Filters out small gaps that may not be profitable after fees.
    #[serde(default = "default_gap_threshold")]
    pub gap_threshold: Decimal,

    /// Whether this strategy is enabled.
    /// Disabled by default as it requires dependency configuration.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_max_iterations() -> usize {
    20
}

fn default_tolerance() -> Decimal {
    Decimal::new(1, 4) // 0.0001
}

fn default_gap_threshold() -> Decimal {
    Decimal::new(2, 2) // 0.02
}

const fn default_enabled() -> bool {
    false // Disabled by default - requires dependency configuration
}

impl Default for CombinatorialConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            tolerance: default_tolerance(),
            gap_threshold: default_gap_threshold(),
            enabled: default_enabled(),
        }
    }
}

/// Combinatorial arbitrage strategy using bounded basket enumeration.
///
/// Checks basket payoffs against all resolutions allowed by verified constraints.
///
/// # Requirements
///
/// This strategy requires several components to function:
/// 1. Cluster cache with operator-verified market relations
/// 2. Market registry for resolving market metadata
/// 3. Projection solver retained by the research API (unused by basket detection)
///
/// Without these dependencies configured, the strategy returns no opportunities.
pub struct CombinatorialStrategy {
    /// Strategy configuration.
    config: CombinatorialConfig,
    /// Frank-Wolfe algorithm instance.
    fw: FrankWolfe,
    /// Cluster cache for relation lookups (injected).
    cluster_cache: Option<Arc<ClusterCache>>,
    /// Market registry for resolving market IDs (injected).
    registry: Option<Arc<MarketRegistry>>,
    /// Cluster detector for enumerating executable baskets.
    detector: Option<ClusterDetector>,
    /// Projection solver implementation (injected from infrastructure).
    projection_solver: Option<Arc<dyn ProjectionSolver>>,
}

impl CombinatorialStrategy {
    /// Create a new strategy with the given configuration.
    ///
    /// The strategy will not detect opportunities until dependencies
    /// (cluster cache, registry, projection solver) are injected.
    #[must_use]
    pub fn new(config: CombinatorialConfig) -> Self {
        let fw_config = FrankWolfeConfig {
            max_iterations: config.max_iterations,
            tolerance: config.tolerance,
        };
        Self {
            config,
            fw: FrankWolfe::new(fw_config),
            cluster_cache: None,
            registry: None,
            detector: None,
            projection_solver: None,
        }
    }

    /// Inject the cluster cache for relation lookups.
    pub fn set_cache(&mut self, cache: Arc<ClusterCache>) {
        self.cluster_cache = Some(cache);
        self.update_detector();
    }

    /// Inject the market registry for resolving market metadata.
    pub fn set_registry(&mut self, registry: Arc<MarketRegistry>) {
        self.registry = Some(registry);
    }

    /// Inject the projection solver from infrastructure wiring.
    pub fn set_projection_solver(&mut self, projection_solver: Arc<dyn ProjectionSolver>) {
        self.projection_solver = Some(projection_solver);
        self.update_detector();
    }

    /// Create the strategy with the cluster cache already set.
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<ClusterCache>) -> Self {
        self.cluster_cache = Some(cache);
        self.update_detector();
        self
    }

    /// Create the strategy with the market registry already set.
    #[must_use]
    pub fn with_registry(mut self, registry: Arc<MarketRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Rebuild the detector instance based on current configuration.
    fn update_detector(&mut self) {
        let Some(projection_solver) = &self.projection_solver else {
            self.detector = None;
            return;
        };

        let detector_config = ClusterDetectionConfig {
            debounce_ms: 100, // Not used in synchronous detection
            min_gap: self.config.gap_threshold,
            max_clusters_per_cycle: 50, // Not relevant for single detection
        };
        self.detector = Some(ClusterDetector::new(
            detector_config,
            Arc::clone(projection_solver),
        ));
    }

    /// Return the current configuration.
    #[must_use]
    pub const fn config(&self) -> &CombinatorialConfig {
        &self.config
    }

    /// Return the Frank-Wolfe algorithm instance.
    #[must_use]
    pub const fn frank_wolfe(&self) -> &FrankWolfe {
        &self.fw
    }
}

impl Strategy for CombinatorialStrategy {
    fn name(&self) -> &'static str {
        "combinatorial"
    }

    fn applies_to(&self, ctx: &MarketContext) -> bool {
        self.config.enabled && (ctx.is_binary() || ctx.has_dependencies)
    }

    fn detect(&self, ctx: &dyn DetectionContext) -> Vec<Opportunity> {
        // Ensure we have all required components
        let cluster_cache = match &self.cluster_cache {
            Some(c) => c,
            None => return vec![],
        };

        let registry = match &self.registry {
            Some(r) => r,
            None => {
                tracing::warn!("Combinatorial strategy missing market registry");
                return vec![];
            }
        };

        let detector = match &self.detector {
            Some(d) => d,
            None => {
                tracing::warn!("Combinatorial strategy missing detector");
                return vec![];
            }
        };

        // Get cluster for this market
        let cluster = match cluster_cache.get_for_market(ctx.market_id()) {
            Some(c) => c,
            None => return vec![], // No known relations
        };

        tracing::debug!(
            market_id = %ctx.market_id(),
            cluster_size = cluster.markets.len(),
            constraint_count = cluster.constraints.len(),
            "Running combinatorial detection on cluster"
        );

        // Create book lookup closure that uses the context
        let book_lookup = |token_id: &crate::domain::id::TokenId| ctx.order_book(token_id);

        // Run cluster detection
        match detector.detect(&cluster, &book_lookup, registry) {
            Ok(Some(cluster_opp)) => {
                tracing::info!(
                    market_id = %ctx.market_id(),
                    gap = %cluster_opp.gap,
                    "Found combinatorial opportunity"
                );
                vec![cluster_opp.opportunity]
            }
            Ok(None) => {
                tracing::trace!(
                    market_id = %ctx.market_id(),
                    "No combinatorial opportunity (gap below threshold)"
                );
                vec![]
            }
            Err(e) => {
                tracing::debug!(
                    market_id = %ctx.market_id(),
                    error = %e,
                    "Combinatorial detection failed"
                );
                vec![]
            }
        }
    }

    fn set_market_registry(&mut self, registry: Arc<MarketRegistry>) {
        self.set_registry(registry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::cache::book::BookCache;
    use crate::application::strategy::context::ConcreteDetectionContext;
    use crate::domain::constraint::Constraint;
    use crate::domain::{
        book::PriceLevel, cluster::Cluster, id::ClusterId, id::MarketId, id::TokenId,
        market::Market, market::Outcome,
    };
    use chrono::{Duration, Utc};
    use rust_decimal_macros::dec;

    fn make_test_config() -> CombinatorialConfig {
        CombinatorialConfig {
            enabled: true,
            max_iterations: 20,
            tolerance: dec!(0.0001),
            gap_threshold: dec!(0.02),
        }
    }

    fn make_binary_market(id: &str, yes_token: &str, no_token: &str) -> Market {
        let outcomes = vec![
            Outcome::new(TokenId::from(yes_token), "Yes"),
            Outcome::new(TokenId::from(no_token), "No"),
        ];
        Market::new(
            MarketId::from(id),
            format!("Market {id}?"),
            outcomes,
            dec!(1),
        )
    }

    #[test]
    fn test_strategy_name() {
        let strategy = CombinatorialStrategy::new(CombinatorialConfig::default());
        assert_eq!(strategy.name(), "combinatorial");
    }

    #[test]
    fn test_disabled_by_default() {
        let config = CombinatorialConfig::default();
        assert!(!config.enabled);
    }

    #[test]
    fn test_applies_only_with_dependencies() {
        let config = CombinatorialConfig {
            enabled: true,
            ..Default::default()
        };

        let strategy = CombinatorialStrategy::new(config);

        // Should not apply to markets without dependencies
        assert!(strategy.applies_to(&MarketContext::binary()));
        assert!(!strategy.applies_to(&MarketContext::multi_outcome(3)));

        // Should apply to markets with dependencies
        let ctx_with_deps = MarketContext::binary()
            .with_dependencies(vec![crate::domain::id::MarketId::from("other")]);
        assert!(strategy.applies_to(&ctx_with_deps));
    }

    #[test]
    fn test_config_defaults() {
        let config = CombinatorialConfig::default();

        assert_eq!(config.max_iterations, 20);
        assert_eq!(config.tolerance, Decimal::new(1, 4));
        assert_eq!(config.gap_threshold, Decimal::new(2, 2));
        assert!(!config.enabled);
    }

    #[test]
    fn test_strategy_requires_cluster_cache() {
        let config = make_test_config();
        let strategy = CombinatorialStrategy::new(config);

        let market = make_binary_market("m1", "yes", "no");
        let cache = BookCache::new();
        let ctx = ConcreteDetectionContext::new(&market, &cache);

        // Without cluster cache, should return empty
        let opps = strategy.detect(&ctx);
        assert!(opps.is_empty());
    }

    fn make_cluster(market_ids: Vec<MarketId>) -> Cluster {
        Cluster {
            id: ClusterId::new(),
            markets: market_ids,
            relations: vec![],
            constraints: vec![Constraint::geq(vec![dec!(1), dec!(1)], dec!(1))],
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_strategy_requires_registry() {
        let config = make_test_config();
        let mut strategy = CombinatorialStrategy::new(config);

        // Set cluster cache but not registry
        let cluster_cache = Arc::new(ClusterCache::new(Duration::hours(1)));
        strategy.set_cache(cluster_cache);

        let market = make_binary_market("m1", "yes", "no");
        let cache = BookCache::new();
        let ctx = ConcreteDetectionContext::new(&market, &cache);

        // Without registry, should return empty
        let opps = strategy.detect(&ctx);
        assert!(opps.is_empty());
    }

    #[test]
    fn test_detect_returns_empty_when_no_cluster() {
        let config = make_test_config();
        let mut strategy = CombinatorialStrategy::new(config);

        // Set both cache and registry
        let cluster_cache = Arc::new(ClusterCache::new(Duration::hours(1)));
        let registry = Arc::new(MarketRegistry::new());
        strategy.set_cache(cluster_cache);
        strategy.set_registry(registry);

        let market = make_binary_market("m1", "yes", "no");
        let cache = BookCache::new();
        let ctx = ConcreteDetectionContext::new(&market, &cache);

        // No cluster exists for this market
        let opps = strategy.detect(&ctx);
        assert!(opps.is_empty());
    }

    #[test]
    fn test_detect_returns_empty_when_gap_below_threshold() {
        let config = CombinatorialConfig {
            gap_threshold: dec!(0.10), // High threshold
            ..make_test_config()
        };
        let mut strategy = CombinatorialStrategy::new(config);

        // Create markets
        let m1 = make_binary_market("m1", "yes1", "no1");
        let m2 = make_binary_market("m2", "yes2", "no2");

        // Build registry
        let mut registry = MarketRegistry::new();
        registry.add(m1.clone());
        registry.add(m2.clone());
        let registry = Arc::new(registry);

        // Create cluster
        let cluster = make_cluster(vec![m1.market_id().clone(), m2.market_id().clone()]);

        let cluster_cache = Arc::new(ClusterCache::new(Duration::hours(1)));
        cluster_cache.put(cluster);

        strategy.set_cache(cluster_cache);
        strategy.set_registry(registry);

        // Set prices that are fair (no arbitrage)
        let cache = BookCache::new();
        cache.update(crate::domain::book::Book::with_levels(
            TokenId::from("yes1"),
            vec![],
            vec![PriceLevel::new(dec!(0.50), dec!(100))],
        ));
        cache.update(crate::domain::book::Book::with_levels(
            TokenId::from("yes2"),
            vec![],
            vec![PriceLevel::new(dec!(0.50), dec!(100))],
        ));

        let ctx = ConcreteDetectionContext::new(&m1, &cache);

        // Should return empty due to low/no arbitrage gap
        let opps = strategy.detect(&ctx);
        assert!(opps.is_empty());
    }

    #[test]
    fn test_detect_handles_missing_price_data() {
        let config = make_test_config();
        let mut strategy = CombinatorialStrategy::new(config);

        // Create markets
        let m1 = make_binary_market("m1", "yes1", "no1");
        let m2 = make_binary_market("m2", "yes2", "no2");

        // Build registry
        let mut registry = MarketRegistry::new();
        registry.add(m1.clone());
        registry.add(m2.clone());
        let registry = Arc::new(registry);

        // Create cluster
        let cluster = make_cluster(vec![m1.market_id().clone(), m2.market_id().clone()]);

        let cluster_cache = Arc::new(ClusterCache::new(Duration::hours(1)));
        cluster_cache.put(cluster);

        strategy.set_cache(cluster_cache);
        strategy.set_registry(registry);

        // Empty cache (no price data)
        let cache = BookCache::new();
        let ctx = ConcreteDetectionContext::new(&m1, &cache);

        // Should fail closed and return empty
        let opps = strategy.detect(&ctx);
        assert!(opps.is_empty());
    }

    #[test]
    fn test_strategy_with_cache_builder() {
        let config = make_test_config();
        let cluster_cache = Arc::new(ClusterCache::new(Duration::hours(1)));
        let strategy = CombinatorialStrategy::new(config).with_cache(cluster_cache);

        assert!(strategy.cluster_cache.is_some());
    }

    #[test]
    fn test_strategy_with_registry_builder() {
        let config = make_test_config();
        let registry = Arc::new(MarketRegistry::new());
        let strategy = CombinatorialStrategy::new(config).with_registry(registry);

        assert!(strategy.registry.is_some());
    }

    #[test]
    fn test_applies_to_checks_enabled_flag() {
        let config = CombinatorialConfig {
            enabled: false,
            ..Default::default()
        };
        let strategy = CombinatorialStrategy::new(config);

        let ctx_with_deps =
            MarketContext::binary().with_dependencies(vec![MarketId::from("other")]);

        // Even with dependencies, disabled strategy doesn't apply
        assert!(!strategy.applies_to(&ctx_with_deps));
    }

    #[test]
    fn test_applies_to_checks_cache_for_relations() {
        let config = make_test_config();
        let mut strategy = CombinatorialStrategy::new(config);

        let m1 = make_binary_market("m1", "yes1", "no1");
        let m2 = make_binary_market("m2", "yes2", "no2");

        // Create cluster
        let cluster = make_cluster(vec![m1.market_id().clone(), m2.market_id().clone()]);

        let cluster_cache = Arc::new(ClusterCache::new(Duration::hours(1)));
        cluster_cache.put(cluster);
        strategy.set_cache(cluster_cache);

        // Context without dependencies but market is in cache
        let ctx = MarketContext {
            outcome_count: 2,
            has_dependencies: false,
            correlated_markets: vec![m1.market_id().clone()],
        };

        // Should apply because cache has relations
        assert!(strategy.applies_to(&ctx));
    }
}
