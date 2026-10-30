//! Outbound (driven) ports implemented by infrastructure adapters.
//!
//! Outbound ports define contracts for infrastructure dependencies that the
//! application core calls out to. Adapters implement these traits to integrate
//! with external systems.
//!
//! # Modules
//!
//! - [`approval`]: Token approval workflows for ERC-20 spending
//! - [`dedup`]: Message deduplication for redundant connections
//! - [`exchange`]: Exchange integration for market data and order execution
//! - [`filter`]: Market filtering and scoring for subscription management
//! - [`inference`]: LLM-based relation inference between markets
//! - [`llm`]: Generic LLM completion interface
//! - [`notifier`]: Event notification dispatch
//! - [`report`]: Read-side reporting and statistics queries
//! - [`solver`]: Linear and integer programming solvers
//! - [`stats`]: Trading statistics recording
//! - [`store`]: Persistence for relations and clusters

pub mod approval;
pub mod dedup;
pub mod exchange;
pub mod filter;
pub mod inference;
pub mod llm;
pub mod notifier;
pub mod report;
pub mod solver;
pub mod stats;
pub mod store;

pub mod journal;
