//! Position recording helpers for the execution flow.
//!
//! Converts executed opportunities into tracked positions for PnL
//! monitoring and settlement handling.

use rust_decimal::Decimal;

use crate::application::state::AppState;
use crate::domain::id::TokenId;
use crate::domain::opportunity::Opportunity;
use crate::domain::position::{Position, PositionLeg, PositionStatus};
use crate::domain::trade::{Failure, Fill};

/// Record a fully executed position in shared state.
///
/// Creates a new position from the opportunity with all legs filled,
/// generating a unique position ID and adding it to the tracker.
pub(crate) fn record_position(
    state: &AppState,
    opportunity: &Opportunity,
    fills: &[Fill],
    trade_id: Option<i32>,
) {
    let position_legs: Vec<PositionLeg> = fills
        .iter()
        .map(|fill| PositionLeg::new(fill.token_id.clone(), fill.amount, fill.price))
        .collect();
    let entry_cost: Decimal = position_legs.iter().map(PositionLeg::cost).sum();
    let volume = fills
        .iter()
        .map(|f| f.amount)
        .min()
        .unwrap_or(Decimal::ZERO);

    let mut positions = state.positions_mut();
    let mut position = Position::new(
        positions.next_id(),
        opportunity.market_id().clone(),
        position_legs,
        entry_cost,
        opportunity.payout() * volume,
        chrono::Utc::now(),
        PositionStatus::Open,
    );

    if let Some(tid) = trade_id {
        position = position.with_trade_id(tid);
    }

    positions.add(position);
}

/// Record a partial-fill position in shared state.
///
/// Creates a position with `PartialFill` status, tracking which legs were
/// filled successfully and which failed. Used when execution partially
/// completes and cancellation fails.
pub(crate) fn record_partial_position(
    state: &AppState,
    opportunity: &Opportunity,
    fills: &[Fill],
    failures: &[Failure],
    trade_id: Option<i32>,
) {
    let filled_token_ids: Vec<TokenId> = fills.iter().map(|f| f.token_id.clone()).collect();
    let missing_token_ids: Vec<TokenId> = failures.iter().map(|f| f.token_id.clone()).collect();

    let position_legs: Vec<PositionLeg> = fills
        .iter()
        .map(|fill| PositionLeg::new(fill.token_id.clone(), fill.amount, fill.price))
        .collect();

    let entry_cost: Decimal = position_legs
        .iter()
        .map(|l| l.entry_price() * l.size())
        .sum();

    let mut positions = state.positions_mut();
    let mut position = Position::new(
        positions.next_id(),
        opportunity.market_id().clone(),
        position_legs,
        entry_cost,
        Decimal::ZERO,
        chrono::Utc::now(),
        PositionStatus::PartialFill {
            filled: filled_token_ids,
            missing: missing_token_ids,
        },
    );

    if let Some(tid) = trade_id {
        position = position.with_trade_id(tid);
    }

    positions.add(position);
}
