//! Behavioral acceptance tests: quotes -> strategy -> guaranteed payoff,
//! and durable execution -> restart -> actual settlement cashflows.
use chrono::Utc;
use edgelord::{
    adapter::outbound::journal::FileExecutionJournal,
    application::{
        cache::book::BookCache,
        cluster::{detector::ClusterDetector, service::ClusterDetectionConfig},
        state::{AppState, RiskLimits},
        strategy::{
            context::ConcreteDetectionContext,
            market_rebalancing::{MarketRebalancingConfig, MarketRebalancingStrategy},
            single_condition::{SingleConditionConfig, SingleConditionStrategy},
        },
    },
    domain::{
        book::{Book, PriceLevel},
        cluster::Cluster,
        constraint::Constraint,
        id::{ClusterId, MarketId, TokenId},
        market::{Market, MarketRegistry, Outcome},
        opportunity::{Opportunity, OpportunityLeg},
        trade::{Failure, Fill, TradeResult},
    },
    port::{
        inbound::strategy::Strategy,
        outbound::{
            journal::{ExecutionJournal, ExecutionRecord},
            solver::{IlpProblem, ProjectionResult, ProjectionSolver},
        },
    },
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::sync::Arc;
struct UnusedProjection;
impl ProjectionSolver for UnusedProjection {
    fn name(&self) -> &'static str {
        "must-not-price-orders"
    }
    fn project(&self, _: &[Decimal], _: &IlpProblem) -> edgelord::error::Result<ProjectionResult> {
        panic!("Projection output cannot be used as an executable quote")
    }
}
fn binary(id: &str) -> Market {
    Market::new(
        MarketId::new(id),
        id,
        vec![
            Outcome::new(TokenId::new(format!("{id}-yes")), "Yes"),
            Outcome::new(TokenId::new(format!("{id}-no")), "No"),
        ],
        dec!(1),
    )
}
fn quote(cache: &BookCache, token: &str, price: Decimal) {
    cache.update(Book::with_levels(
        TokenId::new(token),
        vec![],
        vec![PriceLevel::new(price, dec!(100))],
    ));
}
fn opportunity() -> Opportunity {
    Opportunity::with_strategy(
        MarketId::new("a"),
        "A?",
        vec![
            OpportunityLeg::new(TokenId::new("a-yes"), dec!(0.4)),
            OpportunityLeg::new(TokenId::new("a-no"), dec!(0.5)),
        ],
        dec!(100),
        dec!(1),
        "single_condition",
    )
}
#[test]
fn complete_sets_use_executable_asks_and_guaranteed_payouts() {
    let cache = BookCache::new();
    quote(&cache, "a-yes", dec!(0.4));
    quote(&cache, "a-no", dec!(0.5));
    let market = binary("a");
    let result = SingleConditionStrategy::new(SingleConditionConfig::default())
        .detect(&ConcreteDetectionContext::new(&market, &cache));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].expected_profit(), dec!(10));
    let event = Market::new(
        MarketId::new("event"),
        "Exactly one",
        (0..3)
            .map(|i| Outcome::new(TokenId::new(format!("e{i}")), format!("{i}")))
            .collect(),
        dec!(1),
    );
    for i in 0..3 {
        quote(&cache, &format!("e{i}"), dec!(0.3));
    }
    let result = MarketRebalancingStrategy::new(MarketRebalancingConfig::default())
        .detect(&ConcreteDetectionContext::new(&event, &cache));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].expected_profit(), dec!(10));
    for winner in 0..3 {
        let payout: Decimal = result[0]
            .legs()
            .iter()
            .map(|leg| {
                if leg.token_id().as_str() == format!("e{winner}") {
                    result[0].volume()
                } else {
                    Decimal::ZERO
                }
            })
            .sum();
        assert_eq!(
            payout - result[0].total_cost() * result[0].volume(),
            dec!(10)
        );
    }
}
#[test]
fn implication_portfolio_buys_no_antecedent_and_yes_consequent() {
    let mut registry = MarketRegistry::new();
    registry.add(binary("a"));
    registry.add(binary("b"));
    let cache = BookCache::new();
    quote(&cache, "a-yes", dec!(0.8));
    quote(&cache, "a-no", dec!(0.25));
    quote(&cache, "b-yes", dec!(0.4));
    quote(&cache, "b-no", dec!(0.65));
    let cluster = Cluster {
        id: ClusterId::new(),
        markets: vec![MarketId::new("a"), MarketId::new("b")],
        relations: vec![],
        constraints: vec![Constraint::leq(vec![dec!(1), dec!(-1)], dec!(0))],
        updated_at: Utc::now(),
    };
    let detector = ClusterDetector::new(
        ClusterDetectionConfig::default(),
        Arc::new(UnusedProjection),
    );
    let result = detector
        .detect(&cluster, &|token| cache.get(token), &registry)
        .unwrap()
        .unwrap()
        .opportunity;
    assert_eq!(result.total_cost(), dec!(0.65));
    assert_eq!(result.volume(), dec!(100));
    for (a, b) in [(false, false), (false, true), (true, true)] {
        let payout: Decimal = result
            .legs()
            .iter()
            .map(|leg| match leg.token_id().as_str() {
                "a-no" if !a => dec!(100),
                "b-yes" if b => dec!(100),
                _ => dec!(0),
            })
            .sum();
        assert!(payout - result.total_cost() * result.volume() >= dec!(35));
    }
}
#[test]
fn mutual_exclusion_buys_no_tokens_and_covers_neither_winning() {
    let mut registry = MarketRegistry::new();
    registry.add(binary("a"));
    registry.add(binary("b"));
    let cache = BookCache::new();
    for id in ["a", "b"] {
        quote(&cache, &format!("{id}-yes"), dec!(0.7));
        quote(&cache, &format!("{id}-no"), dec!(0.35));
    }
    let cluster = Cluster {
        id: ClusterId::new(),
        markets: vec![MarketId::new("a"), MarketId::new("b")],
        relations: vec![],
        constraints: vec![Constraint::leq(vec![dec!(1), dec!(1)], dec!(1))],
        updated_at: Utc::now(),
    };
    let result = ClusterDetector::new(
        ClusterDetectionConfig::default(),
        Arc::new(UnusedProjection),
    )
    .detect(&cluster, &|token| cache.get(token), &registry)
    .unwrap()
    .unwrap()
    .opportunity;
    assert!(result
        .legs()
        .iter()
        .all(|leg| leg.token_id().as_str().ends_with("-no")));
    assert_eq!(result.payout(), dec!(1));
    assert_eq!(result.expected_profit(), dec!(30));
}
#[test]
fn replay_preserves_inventory_and_actual_cost_and_settlement_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger");
    {
        let journal = FileExecutionJournal::open(&path).unwrap();
        journal
            .append(&ExecutionRecord::Intent {
                id: "1".into(),
                opportunity: opportunity(),
            })
            .unwrap();
        journal
            .append(&ExecutionRecord::Outcome {
                trade_id: Some(42),
                id: "1".into(),
                result: TradeResult::Success {
                    fills: vec![
                        Fill::new(TokenId::new("a-yes"), "y", dec!(100), dec!(0.39)),
                        Fill::new(TokenId::new("a-no"), "n", dec!(100), dec!(0.49)),
                    ],
                },
            })
            .unwrap();
    }
    {
        let state = AppState::with_journal(
            RiskLimits::default(),
            Arc::new(FileExecutionJournal::open(&path).unwrap()),
        )
        .unwrap();
        assert_eq!(state.total_exposure(), dec!(88));
        assert_eq!(
            state
                .positions()
                .open_positions()
                .next()
                .unwrap()
                .trade_id(),
            Some(42)
        );
        let payouts = vec![
            (TokenId::new("a-yes"), dec!(0)),
            (TokenId::new("a-no"), dec!(1)),
        ];
        assert_eq!(state.settle_tokens(&payouts).unwrap()[0].1, dec!(12));
        assert!(state.settle_tokens(&payouts).unwrap().is_empty());
        assert_eq!(state.total_exposure(), dec!(0));
    }
    let state = AppState::with_journal(
        RiskLimits::default(),
        Arc::new(FileExecutionJournal::open(&path).unwrap()),
    )
    .unwrap();
    assert_eq!(state.total_exposure(), dec!(0));
    assert_eq!(state.positions().closed_positions().count(), 1);
}
#[test]
fn unresolved_intent_survives_restart_and_cannot_be_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger");
    let journal = Arc::new(FileExecutionJournal::open(&path).unwrap());
    journal
        .append(&ExecutionRecord::Intent {
            id: "lost-response".into(),
            opportunity: opportunity(),
        })
        .unwrap();
    let state = AppState::with_journal(RiskLimits::default(), journal).unwrap();
    assert_eq!(state.pending_exposure(), dec!(90));
    assert!(state.is_circuit_breaker_active());
    state.reset_circuit_breaker();
    assert!(state.is_circuit_breaker_active());
}
#[test]
fn losing_partial_fill_records_a_loss_not_a_guaranteed_profit() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Arc::new(FileExecutionJournal::open(&dir.path().join("ledger")).unwrap());
    journal
        .append(&ExecutionRecord::Intent {
            id: "1".into(),
            opportunity: opportunity(),
        })
        .unwrap();
    journal
        .append(&ExecutionRecord::Outcome {
            trade_id: Some(42),
            id: "1".into(),
            result: TradeResult::Partial {
                fills: vec![Fill::new(TokenId::new("a-yes"), "y", dec!(40), dec!(0.4))],
                failures: vec![Failure::new(TokenId::new("a-no"), "rejected")],
            },
        })
        .unwrap();
    let state = AppState::with_journal(RiskLimits::default(), journal).unwrap();
    assert_eq!(state.total_exposure(), dec!(16));
    let closed = state
        .settle_tokens(&[
            (TokenId::new("a-yes"), dec!(0)),
            (TokenId::new("a-no"), dec!(1)),
        ])
        .unwrap();
    assert_eq!(closed[0].1, dec!(-16));
}

