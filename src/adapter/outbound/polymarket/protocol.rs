//! Explicit lifecycle gate for the retired International V1 trading protocol.

use crate::error::{ConfigError, Result};

/// Upstream retirement notice: <https://github.com/Polymarket/rs-clob-client>
pub const LIVE_PROTOCOL_BLOCKER: &str = "International V1 execution is disabled: the upstream SDK is retired and nonfunctional; V2 migration and execution verification are required";

/// Fail before parsing keys, acquiring account locks, or authenticating.
/// Remove this gate only alongside a verified V2 executor migration.
pub fn ensure_live_supported() -> Result<()> {
    Err(ConfigError::Other(LIVE_PROTOCOL_BLOCKER.into()).into())
}
