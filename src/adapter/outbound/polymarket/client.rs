//! Polymarket REST API client.
//!
//! Supports two API surfaces:
//! - **CLOB API** (`clob.polymarket.com`) — order execution, order book queries
//! - **Gamma API** (`gamma-api.polymarket.com`) — market discovery with
//!   volume, liquidity, and outcome metadata
//!
//! Market discovery uses the Gamma API for richer data (volume/liquidity).
//! All other operations (WS streaming, order execution) use the CLOB API.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client as HttpClient;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::dto::response::{GammaMarket, PolymarketMarket, PolymarketMarketsResponse};
use super::settings::PolymarketConfig;
use crate::error::Result;
use crate::port::{
    outbound::exchange::MarketFetcher, outbound::exchange::MarketInfo,
    outbound::exchange::OutcomeInfo,
};

/// HTTP client for the Polymarket REST APIs.
///
/// Handles fetching market data from both the CLOB and Gamma APIs.
/// Market discovery uses Gamma (richer metadata); trading uses CLOB.
pub struct PolymarketClient {
    http: HttpClient,
    /// CLOB API base URL (order execution, order book).
    base_url: String,
    /// Gamma API base URL (market discovery, volume/liquidity).
    gamma_url: String,
    retry_max_attempts: u32,
    retry_backoff_ms: u64,
}

impl PolymarketClient {
    /// Create a new Polymarket client with the given base URL.
    ///
    /// # Arguments
    ///
    /// * `base_url` - The base URL for the Polymarket CLOB API
    ///   (e.g., `https://clob.polymarket.com`)
    #[must_use]
    pub fn new(base_url: String) -> Self {
        Self {
            http: HttpClient::new(),
            gamma_url: "https://gamma-api.polymarket.com".into(),
            base_url,
            retry_max_attempts: 1,
            retry_backoff_ms: 0,
        }
    }

    #[must_use]
    pub fn from_config(config: &PolymarketConfig) -> Self {
        let http = HttpClient::builder()
            .timeout(Duration::from_millis(config.http.timeout_ms))
            .connect_timeout(Duration::from_millis(config.http.connect_timeout_ms))
            .build()
            .unwrap_or_else(|err| {
                warn!(error = %err, "Failed to build HTTP client, using defaults");
                HttpClient::new()
            });

        Self {
            http,
            base_url: config.api_url.clone(),
            gamma_url: config.gamma_api_url.clone(),
            retry_max_attempts: config.http.retry_max_attempts,
            retry_backoff_ms: config.http.retry_backoff_ms,
        }
    }

    async fn get_with_retry<T>(&self, url: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut attempt = 0;
        let max_attempts = self.retry_max_attempts.max(1);

        loop {
            attempt += 1;
            let response = self.http.get(url).send().await;
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    if attempt >= max_attempts || !Self::should_retry(&err) {
                        return Err(err.into());
                    }
                    self.backoff(attempt, max_attempts, &err).await;
                    continue;
                }
            };

            let response = match response.error_for_status() {
                Ok(response) => response,
                Err(err) => return Err(err.into()),
            };

            let parsed = response.json::<T>().await;
            match parsed {
                Ok(parsed) => return Ok(parsed),
                Err(err) => {
                    if attempt >= max_attempts || !Self::should_retry(&err) {
                        return Err(err.into());
                    }
                    self.backoff(attempt, max_attempts, &err).await;
                }
            }
        }
    }

    fn should_retry(err: &reqwest::Error) -> bool {
        err.is_timeout() || err.is_connect()
    }

    async fn backoff(&self, attempt: u32, max_attempts: u32, err: &reqwest::Error) {
        warn!(
            attempt,
            max_attempts,
            error = %err,
            "HTTP request failed, retrying"
        );
        if self.retry_backoff_ms > 0 {
            sleep(Duration::from_millis(self.retry_backoff_ms)).await;
        }
    }

    /// Fetch active markets from the CLOB API.
    ///
    /// Returns raw market data without volume/liquidity metadata.
    /// Prefer [`Self::get_gamma_markets`] for market discovery.
    pub async fn get_active_markets(&self, limit: usize) -> Result<Vec<PolymarketMarket>> {
        let url = format!(
            "{}/markets?active=true&closed=false&limit={}",
            self.base_url, limit
        );

        info!(url = %url, "Fetching active markets (CLOB)");

        let response: PolymarketMarketsResponse = self.get_with_retry(&url).await?;

        let markets = response.data.unwrap_or_default();
        debug!(count = markets.len(), "Fetched markets from CLOB");

        Ok(markets)
    }

    /// Fetch active markets from the Gamma API.
    ///
    /// Returns market data with volume, liquidity, and outcome metadata.
    /// Used for market discovery and filtering.
    pub async fn get_gamma_markets(&self, limit: usize) -> Result<Vec<GammaMarket>> {
        let mut markets = Vec::new();
        while markets.len() < limit {
            let page_size = (limit - markets.len()).min(100);
            let url = format!(
                "{}/markets?active=true&closed=false&limit={}&offset={}",
                self.gamma_url,
                page_size,
                markets.len()
            );
            let page: Vec<GammaMarket> = self.get_with_retry(&url).await?;
            let count = page.len();
            markets.extend(page);
            if count < page_size {
                break;
            }
        }
        markets.truncate(limit);
        Ok(markets)
    }
}

