//! Startup market discovery and strategy wiring.

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::application::strategy::registry::StrategyRegistry;
use crate::domain::{id::TokenId, market::MarketRegistry};
use crate::error::Result;
use crate::infrastructure::config::settings::{Config, ExchangeSpecificConfig};
use crate::infrastructure::exchange::factory::ExchangeFactory;
use crate::port::inbound::strategy::StrategyEngine;
use crate::port::outbound::inference::MarketSummary;

/// Startup artifacts needed by the runtime event loop.
pub(crate) struct PreparedMarkets {
    pub registry: Arc<MarketRegistry>,
    pub strategies: Arc<StrategyRegistry>,
    pub token_ids: Vec<TokenId>,
    pub market_summaries: Vec<MarketSummary>,
}

/// Fetch, filter, parse, and wire markets into strategy runtime state.
pub(crate) async fn prepare_markets(
    config: &Config,
    mut strategies: StrategyRegistry,
) -> Result<Option<PreparedMarkets>> {
    info!(
        strategies = ?strategies.strategy_names(),
        "Strategies loaded"
    );

    let max_markets = match &config.exchange_config {
        ExchangeSpecificConfig::Polymarket(pm_config) => pm_config.market_filter.max_markets,
    };

    let market_fetcher = ExchangeFactory::create_market_fetcher(config);
    info!(
        exchange = market_fetcher.exchange_name(),
        max_markets, "Fetching markets"
    );
    let market_infos = market_fetcher.get_markets(max_markets).await?;
    let markets_fetched = market_infos.len();

    if market_infos.is_empty() {
        warn!("No active markets found");
        return Ok(None);
    }

    let market_filter = ExchangeFactory::create_filter(config)?;
    let market_infos = market_filter.filter(&market_infos);
    let markets_filtered = market_infos.len();

    info!(
        markets_fetched,
        markets_filtered,
        rejected = markets_fetched - markets_filtered,
        "Volume/liquidity filter applied"
    );

    if market_infos.is_empty() {
        warn!("No markets passed volume/liquidity filter");
        return Ok(None);
    }

    let market_parser = ExchangeFactory::create_market_parser(config);
    let markets = market_parser.parse_markets(&market_infos);
    let markets_parsed = markets.len();

    let mut registry = MarketRegistry::new();
    for market in markets {
        registry.add(market);
    }

    append_verified_groups(config, &mut registry)?;
    for relation in &config.strategies.verified_relations {
        for id in relation.market_ids() {
            if registry.get_by_market_id(id).is_none() {
                return Err(crate::error::Error::Parse(format!(
                    "Verified relation references undiscovered market {id}"
                )));
            }
        }
    }

    info!(
        markets_fetched,
        markets_parsed,
        yes_no_pairs = registry.len(),
        "Market scan complete"
    );

    if registry.is_empty() {
        warn!("No YES/NO market pairs found");
        return Ok(None);
    }

    for market in registry.markets() {
        debug!(
            market_id = %market.market_id(),
            question = %market.question(),
            "Tracking market"
        );
    }

    let market_summaries: Vec<MarketSummary> = registry
        .markets()
        .iter()
        .map(|m| MarketSummary {
            id: m.market_id().clone(),
            question: m.question().to_string(),
            outcomes: m.outcomes().iter().map(|o| o.name().to_string()).collect(),
        })
        .collect();

    let mut token_ids: Vec<TokenId> = registry.all_token_ids().cloned().collect();
    token_ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    info!(tokens = token_ids.len(), "Subscribing to tokens");

    let registry = Arc::new(registry);
    strategies.set_registry(Arc::clone(&registry));
    let strategies = Arc::new(strategies);

    Ok(Some(PreparedMarkets {
        registry,
        strategies,
        token_ids,
        market_summaries,
    }))
}

fn append_verified_groups(config: &Config, registry: &mut MarketRegistry) -> Result<()> {
    for (index, group) in config.strategies.exhaustive_groups.iter().enumerate() {
        if group.len() > config.strategies.market_rebalancing.max_outcomes {
            return Err(crate::error::Error::Parse(
                "Exhaustive group exceeds max_outcomes".into(),
            ));
        }
        let unique: std::collections::HashSet<_> = group.iter().collect();
        if group.len() < 3 || group.len() != unique.len() {
            return Err(crate::error::Error::Parse(
                "Exhaustive groups require at least three distinct condition IDs".into(),
            ));
        }
        let mut outcomes = Vec::new();
        for id in group {
            let market = registry.get_by_market_id(id).ok_or_else(|| {
                crate::error::Error::Parse(format!(
                    "Incomplete exhaustive group: missing {id}; increase discovery limit"
                ))
            })?;
            if !market.is_binary() {
                return Err(crate::error::Error::Parse(
                    "Group member must be binary".into(),
                ));
            }
            outcomes.push(crate::domain::market::Outcome::new(
                market.outcomes()[0].token_id().clone(),
                market.question(),
            ));
        }
        let tokens: std::collections::HashSet<_> = outcomes.iter().map(|o| o.token_id()).collect();
        if tokens.len() != outcomes.len() {
            return Err(crate::error::Error::Parse(
                "Exhaustive group repeats an outcome token".into(),
            ));
        }
        registry.add(crate::domain::market::Market::new(
            crate::domain::id::MarketId::new(format!("verified-event-{index}")),
            format!("Operator-verified exhaustive group {index}"),
            outcomes,
            rust_decimal::Decimal::ONE,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        id::MarketId,
        market::{Market, Outcome},
    };
    use rust_decimal::Decimal;
    #[test]
    fn complete_groups_preserve_binary_detection_and_missing_members_fail() {
        let mut config = Config::default();
        config.strategies.exhaustive_groups = vec![vec![
            MarketId::new("a"),
            MarketId::new("b"),
            MarketId::new("c"),
        ]];
        let mut registry = MarketRegistry::new();
        for name in ["a", "b"] {
            registry.add(Market::new(
                MarketId::new(name),
                name,
                vec![
                    Outcome::new(TokenId::new(format!("{name}-yes")), "Yes"),
                    Outcome::new(TokenId::new(format!("{name}-no")), "No"),
                ],
                Decimal::ONE,
            ));
        }
        assert!(append_verified_groups(&config, &mut registry).is_err());
        registry.add(Market::new(
            MarketId::new("c"),
            "c",
            vec![
                Outcome::new(TokenId::new("c-yes"), "Yes"),
                Outcome::new(TokenId::new("c-no"), "No"),
            ],
            Decimal::ONE,
        ));
        append_verified_groups(&config, &mut registry).unwrap();
        let token = TokenId::new("a-yes");
        let affected: Vec<_> = registry.get_all_by_token(&token).collect();
        assert_eq!(affected.len(), 2);
        assert!(affected.iter().any(|m| m.outcome_count() == 2));
        assert!(affected.iter().any(|m| m.outcome_count() == 3));
    }
}