#[tokio::test]
async fn websocket_transport_applies_absolute_changes_in_order() {
    use edgelord::{
        adapter::outbound::polymarket::stream::PolymarketDataStream,
        port::outbound::exchange::{MarketDataStream, MarketEvent},
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        let subscription = ws.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(subscription.contains("custom_feature_enabled"));
        ws.send(Message::Text(r#"[{"asset_id":"123","bids":[],"asks":[{"price":"0.6","size":"50"},{"price":"0.4","size":"100"}],"timestamp":"1000"}]"#.into())).await.unwrap();
        ws.send(Message::Text(r#"{"event_type":"price_change","timestamp":"1001","price_changes":[{"asset_id":"123","price":"0.4","size":"0","side":"SELL"},{"asset_id":"123","price":"0.5","size":"20","side":"SELL"}]}"#.into())).await.unwrap();
        // A delayed event must not restore an obsolete quote.
        ws.send(Message::Text(r#"{"event_type":"price_change","timestamp":"999","price_changes":[{"asset_id":"123","price":"0.1","size":"100","side":"SELL"}]}"#.into())).await.unwrap();
        ws.send(Message::Text(
            r#"{"event_type":"market_resolved","market":"a","winning_asset_id":"123"}"#.into(),
        ))
        .await
        .unwrap();
        while let Some(Ok(message)) = ws.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    });
    let mut stream = PolymarketDataStream::new(format!("ws://{address}"));
    stream.connect().await.unwrap();
    stream.subscribe(&[TokenId::new("123")]).await.unwrap();
    let event = stream.next_event().await.unwrap();
    assert_eq!(
        event.order_book().unwrap().best_ask().unwrap().price(),
        dec!(0.4)
    );
    let event = stream.next_event().await.unwrap();
    assert_eq!(
        event.order_book().unwrap().best_ask().unwrap().price(),
        dec!(0.6)
    );
    let event = stream.next_event().await.unwrap();
    assert_eq!(
        event.order_book().unwrap().best_ask().unwrap().price(),
        dec!(0.5)
    );
    assert_eq!(
        event.order_book().unwrap().best_ask().unwrap().size(),
        dec!(20)
    );
    assert!(matches!(
        stream.next_event().await,
        Some(MarketEvent::MarketSettled { .. })
    ));
    server.abort();
}

#[test]
fn return_to_previous_quote_is_not_deduplicated() {
    use edgelord::{
        adapter::outbound::polymarket::{
            dedup::PolymarketDeduplicator, settings::PolymarketDedupConfig,
        },
        port::outbound::{dedup::MessageDeduplicator, exchange::MarketEvent},
    };
    let dedup = PolymarketDeduplicator::new(&PolymarketDedupConfig {
        enabled: true,
        ..Default::default()
    });
    let event = |price| MarketEvent::BookSnapshot {
        token_id: TokenId::new("a"),
        book: Book::with_levels(
            TokenId::new("a"),
            vec![],
            vec![PriceLevel::new(price, dec!(100))],
        ),
    };
    assert!(!dedup.is_duplicate(&event(dec!(0.4))));
    assert!(dedup.is_duplicate(&event(dec!(0.4))));
    assert!(!dedup.is_duplicate(&event(dec!(0.5))));
    assert!(!dedup.is_duplicate(&event(dec!(0.4))));
}

#[test]
fn account_binding_rejects_another_wallet() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::with_journal(
        RiskLimits::default(),
        Arc::new(FileExecutionJournal::open(&dir.path().join("ledger")).unwrap()),
    )
    .unwrap();
    state.bind_account("137:wallet-a").unwrap();
    state.bind_account("137:wallet-a").unwrap();
    assert!(state.bind_account("137:wallet-b").is_err());
}

#[test]
fn malformed_success_cannot_create_inventory() {
    let bad = TradeResult::Success {
        fills: vec![Fill::new(TokenId::new("a-yes"), "y", dec!(40), dec!(0.4))],
    };
    assert!(!bad.is_consistent_with(&opportunity()));
    let overprice = TradeResult::Success {
        fills: vec![
            Fill::new(TokenId::new("a-yes"), "y", dec!(100), dec!(0.9)),
            Fill::new(TokenId::new("a-no"), "n", dec!(100), dec!(0.5)),
        ],
    };
    assert!(!overprice.is_consistent_with(&opportunity()));
}

#[test]
fn inferred_relations_cannot_enter_live_strategy_registry() {
    use edgelord::{
        application::cache::cluster::ClusterCache,
        domain::relation::{Relation, RelationKind},
        infrastructure::{config::settings::Config, factory::strategy::build_strategy_registry},
    };
    let mut config = Config::default();
    config.strategies.enabled = vec!["combinatorial".into()];
    let inferred = Arc::new(ClusterCache::new(chrono::Duration::hours(1)));
    let relation = RelationKind::Implies {
        if_yes: MarketId::new("a"),
        then_yes: MarketId::new("b"),
    };
    inferred.put_relations(vec![Relation::new(
        relation.clone(),
        1.0,
        "Unverified model claim",
    )]);
    let mut markets = MarketRegistry::new();
    markets.add(binary("a"));
    markets.add(binary("b"));
    let markets = Arc::new(markets);
    let cache = BookCache::new();
    for (id, price) in [
        ("a-yes", dec!(0.8)),
        ("a-no", dec!(0.25)),
        ("b-yes", dec!(0.4)),
        ("b-no", dec!(0.65)),
    ] {
        quote(&cache, id, price);
    }
    let market = binary("a");
    let ctx = ConcreteDetectionContext::new(&market, &cache);
    let mut strategies = build_strategy_registry(&config, inferred.clone());
    strategies.set_registry(markets.clone());
    assert!(strategies.detect_all(&ctx).is_empty());
    config.strategies.verified_relations = vec![relation];
    let mut strategies = build_strategy_registry(&config, inferred);
    strategies.set_registry(markets);
    assert_eq!(strategies.detect_all(&ctx).len(), 1);
}
