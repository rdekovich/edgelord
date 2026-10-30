//! Diagnostic projection types for operator-facing adapters.
//!
//! Defines view models for health checks, connectivity tests, and system
//! diagnostics exposed through operator interfaces.

use async_trait::async_trait;

use crate::error::Result;

/// Summary report for configuration checks.
///
/// Contains the key configuration values for operator review.
#[derive(Debug, Clone)]
pub struct ConfigCheckReport {
    /// Exchange name.
    pub exchange: String,

    /// Environment name (e.g., "mainnet", "testnet").
    pub environment: String,

    /// Blockchain chain ID.
    pub chain_id: u64,

    /// Names of enabled detection strategies.
    pub enabled_strategies: Vec<String>,

    /// Whether dry-run mode is enabled.
    pub dry_run: bool,

    /// Whether a wallet is configured.
    pub wallet_configured: bool,

    /// Whether Telegram notifications are enabled.
    pub telegram_enabled: bool,

    /// Whether a Telegram bot token is present.
    pub telegram_token_present: bool,

    /// Whether a Telegram chat ID is configured.
    pub telegram_chat_present: bool,
}

/// Report on readiness for live trading.
///
/// Contains checks that must pass before enabling real trading.
#[derive(Debug, Clone)]
pub struct LiveReadinessReport {
    /// Known protocol incompatibility preventing live execution.
    pub protocol_blocker: Option<String>,

    /// Exchange name.
    pub exchange: String,

    /// Environment name.
    pub environment: String,

    /// Blockchain chain ID.
    pub chain_id: u64,

    /// Whether dry-run mode is enabled.
    pub dry_run: bool,

    /// Whether the environment is configured for mainnet.
    pub environment_is_mainnet: bool,

    /// Whether the chain ID corresponds to Polygon mainnet.
    pub chain_is_polygon_mainnet: bool,

    /// Whether a wallet is configured.
    pub wallet_configured: bool,
}

impl LiveReadinessReport {
    /// Return `true` when all live-trading requirements are satisfied.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.protocol_blocker.is_none()
            && self.environment_is_mainnet
            && self.chain_is_polygon_mainnet
            && self.wallet_configured
            && !self.dry_run
    }
}

/// Connection endpoints for connectivity verification.
#[derive(Debug, Clone)]
pub struct ConnectionCheckTarget {
    /// Exchange name.
    pub exchange: String,

    /// Environment name.
    pub environment: String,

    /// WebSocket endpoint URL to test.
    pub ws_url: String,

    /// REST API endpoint URL to test.
    pub api_url: String,
}

/// Status of an individual health check.
#[derive(Debug, Clone)]
pub enum HealthCheckStatus {
    /// Check passed successfully.
    Healthy,

    /// Check failed with the specified reason.
    Unhealthy(String),
}

/// Single entry in a health check report.
#[derive(Debug, Clone)]
pub struct HealthCheckEntry {
    /// Name of this health check.
    pub name: String,

    /// Whether failure of this check prevents operation.
    pub critical: bool,

    /// Current status of this check.
    pub status: HealthCheckStatus,
}

/// Aggregated health check report.
#[derive(Debug, Clone, Default)]
pub struct HealthCheckReport {
    /// Individual health check results.
    pub checks: Vec<HealthCheckEntry>,
}

impl HealthCheckReport {
    /// Return `true` when all critical checks are healthy.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.checks
            .iter()
            .filter(|check| check.critical)
            .all(|check| matches!(&check.status, HealthCheckStatus::Healthy))
    }
}

/// Receipt from sending a Telegram test message.
#[derive(Debug, Clone)]
pub struct TelegramTestReceipt {
    /// Masked bot token (for secure display).
    pub masked_token: String,

    /// Chat ID where the message was sent.
    pub chat_id: String,
}

/// Diagnostic use-cases for operator-facing adapters.
///
/// Provides health checks, connectivity verification, and system diagnostics.
///
/// # Thread Safety
///
/// Implementations must be thread-safe (`Send + Sync`).
#[async_trait]
pub trait DiagnosticOperator: Send + Sync {
    /// Build a configuration check summary.
    ///
    /// # Arguments
    ///
    /// * `config_toml` - Raw TOML configuration content.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration cannot be parsed.
    fn check_config(&self, config_toml: &str) -> Result<ConfigCheckReport>;

    /// Build a live-trading readiness report.
    ///
    /// # Arguments
    ///
    /// * `config_toml` - Raw TOML configuration content.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration cannot be parsed.
    fn check_live_readiness(&self, config_toml: &str) -> Result<LiveReadinessReport>;

    /// Resolve exchange connection endpoints for connectivity checks.
    ///
    /// # Arguments
    ///
    /// * `config_toml` - Raw TOML configuration content.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration cannot be parsed.
    fn connection_target(&self, config_toml: &str) -> Result<ConnectionCheckTarget>;

    /// Verify REST API endpoint connectivity.
    ///
    /// # Arguments
    ///
    /// * `api_url` - REST API endpoint URL to test.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unreachable.
    async fn verify_rest_connectivity(&self, api_url: &str) -> Result<()>;

    /// Verify WebSocket endpoint connectivity.
    ///
    /// # Arguments
    ///
    /// * `ws_url` - WebSocket endpoint URL to test.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unreachable.
    async fn verify_websocket_connectivity(&self, ws_url: &str) -> Result<()>;

    /// Run local health checks.
    ///
    /// # Arguments
    ///
    /// * `config_toml` - Raw TOML configuration content.
    ///
    /// # Errors
    ///
    /// Returns an error if health checks cannot be executed.
    fn health_report(&self, config_toml: &str) -> Result<HealthCheckReport>;

    /// Send a test message via Telegram.
    ///
    /// # Arguments
    ///
    /// * `config_toml` - Raw TOML configuration content.
    ///
    /// # Errors
    ///
    /// Returns an error if Telegram is not configured or the message fails.
    async fn send_telegram_test(&self, config_toml: &str) -> Result<TelegramTestReceipt>;
}
