//! Edgelord - Prediction market arbitrage framework.
//!
//! # For CLI users
//!
//! Install and run:
//!
//! ```text
//! cargo install edgelord
//! edgelord init
//! edgelord run
//! ```
//!
//! # For developers
//!
//! Fork this repo and extend:
//!
//! - Add strategies: implement `port::inbound::strategy::Strategy`
//! - Add exchanges: implement `port::outbound::exchange::*`
//! - Add notifiers: implement `port::outbound::notifier::Notifier`
//!
//! # Architecture
//!
//! ```text
//! domain/          Pure types, no I/O
//! port/            Trait definitions (extension points)
//! adapter/
//!   inbound/       Driving adapters (CLI)
//!   outbound/      Driven adapters (exchange, storage, notifier, solver, llm)
//! application/     Application services (use cases)
//! infrastructure/  Runtime concerns (caching, config, orchestration)
//! ```

// async_trait adds #[must_use] to already-must-use boxed futures.
#![allow(clippy::double_must_use)]

pub mod adapter;
pub mod application;
pub mod domain;
pub mod error;
pub mod infrastructure;
pub mod port;

#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
