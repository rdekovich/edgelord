//! Shared application state.
//!
//! Provides centralized, thread-safe state management for the application
//! including position tracking, risk limits, and circuit breaker status.

use crate::domain::{opportunity::Opportunity, trade::TradeResult};
use crate::port::outbound::journal::{ExecutionJournal, ExecutionRecord};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use rust_decimal::Decimal;

use crate::application::cache::position::PositionTracker;
use crate::domain::{money::Price, position::PositionStatus};
use crate::port::inbound::runtime::{
    RuntimePosition, RuntimePositionStatus, RuntimeRiskLimitKind, RuntimeRiskLimitUpdateError,
    RuntimeRiskLimits, RuntimeState,
};

/// Risk limits configuration.
///
/// Controls trading behavior by setting thresholds for position sizes,
/// exposure limits, and profit requirements.
#[derive(Debug, Clone)]
pub struct RiskLimits {
    /// Maximum position size per market in dollars.
    pub max_position_per_market: Decimal,
    /// Maximum total exposure across all open positions.
    pub max_total_exposure: Decimal,
    /// Minimum expected profit required to execute a trade.
    pub min_profit_threshold: Decimal,
    /// Maximum allowed slippage between detection and execution (e.g., 0.02 = 2%).
    pub max_slippage: Decimal,
    /// Timeout in seconds before cancelling an execution attempt.
    pub execution_timeout_secs: u64,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            max_position_per_market: Decimal::from(1000),
            max_total_exposure: Decimal::from(10000),
            min_profit_threshold: Decimal::new(5, 2), // $0.05
            max_slippage: Decimal::new(2, 2),         // 2%
            execution_timeout_secs: 30,
        }
    }
}

/// Risk limit fields that can be changed at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLimitKind {
    /// Maximum position size per market.
    MaxPositionPerMarket,
    /// Maximum total exposure across all positions.
    MaxTotalExposure,
    /// Minimum profit threshold for execution.
    MinProfitThreshold,
    /// Maximum slippage tolerance.
    MaxSlippage,
}

impl RiskLimitKind {
    /// Return a stable field name for logs and command output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxPositionPerMarket => "max_position",
            Self::MaxTotalExposure => "max_exposure",
            Self::MinProfitThreshold => "min_profit",
            Self::MaxSlippage => "max_slippage",
        }
    }
}

/// Error returned when a runtime risk limit update is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskLimitUpdateError {
    /// Description of why the update was rejected.
    reason: &'static str,
}

