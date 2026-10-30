//! Slippage calculations for opportunity handling.
//!
//! Computes price movement between opportunity detection and execution
//! to prevent trading on stale signals.

use rust_decimal::Decimal;

use crate::application::cache::book::BookCache;
use crate::domain::opportunity::Opportunity;

/// Calculate the maximum slippage across all opportunity legs.
///
/// Slippage is computed as the absolute percentage change between the
/// expected price (from detection) and the current price (from cache).
///
/// Returns `None` if any required order book is missing or has no asks,
/// or if any expected price is zero.
pub(crate) fn get_max_slippage(opportunity: &Opportunity, cache: &BookCache) -> Option<Decimal> {
    let mut max_slippage = Decimal::ZERO;

    for leg in opportunity.legs() {
        let book = cache.get(leg.token_id())?;
        let ask = book.best_ask()?;
        if ask.size() < opportunity.volume() {
            return None;
        }
        let current_price = ask.price();
        let expected_price = leg.ask_price();

        if expected_price == Decimal::ZERO {
            return None;
        }

        let slippage = ((current_price - expected_price).abs()) / expected_price;
        max_slippage = max_slippage.max(slippage);
    }

    Some(max_slippage)
}
