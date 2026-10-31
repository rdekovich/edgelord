//! Polymarket US protocol boundary.
//!
//! Wire contracts follow the official Polymarket US TypeScript SDK, revision
//! `abdcaaed4fc975994611aa8959a362bbfcb42d91`. These are deliberately separate
//! from the International CLOB types: US uses signed positions in one instrument.
//! The client is read-only until execution, fees and journal reconciliation are
//! integrated. An order acknowledgement must never be treated as a fill.

pub mod auth;
pub mod client;
pub mod error;
pub mod model;
