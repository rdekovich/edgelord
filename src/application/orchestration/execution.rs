//! Execution spawning workflow.
//!
//! Handles asynchronous trade execution in a background task, managing
//! success/failure outcomes, partial fills, and resource cleanup.

use std::sync::Arc;

use tokio::time::{timeout, Duration};
use tracing::error;

use super::position::{record_partial_position, record_position};
use crate::application::state::AppState;
use crate::domain::opportunity::Opportunity;
use crate::domain::stats::{TradeLeg, TradeOpenEvent};
use crate::domain::trade::TradeResult;
use crate::port::outbound::notifier::NotifierRegistry;
use crate::port::outbound::stats::StatsRecorder;
use crate::port::{
    outbound::exchange::ArbitrageExecutor, outbound::notifier::Event,
    outbound::notifier::ExecutionEvent,
};

/// RAII guard that releases the execution lock when dropped.
///
/// Ensures the market execution lock is released even if the execution
/// task panics or is cancelled.
struct ExecutionLockGuard {
    /// Shared application state containing the lock.
    state: Arc<AppState>,
    /// Market ID whose lock is held.
    market_id: String,
    completed: bool,
}

impl ExecutionLockGuard {
    /// Create a new guard (assumes lock is already held).
    fn new(state: Arc<AppState>, market_id: String) -> Self {
        Self {
            state,
            market_id,
            completed: false,
        }
    }
}

impl Drop for ExecutionLockGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.state.require_reconciliation(
                "Execution interrupted; reconcile durable intent before resuming",
            );
        }
        self.state.release_execution(&self.market_id);
    }
}

