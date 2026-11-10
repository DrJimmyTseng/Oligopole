use crate::ids::{CorpId, InstrumentId, OrderId, SkuId};
use oligopole_ledger::LedgerError;
use thiserror::Error;

/// Canonical error type for `oligopole-core` simulation and market operations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    #[error("ledger error: {0}")]
    Ledger(#[from] LedgerError),

    #[error("unknown corporation id: {0:?}")]
    UnknownCorporation(CorpId),

    #[error("unknown market instrument id: {0:?}")]
    UnknownInstrument(InstrumentId),

    #[error("unknown SKU id: {0:?}")]
    UnknownSku(SkuId),

    #[error("unknown order id: {0:?}")]
    UnknownOrder(OrderId),

    #[error("invalid order parameters: {reason}")]
    InvalidOrder { reason: String },

    #[error("insufficient unreserved equity shares for {corp_id:?}: available {available}, requested {requested}")]
    InsufficientShares {
        corp_id: CorpId,
        available: u64,
        requested: u64,
    },

    #[error("decimal arithmetic overflow in economic calculation")]
    DecimalOverflow,
}
