//! Risk management service.
//!
//! Provides pre-execution validation for opportunities, ensuring trades
//! comply with configured risk limits before execution.

use std::sync::Arc;

use rust_decimal::Decimal;
use tracing::{info, warn};

use crate::application::state::AppState;
use crate::domain::{opportunity::Opportunity, position::Position};
use crate::error::RiskError;
use crate::port::inbound::risk::RiskCheckResult;

/// Risk manager that validates trades before execution.
///
/// Performs comprehensive pre-trade checks including:
/// - Circuit breaker status (halts all trading when active)
/// - Profit threshold validation (filters unprofitable opportunities)
/// - Position limits per market (prevents concentration risk)
/// - Total exposure limits (caps overall risk exposure)
///
/// On approval, atomically reserves exposure to prevent concurrent
/// opportunities from exceeding configured limits.
pub struct RiskManager {
    /// Shared application state containing risk limits and positions.
    state: Arc<AppState>,
}

impl RiskManager {
    /// Create a new risk manager with the given shared state.
    pub const fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Validate an opportunity against all risk checks.
    ///
    /// Checks are performed in order: circuit breaker, profit threshold,
    /// position limit, and exposure limit. On approval, atomically reserves
    /// the required exposure to prevent concurrent opportunities from
    /// exceeding limits.
    ///
    /// Returns [`RiskCheckResult::Approved`] if all checks pass, or
    /// [`RiskCheckResult::Rejected`] with the specific error if any check fails.
    #[must_use]
    pub fn check(&self, opportunity: &Opportunity) -> RiskCheckResult {
        // Check circuit breaker first
        if let Err(e) = self.check_circuit_breaker() {
            return RiskCheckResult::Rejected(e);
        }

        // Check profit threshold
        if let Err(e) = self.check_profit_threshold(opportunity) {
            return RiskCheckResult::Rejected(e);
        }

        // Check position limit for this market (before reserving exposure)
        if let Err(e) = self.check_position_limit(opportunity) {
            return RiskCheckResult::Rejected(e);
        }

        // Atomically check and reserve exposure
        let additional_exposure = opportunity.total_cost() * opportunity.volume();
        if !self.state.try_reserve_exposure(additional_exposure) {
            let current = self.state.total_exposure();
            let pending = self.state.pending_exposure();
            let limit = self.state.risk_limits().max_total_exposure;
            warn!(
                current = %current,
                pending = %pending,
                additional = %additional_exposure,
                limit = %limit,
                "Exposure limit would be exceeded"
            );
            return RiskCheckResult::Rejected(RiskError::ExposureLimitExceeded {
                current,
                additional: additional_exposure,
                limit,
            });
        }

        RiskCheckResult::Approved
    }

    /// Release previously reserved exposure.
    ///
    /// Call after execution completes (success or failure) to free the
    /// reserved exposure for future opportunities.
    pub fn release_exposure(&self, opportunity: &Opportunity) {
        let amount = opportunity.total_cost() * opportunity.volume();
        self.state.release_exposure(amount);
    }

    /// Activate the circuit breaker, halting all new trades.
    pub fn trigger_circuit_breaker(&self, reason: impl Into<String>) {
        let reason = reason.into();
        warn!(reason = %reason, "Triggering circuit breaker");
        self.state.activate_circuit_breaker(reason);
    }

    /// Deactivate the circuit breaker, resuming normal trading.
    pub fn reset_circuit_breaker(&self) {
        info!("Resetting circuit breaker");
        self.state.reset_circuit_breaker();
    }

    /// Return true if the circuit breaker is currently active.
    #[must_use]
    pub fn is_circuit_breaker_active(&self) -> bool {
        self.state.is_circuit_breaker_active()
    }

    /// Log a successful execution for monitoring.
    pub fn record_execution(&self, opportunity: &Opportunity) {
        info!(
            market_id = %opportunity.market_id(),
            volume = %opportunity.volume(),
            profit = %opportunity.expected_profit(),
            "Execution recorded"
        );
    }

    /// Verify the circuit breaker is not active.
    fn check_circuit_breaker(&self) -> Result<(), RiskError> {
        if self.state.is_circuit_breaker_active() {
            let reason = self
                .state
                .circuit_breaker_reason()
                .unwrap_or_else(|| "unknown".to_string());
            warn!(reason = %reason, "Circuit breaker active");
            return Err(RiskError::CircuitBreakerActive { reason });
        }
        Ok(())
    }

    /// Verify expected profit meets the configured minimum threshold.
    fn check_profit_threshold(&self, opportunity: &Opportunity) -> Result<(), RiskError> {
        let threshold = self.state.risk_limits().min_profit_threshold;
        let expected = opportunity.expected_profit();

        if expected < threshold {
            return Err(RiskError::ProfitBelowThreshold {
                expected,
                threshold,
            });
        }
        Ok(())
    }

