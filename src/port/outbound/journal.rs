//! Durable execution intents and results. Statistics are not a trading ledger.
use crate::{
    domain::{opportunity::Opportunity, trade::TradeResult},
    error::Result,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionRecord {
    /// Bind this ledger to one chain and signer.
    Account { identity: String },
    Intent {
        id: String,
        opportunity: Opportunity,
    },
    Outcome {
        id: String,
        #[serde(default)]
        trade_id: Option<i32>,
        result: TradeResult,
    },
    /// Authoritative resolution payouts, persisted before closing inventory.
    Settlement {
        payouts: Vec<(crate::domain::id::TokenId, rust_decimal::Decimal)>,
    },
}

/// Append must be durable before returning. Failure must prevent submission.
pub trait ExecutionJournal: Send + Sync {
    fn append(&self, record: &ExecutionRecord) -> Result<()>;
    fn records(&self) -> Result<Vec<ExecutionRecord>>;
}