impl RiskLimitUpdateError {
    /// Create a new error with the given reason.
    #[must_use]
    pub const fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    /// Return the reason for the rejection.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl std::fmt::Display for RiskLimitUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for RiskLimitUpdateError {}

fn app_limits_to_runtime(limits: RiskLimits) -> RuntimeRiskLimits {
    RuntimeRiskLimits {
        max_position_per_market: limits.max_position_per_market,
        max_total_exposure: limits.max_total_exposure,
        min_profit_threshold: limits.min_profit_threshold,
        max_slippage: limits.max_slippage,
    }
}

fn runtime_kind_to_app(kind: RuntimeRiskLimitKind) -> RiskLimitKind {
    match kind {
        RuntimeRiskLimitKind::MaxPositionPerMarket => RiskLimitKind::MaxPositionPerMarket,
        RuntimeRiskLimitKind::MaxTotalExposure => RiskLimitKind::MaxTotalExposure,
        RuntimeRiskLimitKind::MinProfitThreshold => RiskLimitKind::MinProfitThreshold,
        RuntimeRiskLimitKind::MaxSlippage => RiskLimitKind::MaxSlippage,
    }
}

/// Shared application state accessible by all services.
///
/// Provides thread-safe access to positions, risk limits, circuit breaker
/// status, and exposure tracking. All operations are safe for concurrent
/// access from multiple tasks.
pub struct AppState {
    /// Position tracker for all open and closed positions.
    positions: RwLock<PositionTracker>,
    /// Current risk limits configuration.
    risk_limits: RwLock<RiskLimits>,
    /// Circuit breaker flag. When true, no new trades are allowed.
    circuit_breaker: AtomicBool,
    /// Reason for circuit breaker activation, if any.
    circuit_breaker_reason: RwLock<Option<String>>,
    /// Markets with in-flight executions (prevents duplicate trades).
    pending_executions: Mutex<HashSet<String>>,
    /// Reserved exposure from approved but not yet executed opportunities.
    pending_exposure: Mutex<Decimal>,
    journal: Option<Arc<dyn ExecutionJournal>>,
    reconciliation_required: AtomicBool,
    settled_tokens: Mutex<HashMap<crate::domain::id::TokenId, Decimal>>,
}

impl AppState {
    /// Create new application state with the given risk limits.
    #[must_use]
    pub fn new(risk_limits: RiskLimits) -> Self {
        Self {
            positions: RwLock::new(PositionTracker::new()),
            risk_limits: RwLock::new(risk_limits),
            circuit_breaker: AtomicBool::new(false),
            circuit_breaker_reason: RwLock::new(None),
            pending_executions: Mutex::new(HashSet::new()),
            pending_exposure: Mutex::new(Decimal::ZERO),
            journal: None,
            reconciliation_required: AtomicBool::new(false),
            settled_tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Restore confirmed inventory and retain every unresolved intent as exposure.
    pub fn with_journal(
        limits: RiskLimits,
        journal: Arc<dyn ExecutionJournal>,
    ) -> crate::error::Result<Self> {
        let mut state = Self::new(limits);
        let mut pending = HashMap::new();
        for record in journal.records()? {
            match record {
                ExecutionRecord::Account { .. } => {}
                ExecutionRecord::Settlement { payouts } => {
                    state.apply_settlement(&payouts)?;
                }
                ExecutionRecord::Intent { id, opportunity } => {
                    if pending.insert(id, opportunity).is_some() {
                        return Err(crate::error::Error::Parse(
                            "Duplicate execution intent".into(),
                        ));
                    }
                }
                ExecutionRecord::Outcome {
                    id,
                    result,
                    trade_id,
                } => {
                    let opportunity = pending.remove(&id).ok_or_else(|| {
                        crate::error::Error::Parse("Execution outcome without intent".into())
                    })?;
                    if !result.is_consistent_with(&opportunity) {
                        return Err(crate::error::Error::Parse(
                            "Invalid execution result in ledger".into(),
                        ));
                    }
                    match result {
                        TradeResult::Success { fills } => {
                            crate::application::orchestration::position::record_position(
                                &state,
                                &opportunity,
                                &fills,
                                trade_id,
                            )
                        }
                        TradeResult::Partial { fills, failures } => {
                            crate::application::orchestration::position::record_partial_position(
                                &state,
                                &opportunity,
                                &fills,
                                &failures,
                                trade_id,
                            );
                            state.require_reconciliation("Restored partial execution");
                        }
                        TradeResult::Failed { .. } => {}
                    }
                }
            }
        }
        for opportunity in pending.values() {
            *state.pending_exposure.lock() += opportunity.total_cost() * opportunity.volume();
            state.require_reconciliation("Unresolved durable execution intent");
        }
        state.apply_settlement(&[])?;
        state.journal = Some(journal);
        Ok(state)
    }

    /// Prevent using a ledger belonging to a different trading account.
    pub fn bind_account(&self, identity: &str) -> crate::error::Result<()> {
        let journal = self.journal.as_ref().ok_or_else(|| {
            crate::error::Error::Parse("Live execution requires a durable ledger".into())
        })?;
        let records = journal.records()?;
        if records.is_empty() {
            return journal.append(&ExecutionRecord::Account {
                identity: identity.into(),
            });
        }
        match records.first() {
            Some(ExecutionRecord::Account { identity: recorded }) if recorded == identity => Ok(()),
            _ => Err(crate::error::Error::Parse(
                "Ledger is unbound or belongs to another account; reconcile before trading".into(),
            )),
        }
    }

    pub(crate) fn record_intent(
        &self,
        id: &str,
        opportunity: &Opportunity,
    ) -> crate::error::Result<()> {
        if let Some(journal) = &self.journal {
            journal.append(&ExecutionRecord::Intent {
                id: id.into(),
                opportunity: opportunity.clone(),
            })?;
        }
        Ok(())
    }

    pub(crate) fn record_execution_result(
        &self,
        id: &str,
        result: &TradeResult,
        trade_id: Option<i32>,
    ) -> crate::error::Result<()> {
        if let Some(journal) = &self.journal {
            journal.append(&ExecutionRecord::Outcome {
                id: id.into(),
                result: result.clone(),
                trade_id,
            })?;
        }
        Ok(())
    }

    /// A normal pause/resume cannot clear uncertainty about funds.
    pub fn require_reconciliation(&self, reason: impl Into<String>) {
        self.reconciliation_required.store(true, Ordering::SeqCst);
        self.activate_circuit_breaker(reason);
    }

    /// Close only when every held token has an authoritative resolution payout.
    pub fn settle_tokens(
        &self,
        payouts: &[(crate::domain::id::TokenId, Decimal)],
    ) -> crate::error::Result<Vec<(Option<i32>, Decimal)>> {
        if payouts
            .iter()
            .any(|(_, payout)| *payout < Decimal::ZERO || *payout > Decimal::ONE)
        {
            return Err(crate::error::Error::Parse(
                "Invalid settlement payout".into(),
            ));
        }
        if let Some(journal) = &self.journal {
            journal.append(&ExecutionRecord::Settlement {
                payouts: payouts.to_vec(),
            })?;
        }
        self.apply_settlement(payouts)
    }

    fn apply_settlement(
        &self,
        payouts: &[(crate::domain::id::TokenId, Decimal)],
    ) -> crate::error::Result<Vec<(Option<i32>, Decimal)>> {
        let mut settled = self.settled_tokens.lock();
        for (token, payout) in payouts {
            if settled.get(token).is_some_and(|old| old != payout) {
                return Err(crate::error::Error::Parse(
                    "Conflicting settlement; reconcile inventory".into(),
                ));
            }
            settled.insert(token.clone(), *payout);
        }
        let mut positions = self.positions.write();
        let closes: Vec<_> = positions
            .all()
            .filter(|p| !p.status().is_closed())
            .filter_map(|position| {
                let payout: Option<Decimal> = position
                    .legs()
                    .iter()
                    .map(|leg| settled.get(leg.token_id()).map(|value| *value * leg.size()))
                    .sum();
                payout.map(|payout| {
                    (
                        position.id(),
                        position.trade_id(),
                        payout - position.entry_cost(),
                    )
                })
            })
            .collect();
        for (id, _, pnl) in &closes {
            positions.close(*id, *pnl);
        }
        Ok(closes
            .into_iter()
            .map(|(_, trade_id, pnl)| (trade_id, pnl))
            .collect())
    }

    /// Acquire read access to the position tracker.
    pub fn positions(&self) -> parking_lot::RwLockReadGuard<'_, PositionTracker> {
        self.positions.read()
    }

    /// Acquire write access to the position tracker.
    pub fn positions_mut(&self) -> parking_lot::RwLockWriteGuard<'_, PositionTracker> {
        self.positions.write()
    }

