//! Review reproductions: assertions describe required behavior, not existing bugs.
use edgelord::adapter::outbound::polymarket::dto::message::PolymarketWsMessage;
use edgelord::adapter::outbound::solver::highs::HiGHSSolver;
use edgelord::application::cluster::{detector::ClusterDetector, service::ClusterDetectionConfig};
use edgelord::application::solver::frank_wolfe::{FrankWolfe, FrankWolfeConfig};
use edgelord::domain::market::Outcome;
use edgelord::domain::{
    book::{Book, PriceLevel},
    cluster::Cluster,
    constraint::{Constraint, VariableBounds},
    id::{ClusterId, MarketId, TokenId},
    market::{Market, MarketRegistry},
};
use edgelord::port::outbound::solver::{IlpProblem, LpProblem, ProjectionResult, ProjectionSolver};
use rust_decimal_macros::dec;
use std::sync::Arc;

#[test]
fn review_single_book_event_is_recognized() {
    let msg: PolymarketWsMessage = serde_json::from_str(r#"{"event_type":"book","asset_id":"123","bids":[],"asks":[{"price":"0.45","size":"100"}]}"#).unwrap();
    assert!(
        !matches!(msg, PolymarketWsMessage::Unknown(_)),
        "Live book object was silently discarded"
    );
}

#[test]
fn review_price_change_event_is_recognized() {
    let msg: PolymarketWsMessage = serde_json::from_str(r#"{"event_type":"price_change","market":"0x123","price_changes":[{"asset_id":"123","price":"0.45","size":"0","side":"SELL","best_bid":"0.44","best_ask":"0.46"}],"timestamp":"1700000000000"}"#).unwrap();
    assert!(
        !matches!(msg, PolymarketWsMessage::Unknown(_)),
        "Removal of best ask was silently discarded"
    );
}

#[test]
fn review_projection_preserves_already_feasible_prices() {
    let theta = vec![dec!(0.4), dec!(0.4)];
    let problem = IlpProblem::all_binary(LpProblem {
        objective: theta.clone(),
        constraints: vec![Constraint::leq(vec![dec!(1), dec!(1)], dec!(1))],
        bounds: vec![VariableBounds::binary(); 2],
    });
    let result = FrankWolfe::new(FrankWolfeConfig::default())
        .project(&theta, &problem, &HiGHSSolver::new())
        .unwrap();
    assert!(
        (result.mu[0] - theta[0]).abs() < dec!(0.001),
        "Feasible prices changed: {result:?}"
    );
}

struct FixedProjection;
impl ProjectionSolver for FixedProjection {
    fn name(&self) -> &'static str {
        "review_fixture"
    }
    fn project(
        &self,
        _: &[rust_decimal::Decimal],
        _: &IlpProblem,
    ) -> edgelord::error::Result<ProjectionResult> {
        Ok(ProjectionResult {
            values: vec![dec!(0.5), dec!(0.5)],
            gap: dec!(0.2),
            iterations: 1,
            converged: true,
        })
    }
}

#[test]
fn review_partial_fill_counts_toward_exposure() {
    use edgelord::application::cache::position::PositionTracker;
    use edgelord::domain::{
        id::PositionId,
        position::{Position, PositionLeg, PositionStatus},
    };
    let mut tracker = PositionTracker::new();
    tracker.add(Position::new(
        PositionId::new(1),
        MarketId::new("a"),
        vec![PositionLeg::new(TokenId::new("yes"), dec!(100), dec!(0.4))],
        dec!(40),
        dec!(100),
        chrono::Utc::now(),
        PositionStatus::PartialFill {
            filled: vec![TokenId::new("yes")],
            missing: vec![TokenId::new("no")],
        },
    ));
    assert_eq!(
        tracker.total_exposure(),
        dec!(40),
        "Filled tokens must count toward exposure even when the hedge failed"
    );
}

#[test]
fn review_cluster_uses_executable_prices() {
    let mut registry = MarketRegistry::new();
    let ids = vec![MarketId::new("a"), MarketId::new("b")];
    for id in &ids {
        registry.add(Market::new(
            id.clone(),
            "Question",
            vec![
                Outcome::new(TokenId::new(id.as_str()), "Yes"),
                Outcome::new(TokenId::new(format!("no-{id}")), "No"),
            ],
            dec!(1),
        ));
    }
    let cluster = Cluster {
        id: ClusterId::new(),
        markets: ids,
        relations: vec![],
        constraints: vec![Constraint::eq(vec![dec!(1), dec!(1)], dec!(1))],
        updated_at: chrono::Utc::now(),
    };
    let lookup = |id: &TokenId| {
        Some(Book::with_levels(
            id.clone(),
            vec![],
            vec![PriceLevel::new(dec!(0.4), dec!(100))],
        ))
    };
    let detector =
        ClusterDetector::new(ClusterDetectionConfig::default(), Arc::new(FixedProjection));
    let result = detector
        .detect(&cluster, &lookup, &registry)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.opportunity.legs()[0].ask_price(),
        dec!(0.4),
        "Projected probabilities replaced available ask prices; volume={}",
        result.opportunity.volume()
    );
}
