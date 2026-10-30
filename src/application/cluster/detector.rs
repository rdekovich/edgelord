//! Cluster detection using bounded enumeration of executable YES/NO baskets.
//!
//! Encapsulates the arbitrage detection algorithm for a single cluster,
//! separate from the service lifecycle management.

use std::sync::Arc;

use rust_decimal::Decimal;

use crate::domain::{
    book::Book, cluster::Cluster, constraint::ConstraintSense, id::TokenId, market::MarketRegistry,
    opportunity::Opportunity, opportunity::OpportunityLeg,
};
use crate::error::{Error, Result};
use crate::port::outbound::solver::ProjectionSolver;

use super::service::{ClusterDetectionConfig, ClusterOpportunity};

/// Errors specific to cluster detection operations.
#[derive(Debug, Clone)]
pub enum DetectionError {
    /// The specified cluster was not found in the cache.
    ClusterNotFound(String),
    /// The cluster contains fewer than the minimum required markets.
    InsufficientMarkets {
        /// ID of the cluster with insufficient markets.
        cluster_id: String,
        /// Actual number of markets in the cluster.
        count: usize,
    },
    /// Price data is not available for a market in the cluster.
    MissingPriceData {
        /// ID of the market missing price data.
        market_id: String,
    },
    /// The Frank-Wolfe solver encountered an error.
    SolverFailed(String),
    /// The arbitrage gap is below the configured threshold.
    GapBelowThreshold {
        /// Detected gap value.
        gap: Decimal,
        /// Required threshold.
        threshold: Decimal,
    },
}

impl std::fmt::Display for DetectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClusterNotFound(id) => write!(f, "Cluster not found: {id}"),
            Self::InsufficientMarkets { cluster_id, count } => {
                write!(f, "Cluster {cluster_id} has only {count} markets (need 2+)")
            }
            Self::MissingPriceData { market_id } => {
                write!(f, "Missing price data for market: {market_id}")
            }
            Self::SolverFailed(msg) => write!(f, "Solver failed: {msg}"),
            Self::GapBelowThreshold { gap, threshold } => {
                write!(f, "Gap {gap} below threshold {threshold}")
            }
        }
    }
}

impl std::error::Error for DetectionError {}

/// Function type for looking up order books by token ID.
pub type BookLookup<'a> = &'a dyn Fn(&TokenId) -> Option<Book>;

/// Cluster detector using Frank-Wolfe projection.
///
/// Encapsulates the detection algorithm for finding arbitrage within a
/// cluster of related markets. Separate from the service lifecycle to
/// enable both synchronous (strategy) and asynchronous (service) usage.
pub struct ClusterDetector {
    /// Detection configuration.
    config: ClusterDetectionConfig,
    /// Projection solver for running Frank-Wolfe.
    _projection_solver: Arc<dyn ProjectionSolver>,
}

impl ClusterDetector {
    /// Create a new detector with the given configuration and solver.
    pub fn new(
        config: ClusterDetectionConfig,
        projection_solver: Arc<dyn ProjectionSolver>,
    ) -> Self {
        Self {
            config,
            _projection_solver: projection_solver,
        }
    }