    /// Verify the position in this market would not exceed the per-market limit.
    fn check_position_limit(&self, opportunity: &Opportunity) -> Result<(), RiskError> {
        let market_id = opportunity.market_id();
        let limit = self.state.risk_limits().max_position_per_market;

        // Calculate current position in this market
        let current = self
            .state
            .positions()
            .open_positions()
            .filter(|p| {
                p.market_id() == market_id
                    || p.legs().iter().any(|held| {
                        opportunity
                            .legs()
                            .iter()
                            .any(|leg| leg.token_id() == held.token_id())
                    })
            })
            .map(Position::entry_cost)
            .sum::<Decimal>();

        let additional = opportunity.total_cost() * opportunity.volume();

        if current + additional > limit {
            warn!(
                market_id = %market_id,
                current = %current,
                additional = %additional,
                limit = %limit,
                "Position limit would be exceeded"
            );
            return Err(RiskError::PositionLimitExceeded {
                market_id: market_id.to_string(),
                current,
                limit,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::state::RiskLimits;
    use crate::domain::{id::MarketId, id::TokenId, opportunity::OpportunityLeg};
    use rust_decimal_macros::dec;

    fn make_opportunity(volume: Decimal, yes_ask: Decimal, no_ask: Decimal) -> Opportunity {
        let legs = vec![
            OpportunityLeg::new(TokenId::from("yes"), yes_ask),
            OpportunityLeg::new(TokenId::from("no"), no_ask),
        ];
        Opportunity::new(
            MarketId::from("test-market"),
            "Test?",
            legs,
            volume,
            Decimal::ONE,
        )
    }

    #[test]
    fn test_check_approved() {
        let state = Arc::new(AppState::default());
        let risk = RiskManager::new(state);

        // Good opportunity: 10% edge, $10 volume = $1 profit
        let opp = make_opportunity(dec!(10), dec!(0.45), dec!(0.45));
        let result = risk.check(&opp);

        assert!(result.is_approved());
    }

    #[test]
    fn test_check_circuit_breaker() {
        let state = Arc::new(AppState::default());
        state.activate_circuit_breaker("test");
        let risk = RiskManager::new(state);

        let opp = make_opportunity(dec!(10), dec!(0.45), dec!(0.45));
        let result = risk.check(&opp);

        assert!(!result.is_approved());
        assert!(matches!(
            result.rejection_error(),
            Some(RiskError::CircuitBreakerActive { .. })
        ));
    }

    #[test]
    fn test_check_profit_below_threshold() {
        let limits = RiskLimits {
            min_profit_threshold: dec!(1.0), // Require $1 minimum
            ..Default::default()
        };
        let state = Arc::new(AppState::new(limits));
        let risk = RiskManager::new(state);

        // Only $0.10 edge * $1 volume = $0.10 profit (below $1 threshold)
        let opp = make_opportunity(dec!(1), dec!(0.45), dec!(0.45));
        let result = risk.check(&opp);

        assert!(!result.is_approved());
        assert!(matches!(
            result.rejection_error(),
            Some(RiskError::ProfitBelowThreshold { .. })
        ));
    }

    #[test]
    fn test_check_exposure_limit() {
        let limits = RiskLimits {
            max_total_exposure: dec!(100), // Only $100 max
            min_profit_threshold: dec!(0),
            ..Default::default()
        };
        let state = Arc::new(AppState::new(limits));
        let risk = RiskManager::new(state);

        // $0.90 cost * $200 volume = $180 exposure (over $100)
        let opp = make_opportunity(dec!(200), dec!(0.45), dec!(0.45));
        let result = risk.check(&opp);

        assert!(!result.is_approved());
        assert!(matches!(
            result.rejection_error(),
            Some(RiskError::ExposureLimitExceeded { .. })
        ));
    }

    #[test]
    fn test_trigger_and_reset_circuit_breaker() {
        let state = Arc::new(AppState::default());
        let risk = RiskManager::new(state);

        assert!(!risk.is_circuit_breaker_active());

        risk.trigger_circuit_breaker("test reason");
        assert!(risk.is_circuit_breaker_active());

        risk.reset_circuit_breaker();
        assert!(!risk.is_circuit_breaker_active());
    }

    #[test]
    fn test_release_exposure() {
        let limits = RiskLimits {
            max_total_exposure: dec!(100),
            min_profit_threshold: dec!(0),
            ..Default::default()
        };
        let state = Arc::new(AppState::new(limits));
        let risk = RiskManager::new(state.clone());

        // Reserve some exposure
        let opp = make_opportunity(dec!(50), dec!(0.45), dec!(0.45)); // $45 cost
        assert!(risk.check(&opp).is_approved());
        assert!(state.pending_exposure() > Decimal::ZERO);

        // Release it
        risk.release_exposure(&opp);
        assert_eq!(state.pending_exposure(), Decimal::ZERO);
    }

    #[test]
    fn test_check_position_limit() {
        use crate::domain::{position::Position, position::PositionLeg, position::PositionStatus};

        let limits = RiskLimits {
            max_position_per_market: dec!(100), // Only $100 per market
            min_profit_threshold: dec!(0),
            ..Default::default()
        };
        let state = Arc::new(AppState::new(limits));

        // Add existing position in this market
        {
            let mut positions = state.positions_mut();
            let position = Position::new(
                positions.next_id(),
                MarketId::from("test-market"),
                vec![
                    PositionLeg::new(TokenId::from("yes"), dec!(50), dec!(0.45)),
                    PositionLeg::new(TokenId::from("no"), dec!(50), dec!(0.45)),
                ],
                dec!(45), // $45 entry cost
                dec!(50),
                chrono::Utc::now(),
                PositionStatus::Open,
            );
            positions.add(position);
        }

        let risk = RiskManager::new(state);

        // Try to add $90 more (45 + 90 = 135 > 100 limit)
        let opp = make_opportunity(dec!(100), dec!(0.45), dec!(0.45)); // $90 cost
        let result = risk.check(&opp);

        assert!(!result.is_approved());
        assert!(matches!(
            result.rejection_error(),
            Some(RiskError::PositionLimitExceeded { .. })
        ));
    }
}
