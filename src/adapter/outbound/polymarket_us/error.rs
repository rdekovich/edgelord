//! Errors contain no credentials, response bodies, or signed headers.

/// Failure at the US protocol boundary.
#[derive(Debug, thiserror::Error)]
pub enum UsError {
    #[error("invalid Polymarket US credentials: {0}")]
    Credentials(&'static str),
    #[error("invalid Polymarket US data: {0}")]
    Protocol(&'static str),
    #[error("Polymarket US request failed with HTTP {0}")]
    Status(u16),
    #[error("Polymarket US transport failed")]
    Transport,
    #[error("Polymarket US response does not match the protocol")]
    Decode,
}
