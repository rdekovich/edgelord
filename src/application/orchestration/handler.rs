//! Event and opportunity handling facade.
//!
//! Provides the public interface for processing market events and opportunities,
//! delegating to internal modules for the actual implementation.

use std::sync::Arc;

use crate::application::cache::book::BookCache;
use crate::application::position::manager::PositionManager;
use crate::application::risk::manager::RiskManager;
use crate::application::state::AppState;
use crate::domain::{market::MarketRegistry, opportunity::Opportunity};
use crate::port::inbound::strategy::StrategyEngine;
use crate::port::outbound::exchange::{ArbitrageExecutor, MarketEvent};
use crate::port::outbound::notifier::NotifierRegistry;
use crate::port::outbound::stats::StatsRecorder;

/// Context containing all dependencies required to process a market event.
///
/// Bundles references to caches, registries, and services needed during
/// event processing. Created per-event to capture current state.
pub struct MarketEventHandlingContext<'a> {
    /// Order book cache for price lookups and updates.
    pub cache: &'a BookCache,
    /// Market registry for resolving token IDs to markets.
    pub registry: &'a MarketRegistry,
    /// Strategy engine for opportunity detection.
    pub strategies: &'a dyn StrategyEngine,
    /// Optional executor for live trading (None in dry-run mode).
    pub executor: Option<Arc<dyn ArbitrageExecutor + Send + Sync>>,
    /// Risk manager for pre-execution validation.
    pub risk_manager: &'a RiskManager,
    /// Notifier registry for sending events to observers.
    pub notifiers: &'a Arc<NotifierRegistry>,
    /// Shared application state.
    pub state: &'a Arc<AppState>,
    /// Statistics recorder for metrics.
    pub stats: &'a Arc<dyn StatsRecorder>,
    /// Position manager for settlement handling.
    pub position_manager: &'a Arc<PositionManager>,
    /// Whether to skip actual execution (log only).
    pub dry_run: bool,
}

/// Context containing dependencies required to process a detected opportunity.
///
/// A subset of [`MarketEventHandlingContext`] focused on opportunity evaluation
/// and execution.
pub(crate) struct OpportunityHandlingContext<'a> {
    /// Optional executor for live trading.
    pub executor: Option<Arc<dyn ArbitrageExecutor + Send + Sync>>,
    /// Risk manager for validation.
    pub risk_manager: &'a RiskManager,
    /// Notifier registry for events.
    pub notifiers: &'a Arc<NotifierRegistry>,
    /// Shared application state.
    pub state: &'a Arc<AppState>,
    /// Statistics recorder.
    pub stats: &'a Arc<dyn StatsRecorder>,
    /// Order book cache for slippage checks.
    pub cache: &'a BookCache,
    /// Whether to skip actual execution.
    pub dry_run: bool,
}

impl<'a> MarketEventHandlingContext<'a> {
    /// Extract an opportunity handling context from this event context.
    pub(crate) fn opportunity_context(&self) -> OpportunityHandlingContext<'a> {
        OpportunityHandlingContext {
            executor: self.executor.clone(),
            risk_manager: self.risk_manager,
            notifiers: self.notifiers,
            state: self.state,
            stats: self.stats,
            cache: self.cache,
            dry_run: self.dry_run,
        }
    }
}

/// Process an incoming market event from the data stream.
///
/// Delegates to the internal event module for actual processing.
pub(crate) fn handle_market_event(event: MarketEvent, context: MarketEventHandlingContext<'_>) {
    super::event::handle_market_event(event, context);
}