/// Spawn asynchronous trade execution without blocking the event loop.
///
/// Executes the opportunity in a background task, handling all outcomes:
/// - Success: Records position and updates exposure tracking
/// - Partial fill: Records actual inventory and halts for reconciliation
/// - Confirmed failure: Releases reserved exposure; timeout/unknown: retains it
///
/// Sends execution result notifications regardless of outcome.
pub(crate) fn spawn_execution(
    executor: Arc<dyn ArbitrageExecutor + Send + Sync>,
    opportunity: Opportunity,
    notifiers: Arc<NotifierRegistry>,
    state: Arc<AppState>,
    stats: Arc<dyn StatsRecorder>,
    opportunity_id: Option<i32>,
) {
    let market_id = opportunity.market_id().to_string();
    tokio::spawn(async move {
        let mut lock_guard = ExecutionLockGuard::new(Arc::clone(&state), market_id.clone());
        let reserved_exposure = opportunity.total_cost() * opportunity.volume();
        let intent_id = uuid::Uuid::new_v4().to_string();
        if let Err(error) = state.record_intent(&intent_id, &opportunity) {
            state.require_reconciliation(format!("Cannot persist execution intent: {error}"));
            state.release_exposure(reserved_exposure);
            return;
        }
        #[cfg(test)]
        let execution_timeout = Duration::from_millis(100);
        #[cfg(not(test))]
        let execution_timeout = Duration::from_secs(state.risk_limits().execution_timeout_secs);
        match timeout(execution_timeout, executor.execute_arbitrage(&opportunity)).await {
            Ok(Ok(result)) => {
                if !result.is_consistent_with(&opportunity) {
                    state.require_reconciliation(
                        "Invalid execution quantities; reservation retained",
                    );
                    return;
                }

                let fills = result.fills();
                let trade_id = opportunity_id.filter(|_| !fills.is_empty()).and_then(|id| {
                    stats.record_trade_open(&TradeOpenEvent {
                        opportunity_id: id,
                        strategy: opportunity.strategy().into(),
                        market_ids: vec![market_id.clone()],
                        legs: fills
                            .iter()
                            .map(|fill| TradeLeg {
                                token_id: fill.token_id.to_string(),
                                side: "buy".into(),
                                price: fill.price,
                                size: fill.amount,
                            })
                            .collect(),
                        size: opportunity.volume(),
                        expected_profit: opportunity.expected_profit(),
                    })
                });
                if let Err(error) = state.record_execution_result(&intent_id, &result, trade_id) {
                    state.require_reconciliation(format!(
                        "Cannot persist execution result: {error}; reconcile orders"
                    ));
                    return; // Intent remains durable, reservation remains held.
                }
                match &result {
                    TradeResult::Success { fills } => {
                        record_position(&state, &opportunity, fills, trade_id);
                    }
                    TradeResult::Partial { fills, failures } => {
                        // Cancellation does not undo a fill. Keep all confirmed inventory.
                        record_partial_position(&state, &opportunity, fills, failures, trade_id);
                        state.require_reconciliation(
                            "Partial execution; reconcile or hedge inventory before resuming",
                        );
                    }
                    TradeResult::Failed { .. } => {}
                }
                state.release_exposure(reserved_exposure);
                lock_guard.completed = true;
                stats.update_peak_exposure(state.total_exposure());
                notifiers.notify_all(Event::ExecutionCompleted(ExecutionEvent::from_result(
                    &market_id, &result,
                )));
            }
            ambiguous => {
                let reason = match ambiguous {
                    Ok(Err(error)) => error.to_string(),
                    Err(_) => "execution_timeout".into(),
                    _ => unreachable!(),
                };
                error!(%market_id, %reason, "Execution outcome unknown; reservation retained");
                state.require_reconciliation(format!(
                    "Unknown execution outcome: {reason}; reconcile before resuming"
                ));
                notifiers.notify_all(Event::ExecutionCompleted(ExecutionEvent {
                    market_id,
                    success: false,
                    details: reason,
                }));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::spawn_execution;
    use std::future::pending;
    use std::sync::Arc;

    use async_trait::async_trait;
    use rust_decimal_macros::dec;
    use tokio::time::{sleep, Duration, Instant};

    use crate::application::state::AppState;
    use crate::domain::id::{MarketId, OrderId, TokenId};
    use crate::domain::opportunity::{Opportunity, OpportunityLeg};
    use crate::domain::position::PositionStatus;
    use crate::domain::trade::{Failure, Fill, TradeResult};
    use crate::error::{Error, ExecutionError};
    use crate::port::outbound::exchange::ArbitrageExecutor;
    use crate::port::outbound::notifier::NotifierRegistry;
    use crate::testkit::stats::in_memory_stats_recorder;

    /// Mock executor that returns PartialFill and fails cancel on one leg.
    struct MockPartialFillExecutor {
        /// Order IDs that should fail cancellation.
        cancel_fail_order_ids: Vec<String>,
    }

    #[async_trait]
    impl ArbitrageExecutor for MockPartialFillExecutor {
        async fn execute_arbitrage(
            &self,
            _opportunity: &Opportunity,
        ) -> Result<TradeResult, Error> {
            Ok(TradeResult::Partial {
                fills: vec![
                    Fill::new(
                        TokenId::from("token-1"),
                        "order-1",
                        rust_decimal::Decimal::from(100),
                        rust_decimal::Decimal::new(40, 2),
                    ),
                    Fill::new(
                        TokenId::from("token-2"),
                        "order-2",
                        rust_decimal::Decimal::from(100),
                        rust_decimal::Decimal::new(45, 2),
                    ),
                ],
                failures: vec![Failure::new(TokenId::from("token-3"), "execution failed")],
            })
        }

        async fn cancel(&self, order_id: &OrderId) -> Result<(), Error> {
            if self
                .cancel_fail_order_ids
                .contains(&order_id.as_str().to_string())
            {
                Err(Error::Execution(ExecutionError::OrderRejected(
                    "cancel failed".to_string(),
                )))
            } else {
                Ok(())
            }
        }

        fn exchange_name(&self) -> &'static str {
            "mock"
        }
    }

    /// Mock executor that never returns to simulate a hang.
    struct MockHangingExecutor;

    #[async_trait]
    impl ArbitrageExecutor for MockHangingExecutor {
        async fn execute_arbitrage(
            &self,
            _opportunity: &Opportunity,
        ) -> Result<TradeResult, Error> {
            pending::<()>().await;
            unreachable!("pending should never resolve");
        }

        async fn cancel(&self, _order_id: &OrderId) -> Result<(), Error> {
            Ok(())
        }

        fn exchange_name(&self) -> &'static str {
            "mock"
        }
    }

    #[tokio::test]
    async fn partial_fill_is_recorded_regardless_of_cancel_capability() {
        let executor = Arc::new(MockPartialFillExecutor {
            cancel_fail_order_ids: vec!["order-1".to_string()],
        });

        let opportunity = Opportunity::with_strategy(
            MarketId::from("test-market"),
            "Test question?",
            vec![
                OpportunityLeg::new(TokenId::from("token-1"), dec!(0.40)),
                OpportunityLeg::new(TokenId::from("token-2"), dec!(0.50)),
                OpportunityLeg::new(TokenId::from("token-3"), dec!(0.10)),
            ],
            dec!(100),
            dec!(1.00),
            "test-strategy",
        );

        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let stats = in_memory_stats_recorder();

        spawn_execution(
            executor,
            opportunity.clone(),
            notifiers,
            state.clone(),
            stats,
            None,
        );

        let start = Instant::now();
        let timeout = Duration::from_secs(5);
        loop {
            if state.positions().all().next().is_some() {
                break;
            }
            if start.elapsed() > timeout {
                panic!("Timed out waiting for partial fill position");
            }
            sleep(Duration::from_millis(10)).await;
        }

        let positions = state.positions();
        let all_positions: Vec<_> = positions.all().collect();
        assert_eq!(
            all_positions.len(),
            1,
            "Expected exactly one position to be recorded"
        );

        let position = &all_positions[0];
        assert_eq!(
            position.market_id().as_str(),
            "test-market",
            "Position should have correct market ID"
        );

        match position.status() {
            PositionStatus::PartialFill { filled, missing } => {
                assert_eq!(filled.len(), 2, "Should have 2 filled legs");
                assert_eq!(missing.len(), 1, "Should have 1 missing leg");

                let filled_ids: Vec<&str> = filled.iter().map(|t| t.as_str()).collect();
                assert!(
                    filled_ids.contains(&"token-1"),
                    "token-1 should be in filled"
                );
                assert!(
                    filled_ids.contains(&"token-2"),
                    "token-2 should be in filled"
                );

                let missing_ids: Vec<&str> = missing.iter().map(|t| t.as_str()).collect();
                assert!(
                    missing_ids.contains(&"token-3"),
                    "token-3 should be in missing"
                );
            }
            _ => panic!(
                "Position should have PartialFill status, got {:?}",
                position.status()
            ),
        }
    }

    #[tokio::test]
    async fn execution_timeout_releases_lock() {
        let executor = Arc::new(MockHangingExecutor);
        let opportunity = Opportunity::with_strategy(
            MarketId::from("timeout-market"),
            "Timeout test?",
            vec![
                OpportunityLeg::new(TokenId::from("token-1"), dec!(0.40)),
                OpportunityLeg::new(TokenId::from("token-2"), dec!(0.50)),
            ],
            dec!(100),
            dec!(1.00),
            "test-strategy",
        );

        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let stats = in_memory_stats_recorder();

        assert!(state.try_lock_execution("timeout-market"));
        assert!(state.try_reserve_exposure(dec!(90)));

        spawn_execution(executor, opportunity, notifiers, state.clone(), stats, None);

        let start = Instant::now();
        let timeout = Duration::from_secs(1);
        loop {
            if state.try_lock_execution("timeout-market") {
                state.release_execution("timeout-market");
                assert_eq!(state.pending_exposure(), dec!(90));
                assert!(state.is_circuit_breaker_active());
                state.reset_circuit_breaker();
                assert!(state.is_circuit_breaker_active());
                break;
            }
            if start.elapsed() > timeout {
                panic!("Execution lock was not released after timeout");
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
}
