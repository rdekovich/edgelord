//! Market event flow for orchestration.
//!
//! Processes incoming market events and triggers appropriate actions:
//! order book updates, strategy detection, and position settlements.

use std::time::Instant;

use rust_decimal::Decimal;
use tracing::{debug, info, warn};

use super::context::MarketDetectionContext;
use super::handler::handle_opportunity;
use super::handler::MarketEventHandlingContext;

use crate::port::outbound::exchange::MarketEvent;

/// Process an incoming market event from the data stream.
///
/// Dispatches to the appropriate handler based on event type:
/// - Book snapshots/deltas: Update cache and run strategy detection
/// - Market settled: Close all positions for the market
/// - Connection events: Log status changes
pub(crate) fn handle_market_event(event: MarketEvent, context: MarketEventHandlingContext<'_>) {
    let start = Instant::now();

    match event {
        MarketEvent::BookSnapshot { token_id, book } => {
            context.cache.update(book);

            for market in context.registry.get_all_by_token(&token_id) {
                let ctx = MarketDetectionContext::new(market, context.cache);
                let opportunities = context.strategies.detect_opportunities(&ctx);

                debug!(
                    market_id = %market.market_id(),
                    opportunities_found = opportunities.len(),
                    "Strategy detection complete (snapshot)"
                );

                for opp in opportunities {
                    handle_opportunity(opp, context.opportunity_context());
                }
            }

            let elapsed = start.elapsed();
            context.stats.record_latency(elapsed.as_millis() as u32);
        }
        MarketEvent::BookDelta { token_id, book } => {
            context.cache.update(book);

            for market in context.registry.get_all_by_token(&token_id) {
                let ctx = MarketDetectionContext::new(market, context.cache);
                let opportunities = context.strategies.detect_opportunities(&ctx);

                debug!(
                    market_id = %market.market_id(),
                    opportunities_found = opportunities.len(),
                    "Strategy detection complete (delta)"
                );

                for opp in opportunities {
                    handle_opportunity(opp, context.opportunity_context());
                }
            }

            let elapsed = start.elapsed();
            context.stats.record_latency(elapsed.as_millis() as u32);
        }
        MarketEvent::MarketSettled {
            market_id,
            winning_outcome,
            payout_per_share,
        } => {
            info!(
                market_id = %market_id,
                winning_outcome = %winning_outcome,
                payout = %payout_per_share,
                "Market settled"
            );

            let Some(market) = context.registry.get_by_market_id(&market_id) else {
                context
                    .state
                    .require_reconciliation("Settlement for unknown market");
                return;
            };
            let winner = market.outcomes().iter().find(|o| {
                o.token_id().as_str() == winning_outcome
                    || o.name().eq_ignore_ascii_case(&winning_outcome)
            });
            let Some(winner) = winner else {
                context
                    .state
                    .require_reconciliation("Unknown winning token in settlement");
                return;
            };
            let payouts: Vec<_> = market
                .outcomes()
                .iter()
                .map(|outcome| {
                    (
                        outcome.token_id().clone(),
                        if outcome.token_id() == winner.token_id() {
                            payout_per_share
                        } else {
                            Decimal::ZERO
                        },
                    )
                })
                .collect();
            match context.state.settle_tokens(&payouts) {
                Ok(closes) => {
                    for (trade_id, pnl) in closes {
                        if let Some(trade_id) = trade_id {
                            context.stats.record_trade_close(
                                &crate::domain::stats::TradeCloseEvent {
                                    trade_id,
                                    realized_profit: pnl,
                                    reason: "resolution (redemption may remain pending)".into(),
                                },
                            );
                        }
                    }
                }
                Err(error) => context
                    .state
                    .require_reconciliation(format!("Cannot persist settlement: {error}")),
            }
            context.cache.clear();
        }
        MarketEvent::Connected => {
            info!("Data stream connected");
        }
        MarketEvent::Disconnected { reason } => {
            context.cache.clear();
            warn!(reason = %reason, "Data stream disconnected");
        }
    }
}