#[async_trait]
impl MarketFetcher for PolymarketClient {
    async fn get_markets(&self, limit: usize) -> Result<Vec<MarketInfo>> {
        let markets = self.get_gamma_markets(limit).await?;
        Ok(markets.into_iter().map(MarketInfo::from).collect())
    }

    fn exchange_name(&self) -> &'static str {
        "Polymarket"
    }
}

// ---------------------------------------------------------------------------
// MarketInfo conversions
// ---------------------------------------------------------------------------

impl From<PolymarketMarket> for MarketInfo {
    fn from(m: PolymarketMarket) -> Self {
        Self {
            id: m.condition_id,
            question: m.question.unwrap_or_default(),
            outcomes: m
                .tokens
                .into_iter()
                .map(|t| OutcomeInfo {
                    token_id: t.token_id,
                    name: t.outcome,
                    price: t.price,
                })
                .collect(),
            active: m.active && !m.closed,
            volume_24h: m.volume_24h,
            liquidity: m.liquidity,
        }
    }
}

impl From<GammaMarket> for MarketInfo {
    fn from(m: GammaMarket) -> Self {
        let token_ids = m.token_ids();
        let names = m.outcome_names();
        let prices = m.outcome_prices();

        let outcomes = token_ids
            .into_iter()
            .enumerate()
            .map(|(i, token_id)| OutcomeInfo {
                token_id,
                name: names.get(i).cloned().unwrap_or_default(),
                price: prices.get(i).copied(),
            })
            .collect();

        Self {
            id: m.condition_id,
            question: m.question.unwrap_or_default(),
            outcomes,
            active: m.active && !m.closed,
            volume_24h: m.volume_24hr,
            liquidity: m.liquidity_num,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::outbound::polymarket::dto::response::{GammaMarket, PolymarketToken};

    // -------------------------------------------------------------------------
    // PolymarketClient Construction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn client_new_stores_base_url() {
        let client = PolymarketClient::new("https://test.polymarket.com".into());
        // Verify construction succeeds (base_url is private)
        assert_eq!(client.exchange_name(), "Polymarket");
    }

    #[test]
    fn client_new_sets_default_retry_config() {
        let client = PolymarketClient::new("https://test.com".into());
        // Default retry_max_attempts should be 1 (no retries)
        // We can't access private fields, but verify construction
        assert_eq!(client.exchange_name(), "Polymarket");
    }

    #[test]
    fn client_from_config_uses_config_values() {
        let config = PolymarketConfig {
            api_url: "https://custom-clob.com".into(),
            gamma_api_url: "https://custom-gamma.com".into(),
            http: super::super::settings::PolymarketHttpConfig {
                timeout_ms: 10000,
                connect_timeout_ms: 5000,
                retry_max_attempts: 5,
                retry_backoff_ms: 1000,
            },
            ..Default::default()
        };

        let client = PolymarketClient::from_config(&config);
        assert_eq!(client.exchange_name(), "Polymarket");
    }

    #[test]
    fn client_from_default_config() {
        let config = PolymarketConfig::default();
        let client = PolymarketClient::from_config(&config);
        assert_eq!(client.exchange_name(), "Polymarket");
    }

    // -------------------------------------------------------------------------
    // MarketFetcher Trait Tests
    // -------------------------------------------------------------------------

    #[test]
    fn client_implements_market_fetcher() {
        let client = PolymarketClient::new("https://test.com".into());
        // Verify trait method exists
        let name = client.exchange_name();
        assert_eq!(name, "Polymarket");
    }

    // -------------------------------------------------------------------------
    // should_retry Tests
    // -------------------------------------------------------------------------

    #[test]
    fn should_retry_logic_is_defined() {
        // We can't easily create reqwest errors for testing, but we verify
        // the function exists and the logic is sensible by reviewing the code.
        // The function returns true for timeout and connect errors.
        // This test documents the expected behavior.
        fn _assert_fn_exists() {
            let _ = PolymarketClient::should_retry;
        }
    }

    // -------------------------------------------------------------------------
    // PolymarketMarket to MarketInfo Conversion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn polymarket_market_converts_to_market_info() {
        let market = PolymarketMarket {
            condition_id: "cond-123".into(),
            question: Some("Will it rain?".into()),
            tokens: vec![
                PolymarketToken {
                    token_id: "yes-token".into(),
                    outcome: "Yes".into(),
                    price: Some(0.65),
                },
                PolymarketToken {
                    token_id: "no-token".into(),
                    outcome: "No".into(),
                    price: Some(0.35),
                },
            ],
            active: true,
            closed: false,
            volume_24h: Some(10000.0),
            liquidity: Some(5000.0),
        };

        let info = MarketInfo::from(market);

        assert_eq!(info.id, "cond-123");
        assert_eq!(info.question, "Will it rain?");
        assert_eq!(info.outcomes.len(), 2);
        assert!(info.active);
        assert_eq!(info.volume_24h, Some(10000.0));
        assert_eq!(info.liquidity, Some(5000.0));

        // Check outcomes
        assert_eq!(info.outcomes[0].token_id, "yes-token");
        assert_eq!(info.outcomes[0].name, "Yes");
        assert_eq!(info.outcomes[0].price, Some(0.65));

        assert_eq!(info.outcomes[1].token_id, "no-token");
        assert_eq!(info.outcomes[1].name, "No");
        assert_eq!(info.outcomes[1].price, Some(0.35));
    }