    /// Detect arbitrage opportunities in a cluster.
    ///
    /// Runs Frank-Wolfe projection to find the distance between current
    /// market prices and the arbitrage-free manifold.
    ///
    /// # Arguments
    ///
    /// * `cluster` - The cluster of related markets to analyze.
    /// * `book_lookup` - Function to look up order books by token ID.
    /// * `registry` - Market registry for resolving market metadata.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(opportunity))` if arbitrage is found above threshold
    /// - `Ok(None)` if gap is below threshold (no actionable opportunity)
    /// - `Err` if detection failed (missing data, solver error, etc.)
    ///
    /// # Errors
    ///
    /// Returns an error if price data is missing or the solver fails.
    pub fn detect(
        &self,
        cluster: &Cluster,
        book_lookup: BookLookup<'_>,
        registry: &MarketRegistry,
    ) -> Result<Option<ClusterOpportunity>> {
        // Enumerate bounded portfolios and prove their minimum payout against every
        // allowed resolution. Projection distance is not an executable portfolio.
        let n = cluster.markets.len();
        if !(2..=8).contains(&n) {
            return Err(Error::Parse(
                "Executable clusters require 2..=8 markets".into(),
            ));
        }
        if cluster.constraints.is_empty()
            || cluster
                .constraints
                .iter()
                .any(|c| c.coefficients.len() != n)
        {
            return Err(Error::Parse(
                "Missing or malformed cluster constraints".into(),
            ));
        }
        let scenarios: Vec<usize> = (0..(1usize << n))
            .filter(|mask| {
                cluster.constraints.iter().all(|c| {
                    let lhs: Decimal = c
                        .coefficients
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| mask & (1 << i) != 0)
                        .map(|(_, coefficient)| *coefficient)
                        .sum();
                    match c.sense {
                        ConstraintSense::LessEqual => lhs <= c.rhs,
                        ConstraintSense::GreaterEqual => lhs >= c.rhs,
                        ConstraintSense::Equal => lhs == c.rhs,
                    }
                })
            })
            .collect();
        if scenarios.is_empty() {
            return Err(Error::Parse("Contradictory market constraints".into()));
        }
        let mut quotes = Vec::new();
        for id in &cluster.markets {
            let market = registry
                .get_by_market_id(id)
                .ok_or_else(|| Error::Parse(format!("Missing market {id}")))?;
            if !market.is_binary() || market.payout() != Decimal::ONE {
                return Err(Error::Parse(
                    "Combinatorial execution requires binary $1 markets".into(),
                ));
            }
            let mut sides = Vec::new();
            for outcome in market.outcomes() {
                sides.push(book_lookup(outcome.token_id()).and_then(|book| {
                    book.best_ask()
                        .filter(|l| {
                            l.price() > Decimal::ZERO
                                && l.price() < Decimal::ONE
                                && l.size() > Decimal::ZERO
                        })
                        .map(|l| (outcome.token_id().clone(), l.price(), l.size()))
                }));
            }
            if sides.iter().all(Option::is_none) {
                return Err(Error::Parse(format!("Missing quotes for {id}")));
            }
            quotes.push(sides);
        }
        let mut best: Option<Opportunity> = None;
        // Each market contributes no token, YES, or NO. Uniform quantities are
        // intentional; unsupported weighted portfolios are never approximated.
        for mut code in 1..3usize.pow(n as u32) {
            let (mut yes, mut no) = (0usize, 0usize);
            let mut legs = Vec::new();
            let mut volume = Decimal::MAX;
            let mut available = true;
            for (i, sides) in quotes.iter().enumerate() {
                let choice = code % 3;
                code /= 3;
                if choice == 0 {
                    continue;
                }
                let Some((token, price, size)) = &sides[choice - 1] else {
                    available = false;
                    break;
                };
                if choice == 1 {
                    yes |= 1 << i;
                } else {
                    no |= 1 << i;
                }
                legs.push(OpportunityLeg::new(token.clone(), *price));
                volume = volume.min(*size);
            }
            if !available || legs.len() < 2 {
                continue;
            }
            let payout = scenarios
                .iter()
                .map(|mask| (mask & yes).count_ones() + ((!mask & no).count_ones()))
                .min()
                .unwrap_or(0);
            if payout == 0 {
                continue;
            }
            let opportunity = Opportunity::with_strategy(
                cluster.markets[0].clone(),
                format!("Conditional portfolio for cluster {}", cluster.id),
                legs,
                volume,
                Decimal::from(payout),
                "combinatorial",
            );
            if opportunity.edge() < self.config.min_gap || opportunity.edge() <= Decimal::ZERO {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|old| opportunity.expected_profit() > old.expected_profit())
            {
                best = Some(opportunity);
            }
        }
        Ok(best.map(|opportunity| ClusterOpportunity {
            cluster_id: cluster.id.to_string(),
            markets: cluster.markets.clone(),
            gap: opportunity.edge(),
            opportunity,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::outbound::solver::IlpProblem;
    use crate::port::{outbound::solver::ProjectionResult, outbound::solver::ProjectionSolver};

    struct MockProjectionSolver;

    impl ProjectionSolver for MockProjectionSolver {
        fn name(&self) -> &'static str {
            "mock_projection"
        }

        fn project(&self, theta: &[Decimal], _problem: &IlpProblem) -> Result<ProjectionResult> {
            Ok(ProjectionResult {
                values: theta.to_vec(),
                gap: Decimal::ZERO,
                iterations: 1,
                converged: true,
            })
        }
    }

    #[test]
    fn test_detector_creation() {
        let config = ClusterDetectionConfig::default();
        let detector = ClusterDetector::new(config, Arc::new(MockProjectionSolver));
        assert_eq!(detector.config.debounce_ms, 100);
    }

    #[test]
    fn test_detection_error_display() {
        let err = DetectionError::ClusterNotFound("test-id".to_string());
        assert!(err.to_string().contains("test-id"));

        let err = DetectionError::InsufficientMarkets {
            cluster_id: "c1".to_string(),
            count: 1,
        };
        assert!(err.to_string().contains("1 markets"));

        let err = DetectionError::GapBelowThreshold {
            gap: Decimal::new(1, 2),
            threshold: Decimal::new(2, 2),
        };
        assert!(err.to_string().contains("below threshold"));
    }
}