    /// Return a clone of the current risk limits.
    pub fn risk_limits(&self) -> RiskLimits {
        self.risk_limits.read().clone()
    }

    /// Update a single risk limit at runtime with validation.
    ///
    /// # Errors
    ///
    /// Returns an error if the value is invalid for the given limit kind:
    /// - `MaxPositionPerMarket` and `MaxTotalExposure` must be greater than 0
    /// - `MinProfitThreshold` must be 0 or greater
    /// - `MaxSlippage` must be between 0 and 1
    pub fn set_risk_limit(
        &self,
        kind: RiskLimitKind,
        value: Decimal,
    ) -> Result<RiskLimits, RiskLimitUpdateError> {
        match kind {
            RiskLimitKind::MaxPositionPerMarket | RiskLimitKind::MaxTotalExposure => {
                if value <= Decimal::ZERO {
                    return Err(RiskLimitUpdateError::new("value must be greater than 0"));
                }
            }
            RiskLimitKind::MinProfitThreshold => {
                if value < Decimal::ZERO {
                    return Err(RiskLimitUpdateError::new("value must be 0 or greater"));
                }
            }
            RiskLimitKind::MaxSlippage => {
                if value < Decimal::ZERO || value > Decimal::ONE {
                    return Err(RiskLimitUpdateError::new("value must be between 0 and 1"));
                }
            }
        }

        let mut limits = self.risk_limits.write();
        match kind {
            RiskLimitKind::MaxPositionPerMarket => limits.max_position_per_market = value,
            RiskLimitKind::MaxTotalExposure => limits.max_total_exposure = value,
            RiskLimitKind::MinProfitThreshold => limits.min_profit_threshold = value,
            RiskLimitKind::MaxSlippage => limits.max_slippage = value,
        }

        Ok(limits.clone())
    }