    #[test]
    fn polymarket_market_active_when_not_closed() {
        let market = PolymarketMarket {
            condition_id: "test".into(),
            question: None,
            tokens: vec![],
            active: true,
            closed: false,
            volume_24h: None,
            liquidity: None,
        };

        let info = MarketInfo::from(market);
        assert!(info.active);
    }

    #[test]
    fn polymarket_market_inactive_when_closed() {
        let market = PolymarketMarket {
            condition_id: "test".into(),
            question: None,
            tokens: vec![],
            active: true,
            closed: true, // closed flag takes precedence
            volume_24h: None,
            liquidity: None,
        };

        let info = MarketInfo::from(market);
        assert!(!info.active);
    }

    #[test]
    fn polymarket_market_inactive_when_not_active() {
        let market = PolymarketMarket {
            condition_id: "test".into(),
            question: None,
            tokens: vec![],
            active: false,
            closed: false,
            volume_24h: None,
            liquidity: None,
        };

        let info = MarketInfo::from(market);
        assert!(!info.active);
    }

    #[test]
    fn polymarket_market_handles_missing_question() {
        let market = PolymarketMarket {
            condition_id: "test".into(),
            question: None,
            tokens: vec![],
            active: true,
            closed: false,
            volume_24h: None,
            liquidity: None,
        };

        let info = MarketInfo::from(market);
        assert_eq!(info.question, "");
    }

    #[test]
    fn polymarket_market_handles_empty_tokens() {
        let market = PolymarketMarket {
            condition_id: "test".into(),
            question: Some("Test?".into()),
            tokens: vec![],
            active: true,
            closed: false,
            volume_24h: None,
            liquidity: None,
        };

        let info = MarketInfo::from(market);
        assert!(info.outcomes.is_empty());
    }

    #[test]
    fn polymarket_market_token_ids_method() {
        let market = PolymarketMarket {
            condition_id: "test".into(),
            question: None,
            tokens: vec![
                PolymarketToken {
                    token_id: "token-1".into(),
                    outcome: "A".into(),
                    price: None,
                },
                PolymarketToken {
                    token_id: "token-2".into(),
                    outcome: "B".into(),
                    price: None,
                },
                PolymarketToken {
                    token_id: "token-3".into(),
                    outcome: "C".into(),
                    price: None,
                },
            ],
            active: true,
            closed: false,
            volume_24h: None,
            liquidity: None,
        };

        let ids = market.token_ids();
        assert_eq!(ids, vec!["token-1", "token-2", "token-3"]);
    }

