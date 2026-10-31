//! Diagnostic operator implementation.

use async_trait::async_trait;

use crate::error::{ConfigError, Error, Result};
use crate::infrastructure::config;
use crate::infrastructure::orchestration::orchestrator::{self, HealthStatus};
use crate::port::inbound::operator::diagnostic::{
    ConfigCheckReport, ConnectionCheckTarget, DiagnosticOperator, HealthCheckEntry,
    HealthCheckReport, HealthCheckStatus, LiveReadinessReport, TelegramTestReceipt,
};

use super::{entry::Operator, shared};

#[async_trait]
impl DiagnosticOperator for Operator {
    fn check_config(&self, config_toml: &str) -> Result<ConfigCheckReport> {
        let config = config::settings::Config::parse_toml(config_toml)?;
        let network = config.network();

        Ok(ConfigCheckReport {
            exchange: format!("{:?}", config.exchange),
            environment: network.environment.to_string(),
            chain_id: network.chain_id,
            enabled_strategies: config.strategies.enabled,
            dry_run: config.dry_run,
            wallet_configured: config.wallet.private_key.is_some(),
            telegram_enabled: config.telegram.enabled,
            telegram_token_present: std::env::var("TELEGRAM_BOT_TOKEN").is_ok(),
            telegram_chat_present: std::env::var("TELEGRAM_CHAT_ID").is_ok(),
        })
    }

    fn check_live_readiness(&self, config_toml: &str) -> Result<LiveReadinessReport> {
        let config = config::settings::Config::parse_toml(config_toml)?;
        let network = config.network();

        Ok(LiveReadinessReport {
            protocol_blocker: Some(
                crate::adapter::outbound::polymarket::protocol::LIVE_PROTOCOL_BLOCKER.into(),
            ),
            exchange: format!("{:?}", config.exchange),
            environment: network.environment.to_string(),
            chain_id: network.chain_id,
            dry_run: config.dry_run,
            environment_is_mainnet: network.is_environment_mainnet(),
            chain_is_polygon_mainnet: network.chain_id == 137,
            wallet_configured: config.wallet.private_key.is_some(),
        })
    }

    fn connection_target(&self, config_toml: &str) -> Result<ConnectionCheckTarget> {
        let config = config::settings::Config::parse_toml(config_toml)?;
        let network = config.network();

        Ok(ConnectionCheckTarget {
            exchange: format!("{:?}", config.exchange),
            environment: network.environment.to_string(),
            ws_url: network.ws_url,
            api_url: network.api_url,
        })
    }

    async fn verify_rest_connectivity(&self, api_url: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{api_url}/markets"))
            .send()
            .await
            .map_err(|error| Error::Connection(error.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Connection(format!(
                "REST API returned non-success status: {}",
                response.status()
            )))
        }
    }

    async fn verify_websocket_connectivity(&self, ws_url: &str) -> Result<()> {
        tokio_tungstenite::connect_async(ws_url)
            .await
            .map(|_| ())
            .map_err(|error| Error::Connection(error.to_string()))
    }

    fn health_report(&self, config_toml: &str) -> Result<HealthCheckReport> {
        let config = config::settings::Config::parse_toml(config_toml)?;
        let report = orchestrator::health_check(&config);
        let checks = report
            .checks()
            .iter()
            .map(|check| HealthCheckEntry {
                name: check.name().to_string(),
                critical: check.critical(),
                status: match check.status() {
                    HealthStatus::Healthy => HealthCheckStatus::Healthy,
                    HealthStatus::Unhealthy(reason) => HealthCheckStatus::Unhealthy(reason.clone()),
                },
            })
            .collect();

        Ok(HealthCheckReport { checks })
    }

    async fn send_telegram_test(&self, config_toml: &str) -> Result<TelegramTestReceipt> {
        let config = config::settings::Config::parse_toml(config_toml)?;

        let token = std::env::var("TELEGRAM_BOT_TOKEN").map_err(|_| ConfigError::MissingField {
            field: "TELEGRAM_BOT_TOKEN environment variable",
        })?;

        let chat_id = std::env::var("TELEGRAM_CHAT_ID").map_err(|_| ConfigError::MissingField {
            field: "TELEGRAM_CHAT_ID environment variable",
        })?;

        let message = format!(
            "🧪 *Edgelord Test Message*\n\n\
            Configuration validated\\!\n\n\
            Environment: `{}`\n\
            Strategies: `{:?}`\n\
            Dry\\-run: `{}`",
            config.network().environment,
            config.strategies.enabled,
            config.dry_run
        );

        let client = reqwest::Client::new();
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");

        let response = client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": message,
                "parse_mode": "MarkdownV2",
            }))
            .send()
            .await
            .map_err(|error| Error::Connection(error.to_string()))?;

        if response.status().is_success() {
            Ok(TelegramTestReceipt {
                masked_token: shared::mask_token(&token),
                chat_id,
            })
        } else {
            let status = response.status();
            let body: String = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(Error::Connection(format!(
                "failed to send telegram message: {status} {body}"
            )))
        }
    }
}