    /// Return true if the circuit breaker is currently active.
    pub fn is_circuit_breaker_active(&self) -> bool {
        self.circuit_breaker.load(Ordering::SeqCst)
    }

    /// Activate the circuit breaker, halting all new trades.
    pub fn activate_circuit_breaker(&self, reason: impl Into<String>) {
        self.circuit_breaker.store(true, Ordering::SeqCst);
        *self.circuit_breaker_reason.write() = Some(reason.into());
    }

    /// Deactivate the circuit breaker, resuming normal trading.
    pub fn reset_circuit_breaker(&self) {
        if self.reconciliation_required.load(Ordering::SeqCst) {
            return;
        }
        self.circuit_breaker.store(false, Ordering::SeqCst);
        *self.circuit_breaker_reason.write() = None;
    }

    /// Return the circuit breaker activation reason, if active.
    pub fn circuit_breaker_reason(&self) -> Option<String> {
        self.circuit_breaker_reason.read().clone()
    }

    /// Return the total exposure across all open positions.
    pub fn total_exposure(&self) -> Price {
        self.positions.read().total_exposure()
    }

    /// Return the count of currently open positions.
    pub fn open_position_count(&self) -> usize {
        self.positions.read().open_count()
    }

    /// Attempt to acquire an execution lock for a market.
    ///
    /// Returns `true` if the lock was acquired, `false` if the market
    /// already has an execution in progress.
    pub fn try_lock_execution(&self, market_id: &str) -> bool {
        let mut pending = self.pending_executions.lock();
        if !pending.is_empty() {
            return false;
        }
        pending.insert(market_id.to_string())
    }

    /// Release the execution lock for a market.
    pub fn release_execution(&self, market_id: &str) {
        self.pending_executions.lock().remove(market_id);
    }

    /// Return the count of markets with in-flight executions.
    pub fn pending_execution_count(&self) -> usize {
        self.pending_executions.lock().len()
    }

    /// Return the current pending (reserved but not yet committed) exposure.
    pub fn pending_exposure(&self) -> Price {
        *self.pending_exposure.lock()
    }

    /// Attempt to reserve exposure for an opportunity.
    ///
    /// Returns `true` if the reservation succeeded (within limits),
    /// `false` if it would exceed the maximum total exposure.
    pub fn try_reserve_exposure(&self, amount: Price) -> bool {
        let positions = self.positions.read();
        let mut pending = self.pending_exposure.lock();
        let current = positions.total_exposure();
        let limit = self.risk_limits.read().max_total_exposure;

        if current + *pending + amount > limit {
            return false;
        }

        *pending += amount;
        true
    }

    /// Release previously reserved exposure.
    pub fn release_exposure(&self, amount: Price) {
        let mut pending = self.pending_exposure.lock();
        *pending -= amount;
        // Ensure we don't go negative (safety check)
        if *pending < Decimal::ZERO {
            *pending = Decimal::ZERO;
        }
    }
}

impl RuntimeState for AppState {
    fn risk_limits(&self) -> RuntimeRiskLimits {
        app_limits_to_runtime(AppState::risk_limits(self))
    }

    fn set_risk_limit(
        &self,
        kind: RuntimeRiskLimitKind,
        value: Decimal,
    ) -> Result<RuntimeRiskLimits, RuntimeRiskLimitUpdateError> {
        AppState::set_risk_limit(self, runtime_kind_to_app(kind), value)
            .map(app_limits_to_runtime)
            .map_err(|err| RuntimeRiskLimitUpdateError::new(err.reason()))
    }

    fn is_circuit_breaker_active(&self) -> bool {
        AppState::is_circuit_breaker_active(self)
    }

    fn circuit_breaker_reason(&self) -> Option<String> {
        AppState::circuit_breaker_reason(self)
    }