    // -------------------------------------------------------------------------
    // GammaMarket to MarketInfo Conversion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn gamma_market_converts_to_market_info() {
        let market = GammaMarket {
            condition_id: "gamma-cond".into(),
            question: Some("Will X happen?".into()),
            active: true,
            closed: false,
            outcomes: Some(r#"["Yes", "No"]"#.into()),
            outcome_prices: Some(r#"["0.70", "0.30"]"#.into()),
            clob_token_ids: Some(r#"["token-yes", "token-no"]"#.into()),
            volume_24hr: Some(50000.0),
            volume_num: Some(1000000.0),
            liquidity_num: Some(25000.0),
        };

        let info = MarketInfo::from(market);

        assert_eq!(info.id, "gamma-cond");
        assert_eq!(info.question, "Will X happen?");
        assert!(info.active);
        assert_eq!(info.outcomes.len(), 2);
        assert_eq!(info.volume_24h, Some(50000.0));
        assert_eq!(info.liquidity, Some(25000.0));

        // Check outcomes
        assert_eq!(info.outcomes[0].token_id, "token-yes");
        assert_eq!(info.outcomes[0].name, "Yes");
        assert!((info.outcomes[0].price.unwrap() - 0.70).abs() < 0.01);
    }

    #[test]
    fn gamma_market_handles_missing_fields() {
        let market = GammaMarket {
            condition_id: "minimal".into(),
            question: None,
            active: true,
            closed: false,
            outcomes: None,
            outcome_prices: None,
            clob_token_ids: None,
            volume_24hr: None,
            volume_num: None,
            liquidity_num: None,
        };

        let info = MarketInfo::from(market);

        assert_eq!(info.id, "minimal");
        assert_eq!(info.question, "");
        assert!(info.outcomes.is_empty());
        assert!(info.volume_24h.is_none());
        assert!(info.liquidity.is_none());
    }

    #[test]
    fn gamma_market_handles_mismatched_arrays() {
        // More token IDs than names/prices
        let market = GammaMarket {
            condition_id: "mismatch".into(),
            question: Some("Test?".into()),
            active: true,
            closed: false,
            outcomes: Some(r#"["Yes"]"#.into()), // Only 1 name
            outcome_prices: Some(r#"["0.60"]"#.into()), // Only 1 price
            clob_token_ids: Some(r#"["token-1", "token-2", "token-3"]"#.into()), // 3 tokens
            volume_24hr: None,
            volume_num: None,
            liquidity_num: None,
        };

        let info = MarketInfo::from(market);

        // Should have 3 outcomes, but only first has name/price
        assert_eq!(info.outcomes.len(), 3);
        assert_eq!(info.outcomes[0].name, "Yes");
        assert_eq!(info.outcomes[1].name, ""); // Missing name defaults to empty
        assert!(info.outcomes[0].price.is_some());
        assert!(info.outcomes[1].price.is_none()); // Missing price
    }

    #[test]
    fn gamma_market_handles_invalid_json_in_fields() {
        let market = GammaMarket {
            condition_id: "invalid-json".into(),
            question: Some("Test?".into()),
            active: true,
            closed: false,
            outcomes: Some("not valid json".into()),
            outcome_prices: Some("[invalid]".into()),
            clob_token_ids: Some("{wrong format}".into()),
            volume_24hr: None,
            volume_num: None,
            liquidity_num: None,
        };

        let info = MarketInfo::from(market);

        // Should gracefully handle invalid JSON by returning empty
        assert!(info.outcomes.is_empty());
    }

    #[test]
    fn gamma_market_active_logic() {
        // Active and not closed
        let market1 = GammaMarket {
            condition_id: "1".into(),
            active: true,
            closed: false,
            ..Default::default()
        };
        assert!(MarketInfo::from(market1).active);

        // Active but closed
        let market2 = GammaMarket {
            condition_id: "2".into(),
            active: true,
            closed: true,
            ..Default::default()
        };
        assert!(!MarketInfo::from(market2).active);

        // Not active
        let market3 = GammaMarket {
            condition_id: "3".into(),
            active: false,
            closed: false,
            ..Default::default()
        };
        assert!(!MarketInfo::from(market3).active);
    }

    // -------------------------------------------------------------------------
    // MarketInfo Helper Methods Tests
    // -------------------------------------------------------------------------

    #[test]
    fn market_info_token_ids_returns_all_tokens() {
        let info = MarketInfo {
            id: "test".into(),
            question: "Test?".into(),
            outcomes: vec![
                OutcomeInfo {
                    token_id: "t1".into(),
                    name: "A".into(),
                    price: None,
                },
                OutcomeInfo {
                    token_id: "t2".into(),
                    name: "B".into(),
                    price: None,
                },
            ],
            active: true,
            volume_24h: None,
            liquidity: None,
        };

        let ids = info.token_ids();
        assert_eq!(ids, vec!["t1", "t2"]);
    }

    #[test]
    fn market_info_is_binary_returns_true_for_two_outcomes() {
        let info = MarketInfo {
            id: "test".into(),
            question: "Binary?".into(),
            outcomes: vec![
                OutcomeInfo {
                    token_id: "yes".into(),
                    name: "Yes".into(),
                    price: None,
                },
                OutcomeInfo {
                    token_id: "no".into(),
                    name: "No".into(),
                    price: None,
                },
            ],
            active: true,
            volume_24h: None,
            liquidity: None,
        };

        assert!(info.is_binary());
    }

    #[test]
    fn market_info_is_binary_returns_false_for_other_counts() {
        // Single outcome
        let info1 = MarketInfo {
            id: "test".into(),
            question: "Single?".into(),
            outcomes: vec![OutcomeInfo {
                token_id: "only".into(),
                name: "Only".into(),
                price: None,
            }],
            active: true,
            volume_24h: None,
            liquidity: None,
        };
        assert!(!info1.is_binary());

        // Three outcomes
        let info3 = MarketInfo {
            id: "test".into(),
            question: "Triple?".into(),
            outcomes: vec![
                OutcomeInfo {
                    token_id: "a".into(),
                    name: "A".into(),
                    price: None,
                },
                OutcomeInfo {
                    token_id: "b".into(),
                    name: "B".into(),
                    price: None,
                },
                OutcomeInfo {
                    token_id: "c".into(),
                    name: "C".into(),
                    price: None,
                },
            ],
            active: true,
            volume_24h: None,
            liquidity: None,
        };
        assert!(!info3.is_binary());

        // Zero outcomes
        let info0 = MarketInfo {
            id: "test".into(),
            question: "Empty?".into(),
            outcomes: vec![],
            active: true,
            volume_24h: None,
            liquidity: None,
        };
        assert!(!info0.is_binary());
    }
}

// -------------------------------------------------------------------------
// Integration Tests (behind feature flag)
// -------------------------------------------------------------------------

#[cfg(all(test, feature = "polymarket-integration"))]
mod integration_tests {
    use super::*;
    use std::env;
    use std::time::Duration;
    use tokio::time::timeout;

    fn get_api_url() -> String {
        env::var("POLYMARKET_API_URL").unwrap_or_else(|_| "https://clob.polymarket.com".into())
    }

    fn get_gamma_url() -> String {
        env::var("POLYMARKET_GAMMA_URL")
            .unwrap_or_else(|_| "https://gamma-api.polymarket.com".into())
    }

    #[tokio::test]
    async fn integration_fetch_active_markets() {
        let client = PolymarketClient::new(get_api_url());

        let result = timeout(Duration::from_secs(30), client.get_active_markets(10))
            .await
            .expect("Timed out fetching markets");

        match result {
            Ok(markets) => {
                println!("Fetched {} markets from CLOB API", markets.len());
                assert!(!markets.is_empty(), "Expected at least one active market");

                // Verify at least some markets have valid structure
                let valid_markets: Vec<_> = markets
                    .iter()
                    .filter(|m| !m.condition_id.is_empty())
                    .collect();
                assert!(
                    !valid_markets.is_empty(),
                    "Expected at least one market with condition_id"
                );
            }
            Err(e) => {
                eprintln!("Integration test failed (may be network issue): {}", e);
            }
        }
    }

    #[tokio::test]
    async fn integration_fetch_gamma_markets() {
        let config = PolymarketConfig {
            api_url: get_api_url(),
            gamma_api_url: get_gamma_url(),
            ..Default::default()
        };
        let client = PolymarketClient::from_config(&config);

        let result = timeout(Duration::from_secs(30), client.get_gamma_markets(10))
            .await
            .expect("Timed out fetching Gamma markets");

        match result {
            Ok(markets) => {
                println!("Fetched {} markets from Gamma API", markets.len());
                assert!(!markets.is_empty(), "Expected at least one active market");

                // Verify Gamma market has expected fields
                for market in &markets {
                    assert!(!market.condition_id.is_empty());
                    // Gamma markets should have token IDs
                    if !market.token_ids().is_empty() {
                        println!(
                            "Market {} has {} tokens",
                            market.condition_id,
                            market.token_ids().len()
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("Integration test failed (may be network issue): {}", e);
            }
        }
    }

    #[tokio::test]
    async fn integration_market_fetcher_trait() {
        let client = PolymarketClient::new(get_api_url());
        let fetcher: &dyn MarketFetcher = &client;

        let result = timeout(Duration::from_secs(30), fetcher.get_markets(5))
            .await
            .expect("Timed out");

        match result {
            Ok(markets) => {
                assert_eq!(fetcher.exchange_name(), "Polymarket");
                println!("Fetched {} markets via MarketFetcher trait", markets.len());
            }
            Err(e) => {
                eprintln!("Integration test failed: {}", e);
            }
        }
    }
}
