//! Opportunity decision flow for orchestration.
//!
//! Evaluates detected opportunities through slippage checks, risk validation,
//! and execution routing.

use std::sync::Arc;

use tracing::{debug, info};

use super::execution::spawn_execution;
use super::handler::OpportunityHandlingContext;
use super::slippage::get_max_slippage;
use crate::domain::{opportunity::Opportunity, stats::RecordedOpportunity};
use crate::error::RiskError;
use crate::port::inbound::risk::RiskCheckResult;
use crate::port::outbound::notifier::{Event, OpportunityEvent, RiskEvent};

/// Process a detected opportunity through validation and execution.
///
/// Performs the following steps:
/// 1. Acquire execution lock (skip if market already being executed)
/// 2. Check slippage against configured threshold
/// 3. Validate opportunity against risk manager
/// 4. Either spawn execution (live mode) or log (dry-run mode)
///
/// Releases locks and reserved exposure on all exit paths.
pub(crate) fn handle_opportunity(mut opp: Opportunity, context: OpportunityHandlingContext<'_>) {
    let OpportunityHandlingContext {
        executor,
        risk_manager,
        notifiers,
        state,
        stats,
        cache,
        dry_run,
    } = context;

    if !state.try_lock_execution(opp.market_id().as_str()) {
        debug!(market_id = %opp.market_id(), "Execution already in progress, skipping");
        return;
    }

    let limits = state.risk_limits();
    let held: rust_decimal::Decimal = state
        .positions()
        .open_positions()
        .filter(|position| {
            position.market_id() == opp.market_id()
                || position.legs().iter().any(|held| {
                    opp.legs()
                        .iter()
                        .any(|leg| held.token_id() == leg.token_id())
                })
        })
        .map(|p| p.entry_cost())
        .sum();
    let budget = (limits.max_position_per_market - held)
        .min(limits.max_total_exposure - state.total_exposure() - state.pending_exposure());
    if opp.total_cost() <= rust_decimal::Decimal::ZERO || budget <= rust_decimal::Decimal::ZERO {
        state.release_execution(opp.market_id().as_str());
        return;
    }
    opp.set_volume(
        opp.volume()
            .min(budget / opp.total_cost())
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::ToZero),
    );
    if opp.volume() <= rust_decimal::Decimal::ZERO {
        state.release_execution(opp.market_id().as_str());
        return;
    }

    if get_max_slippage(&opp, cache).is_none() {
        state.release_execution(opp.market_id().as_str());
        return;
    }

    let max_slippage = state.risk_limits().max_slippage;
    if let Some(slippage) = get_max_slippage(&opp, cache) {
        if slippage > max_slippage {
            debug!(
                market_id = %opp.market_id(),
                slippage = %slippage,
                max = %max_slippage,
                "Slippage check failed, rejecting opportunity"
            );
            state.release_execution(opp.market_id().as_str());

            stats.record_opportunity(&RecordedOpportunity {
                strategy: opp.strategy().to_string(),
                market_ids: vec![opp.market_id().to_string()],
                edge: opp.edge(),
                expected_profit: opp.expected_profit(),
                executed: false,
                rejected_reason: Some("slippage_too_high".to_string()),
            });

            let error = RiskError::SlippageTooHigh {
                actual: slippage,
                max: max_slippage,
            };
            notifiers.notify_all(Event::RiskRejected(RiskEvent::new(
                opp.market_id().as_str(),
                &error,
            )));
            return;
        }
    }

    notifiers.notify_all(Event::OpportunityDetected(OpportunityEvent::from(&opp)));

    match risk_manager.check(&opp) {
        RiskCheckResult::Approved => {
            let opp_id = stats.record_opportunity(&RecordedOpportunity {
                strategy: opp.strategy().to_string(),
                market_ids: vec![opp.market_id().to_string()],
                edge: opp.edge(),
                expected_profit: opp.expected_profit(),
                executed: !dry_run,
                rejected_reason: None,
            });

            let reserved_exposure = opp.total_cost() * opp.volume();

            if dry_run {
                info!(
                    market_id = %opp.market_id(),
                    edge = %opp.edge(),
                    profit = %opp.expected_profit(),
                    "Dry-run: would execute trade"
                );
                state.release_exposure(reserved_exposure);
                state.release_execution(opp.market_id().as_str());
            } else if let Some(exec) = executor {
                spawn_execution(
                    exec,
                    opp,
                    notifiers.clone(),
                    state.clone(),
                    Arc::clone(stats),
                    opp_id,
                );
            } else {
                state.release_exposure(reserved_exposure);
                state.release_execution(opp.market_id().as_str());
            }
        }
        RiskCheckResult::Rejected(error) => {
            stats.record_opportunity(&RecordedOpportunity {
                strategy: opp.strategy().to_string(),
                market_ids: vec![opp.market_id().to_string()],
                edge: opp.edge(),
                expected_profit: opp.expected_profit(),
                executed: false,
                rejected_reason: Some(format!("{error}")),
            });

            state.release_execution(opp.market_id().as_str());
            notifiers.notify_all(Event::RiskRejected(RiskEvent::new(
                opp.market_id().as_str(),
                &error,
            )));
        }
    }
}
