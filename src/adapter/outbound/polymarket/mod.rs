//! Polymarket exchange integration.
//!
//! Provides adapters for interacting with the Polymarket prediction market
//! exchange, including REST API clients, WebSocket streaming, order execution,
//! and token approvals.
//!
//! # Modules
//!
//! - [`client`] - REST API client for CLOB and Gamma endpoints
//! - [`stream`] - WebSocket handler for real-time market data
//! - [`executor`] - Order execution and trade management
//! - [`approval`] - ERC-20 token approval for exchange contracts
//! - [`filter`] - Market eligibility filtering
//! - [`scorer`] - Market scoring for subscription prioritization
//! - [`dedup`] - Message deduplication for redundant connections
//! - [`settings`] - Configuration types for the Polymarket adapter

pub mod approval;
pub mod client;
pub mod dedup;
pub mod dto;
pub mod executor;
pub mod filter;
pub mod market;
pub mod scorer;
pub mod settings;
pub mod stream;

pub mod protocol;