/// Process a detected opportunity through risk checks and execution.
///
/// Delegates to the internal opportunity module for actual processing.
pub(crate) fn handle_opportunity(opp: Opportunity, context: OpportunityHandlingContext<'_>) {
    super::opportunity::handle_opportunity(opp, context);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::application::cache::book::BookCache;
    use crate::application::orchestration::slippage::get_max_slippage;
    use crate::application::risk::manager::RiskManager;
    use crate::application::state::RiskLimits;
    use crate::application::strategy::registry::StrategyRegistry;
    use crate::domain::book::{Book, PriceLevel};
    use crate::domain::id::{MarketId, TokenId};
    use crate::domain::market::{Market, Outcome};
    use crate::domain::opportunity::{Opportunity, OpportunityLeg};
    use crate::port::outbound::notifier::NotifierRegistry;
    use crate::testkit::stats::in_memory_stats_recorder;

    fn make_order_book(token_id: &str, bid: Decimal, ask: Decimal) -> Book {
        Book::with_levels(
            TokenId::from(token_id),
            vec![PriceLevel::new(bid, Decimal::new(100, 0))],
            vec![PriceLevel::new(ask, Decimal::new(100, 0))],
        )
    }

    fn make_binary_market(
        id: &str,
        question: &str,
        yes_token: &str,
        no_token: &str,
        payout: Decimal,
    ) -> Market {
        let outcomes = vec![
            Outcome::new(TokenId::from(yes_token), "Yes"),
            Outcome::new(TokenId::from(no_token), "No"),
        ];
        Market::new(MarketId::from(id), question, outcomes, payout)
    }

    fn make_registry(markets: Vec<Market>) -> MarketRegistry {
        let mut registry = MarketRegistry::new();
        for market in markets {
            registry.add(market);
        }
        registry
    }

    fn make_test_opportunity() -> Opportunity {
        Opportunity::new(
            MarketId::from("test-market"),
            "Will it rain?",
            vec![
                OpportunityLeg::new(TokenId::from("yes-token"), dec!(0.40)),
                OpportunityLeg::new(TokenId::from("no-token"), dec!(0.50)),
            ],
            dec!(100),
            dec!(1.00),
        )
    }

    // ========== get_max_slippage tests ==========

    #[test]
    fn get_max_slippage_returns_none_when_token_not_in_cache() {
        let cache = BookCache::new();
        let opp = make_test_opportunity();
        let result = get_max_slippage(&opp, &cache);
        assert!(result.is_none());
    }

    #[test]
    fn get_max_slippage_returns_none_when_book_has_no_asks() {
        let cache = BookCache::new();
        // Create order book with bids but no asks
        let empty_ask_book = Book::with_levels(
            TokenId::from("yes-token"),
            vec![PriceLevel::new(dec!(0.40), dec!(100))],
            vec![], // no asks
        );
        cache.update(empty_ask_book);
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity();
        let result = get_max_slippage(&opp, &cache);
        assert!(result.is_none());
    }

    #[test]
    fn get_max_slippage_returns_none_when_expected_price_is_zero() {
        let cache = BookCache::new();
        cache.update(make_order_book("yes-token", dec!(0.39), dec!(0.40)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        // Create opportunity with zero price leg
        let opp = Opportunity::new(
            MarketId::from("test-market"),
            "Zero price test?",
            vec![
                OpportunityLeg::new(TokenId::from("yes-token"), dec!(0.00)),
                OpportunityLeg::new(TokenId::from("no-token"), dec!(0.50)),
            ],
            dec!(100),
            dec!(1.00),
        );

        let result = get_max_slippage(&opp, &cache);
        assert!(result.is_none());
    }

    #[test]
    fn get_max_slippage_returns_zero_when_prices_match() {
        let cache = BookCache::new();
        cache.update(make_order_book("yes-token", dec!(0.39), dec!(0.40)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity();
        let result = get_max_slippage(&opp, &cache);
        assert_eq!(result, Some(dec!(0)));
    }

    #[test]
    fn get_max_slippage_calculates_slippage_when_price_increased() {
        let cache = BookCache::new();
        // Price increased from 0.40 to 0.42 = 5% slippage
        cache.update(make_order_book("yes-token", dec!(0.41), dec!(0.42)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity();
        let result = get_max_slippage(&opp, &cache);
        assert_eq!(result, Some(dec!(0.05))); // (0.42 - 0.40) / 0.40 = 0.05
    }

    #[test]
    fn get_max_slippage_calculates_slippage_when_price_decreased() {
        let cache = BookCache::new();
        // Price decreased from 0.40 to 0.38 = 5% slippage (absolute)
        cache.update(make_order_book("yes-token", dec!(0.37), dec!(0.38)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity();
        let result = get_max_slippage(&opp, &cache);
        assert_eq!(result, Some(dec!(0.05))); // |0.38 - 0.40| / 0.40 = 0.05
    }

    #[test]
    fn get_max_slippage_returns_max_across_legs() {
        let cache = BookCache::new();
        // Yes: 5% slippage, No: 10% slippage
        cache.update(make_order_book("yes-token", dec!(0.41), dec!(0.42)));
        cache.update(make_order_book("no-token", dec!(0.54), dec!(0.55)));

        let opp = make_test_opportunity();
        let result = get_max_slippage(&opp, &cache);
        assert_eq!(result, Some(dec!(0.10))); // max(0.05, 0.10) = 0.10
    }

    // ========== handle_opportunity tests ==========

    #[test]
    fn handle_opportunity_skips_when_execution_already_locked() {
        let opp = make_test_opportunity();
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = RiskManager::new(Arc::clone(&state));
        let stats = in_memory_stats_recorder();
        let cache = BookCache::new();

        // Lock the market first
        state.try_lock_execution("test-market");

        // Try to handle opportunity - should return early
        handle_opportunity(
            opp,
            OpportunityHandlingContext {
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                cache: &cache,
                dry_run: true,
            },
        );

        // No notifications should have been sent (no OpportunityDetected)
        // The function returns early before notifying
    }

    #[test]
    fn handle_opportunity_rejects_high_slippage() {
        let state = Arc::new(AppState::new(RiskLimits {
            max_slippage: dec!(0.01), // 1% max slippage
            ..Default::default()
        }));
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = RiskManager::new(Arc::clone(&state));
        let stats = in_memory_stats_recorder();
        let cache = BookCache::new();

        // Set up cache with 5% slippage
        cache.update(make_order_book("yes-token", dec!(0.41), dec!(0.42)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity();

        handle_opportunity(
            opp,
            OpportunityHandlingContext {
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                cache: &cache,
                dry_run: true,
            },
        );

        // Execution lock should be released
        assert!(
            state.try_lock_execution("test-market"),
            "Lock should be released after slippage rejection"
        );
    }

    #[test]
    fn handle_opportunity_releases_lock_on_risk_rejection() {
        let state = Arc::new(AppState::new(RiskLimits {
            min_profit_threshold: dec!(100), // Require $100 profit minimum
            ..Default::default()
        }));
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = RiskManager::new(Arc::clone(&state));
        let stats = in_memory_stats_recorder();
        let cache = BookCache::new();

        cache.update(make_order_book("yes-token", dec!(0.39), dec!(0.40)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity(); // Expected profit = $10, below threshold

        handle_opportunity(
            opp,
            OpportunityHandlingContext {
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                cache: &cache,
                dry_run: true,
            },
        );

        // Lock should be released after risk rejection
        assert!(
            state.try_lock_execution("test-market"),
            "Lock should be released after risk rejection"
        );
    }

    #[test]
    fn handle_opportunity_dry_run_releases_lock_and_exposure() {
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = RiskManager::new(Arc::clone(&state));
        let stats = in_memory_stats_recorder();
        let cache = BookCache::new();

        cache.update(make_order_book("yes-token", dec!(0.39), dec!(0.40)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity();

        handle_opportunity(
            opp,
            OpportunityHandlingContext {
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                cache: &cache,
                dry_run: true,
            },
        );

        // Lock should be released
        assert!(
            state.try_lock_execution("test-market"),
            "Lock should be released after dry run"
        );

        // Exposure should be released (back to zero)
        assert_eq!(
            state.pending_exposure(),
            dec!(0),
            "Pending exposure should be zero after dry run"
        );
    }

    #[test]
    fn handle_opportunity_no_executor_releases_lock_and_exposure() {
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = RiskManager::new(Arc::clone(&state));
        let stats = in_memory_stats_recorder();
        let cache = BookCache::new();

        cache.update(make_order_book("yes-token", dec!(0.39), dec!(0.40)));
        cache.update(make_order_book("no-token", dec!(0.49), dec!(0.50)));

        let opp = make_test_opportunity();

        handle_opportunity(
            opp,
            OpportunityHandlingContext {
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                cache: &cache,
                dry_run: false,
            },
        );

        // Lock should be released
        assert!(
            state.try_lock_execution("test-market"),
            "Lock should be released when no executor"
        );
    }

    // ========== handle_market_event tests ==========

    #[test]
    fn handle_market_event_updates_cache_on_snapshot() {
        let cache = Arc::new(BookCache::new());
        let registry = Arc::new(make_registry(vec![make_binary_market(
            "market-1",
            "Test?",
            "yes-1",
            "no-1",
            dec!(1.00),
        )]));
        let strategies = StrategyRegistry::new();
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = Arc::new(RiskManager::new(Arc::clone(&state)));
        let stats = in_memory_stats_recorder();
        let position_manager = Arc::new(
            crate::application::position::manager::PositionManager::new(Arc::clone(&stats)),
        );

        let book = make_order_book("yes-1", dec!(0.40), dec!(0.42));

        handle_market_event(
            MarketEvent::BookSnapshot {
                token_id: TokenId::from("yes-1"),
                book,
            },
            MarketEventHandlingContext {
                cache: &cache,
                registry: &registry,
                strategies: &strategies,
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                position_manager: &position_manager,
                dry_run: true,
            },
        );

        // Cache should have the order book
        let cached = cache.get(&TokenId::from("yes-1"));
        assert!(cached.is_some(), "Order book should be in cache");
        assert_eq!(
            cached.unwrap().best_ask().unwrap().price(),
            dec!(0.42),
            "Cached ask price should match"
        );
    }

    #[test]
    fn handle_market_event_updates_cache_on_delta() {
        let cache = Arc::new(BookCache::new());
        let registry = Arc::new(make_registry(vec![make_binary_market(
            "market-1",
            "Test?",
            "yes-1",
            "no-1",
            dec!(1.00),
        )]));
        let strategies = StrategyRegistry::new();
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = Arc::new(RiskManager::new(Arc::clone(&state)));
        let stats = in_memory_stats_recorder();
        let position_manager = Arc::new(
            crate::application::position::manager::PositionManager::new(Arc::clone(&stats)),
        );

        let book = make_order_book("yes-1", dec!(0.40), dec!(0.42));

        handle_market_event(
            MarketEvent::BookDelta {
                token_id: TokenId::from("yes-1"),
                book,
            },
            MarketEventHandlingContext {
                cache: &cache,
                registry: &registry,
                strategies: &strategies,
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                position_manager: &position_manager,
                dry_run: true,
            },
        );

        // Cache should have the order book
        let cached = cache.get(&TokenId::from("yes-1"));
        assert!(cached.is_some(), "Order book should be in cache");
    }

    #[test]
    fn handle_market_event_settles_positions() {
        let cache = Arc::new(BookCache::new());
        let mut registry = MarketRegistry::new();
        registry.add(crate::domain::market::Market::new(
            MarketId::from("settled-market"),
            "Settled?",
            vec![
                crate::domain::market::Outcome::new(TokenId::from("yes-1"), "Yes"),
                crate::domain::market::Outcome::new(TokenId::from("no-1"), "No"),
            ],
            dec!(1),
        ));
        let registry = Arc::new(registry);
        let strategies = StrategyRegistry::new();
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = Arc::new(RiskManager::new(Arc::clone(&state)));
        let stats = in_memory_stats_recorder();
        let position_manager = Arc::new(
            crate::application::position::manager::PositionManager::new(Arc::clone(&stats)),
        );

        // Add a position to close
        {
            use crate::domain::{
                position::Position, position::PositionLeg, position::PositionStatus,
            };
            let mut positions = state.positions_mut();
            let legs = vec![
                PositionLeg::new(TokenId::from("yes-1"), dec!(100), dec!(0.40)),
                PositionLeg::new(TokenId::from("no-1"), dec!(100), dec!(0.50)),
            ];
            let position = Position::new(
                positions.next_id(),
                MarketId::from("settled-market"),
                legs,
                dec!(90),
                dec!(100),
                chrono::Utc::now(),
                PositionStatus::Open,
            );
            positions.add(position);
        }

        // Verify position exists
        assert_eq!(state.positions().all().count(), 1);

        handle_market_event(
            MarketEvent::MarketSettled {
                market_id: MarketId::from("settled-market"),
                winning_outcome: "Yes".to_string(),
                payout_per_share: dec!(1.00),
            },
            MarketEventHandlingContext {
                cache: &cache,
                registry: &registry,
                strategies: &strategies,
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                position_manager: &position_manager,
                dry_run: true,
            },
        );

        // Position should be closed
        let positions = state.positions();
        let open_positions: Vec<_> = positions.all().collect();
        assert!(
            open_positions.is_empty()
                || open_positions
                    .iter()
                    .all(|p| !matches!(p.status(), crate::domain::position::PositionStatus::Open)),
            "Position should be closed after settlement"
        );
    }

    #[test]
    fn handle_market_event_connected_does_not_panic() {
        let cache = Arc::new(BookCache::new());
        let registry = Arc::new(MarketRegistry::new());
        let strategies = StrategyRegistry::new();
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = Arc::new(RiskManager::new(Arc::clone(&state)));
        let stats = in_memory_stats_recorder();
        let position_manager = Arc::new(
            crate::application::position::manager::PositionManager::new(Arc::clone(&stats)),
        );

        // Should not panic
        handle_market_event(
            MarketEvent::Connected,
            MarketEventHandlingContext {
                cache: &cache,
                registry: &registry,
                strategies: &strategies,
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                position_manager: &position_manager,
                dry_run: true,
            },
        );
    }

    #[test]
    fn handle_market_event_disconnected_does_not_panic() {
        let cache = Arc::new(BookCache::new());
        let registry = Arc::new(MarketRegistry::new());
        let strategies = StrategyRegistry::new();
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = Arc::new(RiskManager::new(Arc::clone(&state)));
        let stats = in_memory_stats_recorder();
        let position_manager = Arc::new(
            crate::application::position::manager::PositionManager::new(Arc::clone(&stats)),
        );

        // Should not panic
        handle_market_event(
            MarketEvent::Disconnected {
                reason: "Test disconnect".to_string(),
            },
            MarketEventHandlingContext {
                cache: &cache,
                registry: &registry,
                strategies: &strategies,
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                position_manager: &position_manager,
                dry_run: true,
            },
        );
    }

    #[test]
    fn handle_market_event_ignores_unknown_token() {
        let cache = Arc::new(BookCache::new());
        let registry = Arc::new(MarketRegistry::new()); // Empty registry
        let strategies = StrategyRegistry::new();
        let state = Arc::new(AppState::default());
        let notifiers = Arc::new(NotifierRegistry::new());
        let risk_manager = Arc::new(RiskManager::new(Arc::clone(&state)));
        let stats = in_memory_stats_recorder();
        let position_manager = Arc::new(
            crate::application::position::manager::PositionManager::new(Arc::clone(&stats)),
        );

        let book = make_order_book("unknown-token", dec!(0.40), dec!(0.42));

        // Should not panic, just update cache and skip strategy detection
        handle_market_event(
            MarketEvent::BookSnapshot {
                token_id: TokenId::from("unknown-token"),
                book,
            },
            MarketEventHandlingContext {
                cache: &cache,
                registry: &registry,
                strategies: &strategies,
                executor: None,
                risk_manager: &risk_manager,
                notifiers: &notifiers,
                state: &state,
                stats: &stats,
                position_manager: &position_manager,
                dry_run: true,
            },
        );

        // Cache should still be updated
        assert!(cache.get(&TokenId::from("unknown-token")).is_some());
    }
}