    fn activate_circuit_breaker(&self, reason: &str) {
        AppState::activate_circuit_breaker(self, reason);
    }

    fn reset_circuit_breaker(&self) {
        AppState::reset_circuit_breaker(self);
    }

    fn open_position_count(&self) -> usize {
        AppState::open_position_count(self)
    }

    fn total_exposure(&self) -> Price {
        AppState::total_exposure(self)
    }

    fn pending_exposure(&self) -> Price {
        AppState::pending_exposure(self)
    }

    fn pending_execution_count(&self) -> usize {
        AppState::pending_execution_count(self)
    }

    fn active_positions(&self) -> Vec<RuntimePosition> {
        AppState::positions(self)
            .all()
            .filter(|p| !p.status().is_closed())
            .map(|p| RuntimePosition {
                market_id: p.market_id().as_str().to_string(),
                status: match p.status() {
                    PositionStatus::Open => RuntimePositionStatus::Open,
                    PositionStatus::PartialFill { .. } => RuntimePositionStatus::PartialFill,
                    PositionStatus::Closed { .. } => RuntimePositionStatus::Closed,
                },
                entry_cost: p.entry_cost(),
                expected_profit: p.expected_profit(),
            })
            .collect()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(RiskLimits::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
        assert!(!state.is_circuit_breaker_active());
        assert!(state.circuit_breaker_reason().is_none());
    }

    #[test]
    fn test_circuit_breaker() {
        let state = AppState::default();

        state.activate_circuit_breaker("test reason");
        assert!(state.is_circuit_breaker_active());
        assert_eq!(
            state.circuit_breaker_reason(),
            Some("test reason".to_string())
        );

        state.reset_circuit_breaker();
        assert!(!state.is_circuit_breaker_active());
        assert!(state.circuit_breaker_reason().is_none());
    }

    #[test]
    fn test_risk_limits_default() {
        let limits = RiskLimits::default();
        assert_eq!(limits.max_position_per_market, Decimal::from(1000));
        assert_eq!(limits.max_total_exposure, Decimal::from(10000));
    }

    #[test]
    fn test_set_risk_limit_updates_value() {
        let state = AppState::default();

        let updated = state
            .set_risk_limit(RiskLimitKind::MinProfitThreshold, dec!(0.25))
            .unwrap();

        assert_eq!(updated.min_profit_threshold, dec!(0.25));
        assert_eq!(state.risk_limits().min_profit_threshold, dec!(0.25));
    }

    #[test]
    fn test_set_risk_limit_rejects_invalid() {
        let state = AppState::default();

        let err = state
            .set_risk_limit(RiskLimitKind::MaxSlippage, dec!(1.5))
            .unwrap_err();

        assert_eq!(err.reason(), "value must be between 0 and 1");
        assert_eq!(state.risk_limits().max_slippage, Decimal::new(2, 2));
    }

    #[test]
    fn test_execution_locking() {
        let state = AppState::default();

        // First lock should succeed
        assert!(state.try_lock_execution("market-1"));

        // Second lock on same market should fail
        assert!(!state.try_lock_execution("market-1"));

        // Different market should succeed
        assert!(!state.try_lock_execution("market-2"));

        // After release, can lock again
        state.release_execution("market-1");
        assert!(state.try_lock_execution("market-1"));
    }

    #[test]
    fn test_total_exposure() {
        use crate::domain::{
            id::MarketId, id::TokenId, position::Position, position::PositionLeg,
            position::PositionStatus,
        };

        let state = AppState::default();

        // Initially zero
        assert_eq!(state.total_exposure(), Decimal::ZERO);

        // Add a position
        {
            let mut positions = state.positions_mut();
            let position = Position::new(
                positions.next_id(),
                MarketId::new("test"),
                vec![
                    PositionLeg::new(TokenId::new("yes"), dec!(100), dec!(0.45)),
                    PositionLeg::new(TokenId::new("no"), dec!(100), dec!(0.45)),
                ],
                dec!(90), // entry cost
                dec!(100),
                chrono::Utc::now(),
                PositionStatus::Open,
            );
            positions.add(position);
        }

        // Now has exposure
        assert_eq!(state.total_exposure(), dec!(90));
    }
}
