//! Bounded, read-only REST access. No implicit retries or redirects.

use std::{collections::BTreeMap, time::Duration};

use reqwest::{Client, Url};
use serde::de::DeserializeOwned;

use super::{auth::Credentials, error::UsError, model::*};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Client for public discovery and explicit authenticated account inspection.
/// Separate origins prevent accidentally sending credentials to the gateway.
pub struct UsClient {
    http: Client,
    public_origin: Url,
    private_origin: Url,
    credentials: Option<Credentials>,
}

impl UsClient {
    /// Production US endpoints. Credentials are optional for public market data.
    pub fn new(credentials: Option<Credentials>) -> Result<Self, UsError> {
        Self::with_origins(
            Url::parse("https://gateway.polymarket.us").expect("constant URL"),
            Url::parse("https://api.polymarket.us").expect("constant URL"),
            credentials,
        )
    }

    // Custom origins are intentionally private: signed requests only go to the
    // official host in production. Unit tests use loopback servers.
    fn with_origins(
        public_origin: Url,
        private_origin: Url,
        credentials: Option<Credentials>,
    ) -> Result<Self, UsError> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| UsError::Transport)?;
        Ok(Self {
            http,
            public_origin,
            private_origin,
            credentials,
        })
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        authenticated: bool,
    ) -> Result<T, UsError> {
        let origin = if authenticated {
            &self.private_origin
        } else {
            &self.public_origin
        };
        let url = origin
            .join(path)
            .map_err(|_| UsError::Protocol("invalid request path"))?;
        let mut request = self.http.get(url).query(query);
        if authenticated {
            let credentials = self.credentials.as_ref().ok_or(UsError::Credentials(
                "authenticated access requires US API keys",
            ))?;
            request = request.headers(credentials.headers(
                "GET",
                path,
                chrono::Utc::now().timestamp_millis(),
            )?);
        }
        let mut response = request.send().await.map_err(|_| UsError::Transport)?;
        if !response.status().is_success() {
            return Err(UsError::Status(response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| UsError::Transport)? {
            if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
                return Err(UsError::Protocol("response exceeds size limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| UsError::Decode)
    }

    pub async fn markets(&self, limit: usize) -> Result<Vec<Market>, UsError> {
        if !(1..=100).contains(&limit) {
            return Err(UsError::Protocol("market limit must be 1..=100"));
        }
        let response: Markets = self
            .get(
                "/v1/markets",
                &[
                    ("limit", limit.to_string()),
                    ("active", "true".into()),
                    ("closed", "false".into()),
                ],
                false,
            )
            .await?;
        for market in &response.markets {
            validate_slug(&market.slug)?;
        }
        Ok(response.markets)
    }

    pub async fn book(&self, slug: &str) -> Result<MarketBook, UsError> {
        validate_slug(slug)?;
        let response: BookResponse = self
            .get(&format!("/v1/markets/{slug}/book"), &[], false)
            .await?;
        if response.market_data.market_slug != slug {
            return Err(UsError::Protocol("book does not match requested market"));
        }
        response.market_data.validate()?;
        Ok(response.market_data)
    }

    pub async fn balances(&self) -> Result<Vec<Balance>, UsError> {
        let response: Balances = self.get("/v1/account/balances", &[], true).await?;
        Ok(response.balances)
    }

    pub async fn open_orders(&self) -> Result<Vec<Order>, UsError> {
        let response: Orders = self.get("/v1/orders/open", &[], true).await?;
        for order in &response.orders {
            order.validate()?;
        }
        Ok(response.orders)
    }

    /// Read every position page. Repeated cursors, duplicate instruments and
    /// incomplete pagination are errors, never a plausible empty inventory.
    pub async fn positions(&self) -> Result<BTreeMap<String, Position>, UsError> {
        let mut result = BTreeMap::new();
        let mut seen = std::collections::HashSet::new();
        let mut query = vec![("limit", "100".into())];
        for _ in 0..1000 {
            let page: Positions = self.get("/v1/portfolio/positions", &query, true).await?;
            for (instrument, position) in page.positions {
                if instrument.is_empty() || result.insert(instrument, position).is_some() {
                    return Err(UsError::Protocol("duplicate or empty position instrument"));
                }
            }
            let next = page.next_cursor.filter(|s| !s.is_empty());
            if page.eof == Some(true) {
                if next.is_some() {
                    return Err(UsError::Protocol("conflicting pagination metadata"));
                }
                return Ok(result);
            }
            match next {
                Some(cursor) if seen.insert(cursor.clone()) => {
                    query = vec![("limit", "100".into()), ("cursor", cursor)]
                }
                None if page.eof != Some(false) => return Ok(result),
                _ => return Err(UsError::Protocol("incomplete or repeated position cursor")),
            }
        }
        Err(UsError::Protocol("position pagination exceeds page limit"))
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
