//! Application orchestration services.
//!
//! Core event processing and execution workflows that coordinate strategy
//! detection, risk validation, and trade execution. This module implements
//! the main event loop that transforms market data into trading actions.
//!
//! # Architecture
//!
//! The orchestration flow processes events through several stages:
//!
//! 1. **Event Handling**: Market events (book updates, settlements) trigger processing
//! 2. **Strategy Detection**: Applicable strategies scan for arbitrage opportunities
//! 3. **Slippage Check**: Validates prices have not moved adversely since detection
//! 4. **Risk Validation**: Ensures opportunity passes all risk gates
//! 5. **Execution**: Spawns async execution task for approved opportunities
//!
//! # Modules
//!
//! - [`handler`]: Public facade for event and opportunity handling
//! - `context`: Detection context wrappers for strategy interface
//! - `event`: Market event processing logic
//! - `execution`: Async execution spawning and result handling
//! - `opportunity`: Opportunity evaluation and routing
//! - `position`: Position recording helpers
//! - `slippage`: Price slippage calculations

mod context;
mod event;
mod execution;
pub mod handler;
mod opportunity;
pub mod position;
mod slippage;
