use crate::ids::{CorpId, InstrumentId, OrderId, ShortLoanId, SkuId, WireMessageId};
use oligopole_ledger::{LedgerError, Money};
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

    #[error("unknown short loan id: {0:?}")]
    UnknownShortLoan(ShortLoanId),

    #[error("unknown wire message id: {0:?}")]
    UnknownWireMessage(WireMessageId),

    #[error("invalid order parameters: {reason}")]
    InvalidOrder { reason: String },

    #[error("LULD circuit-breaker band violation: price {price} outside [{lower_band}, {upper_band}]")]
    LuldBandViolation {
        price: Money,
        lower_band: Money,
        upper_band: Money,
    },

    #[error("IPO GAAP listing gate violation: {reason}")]
    IpoGateViolation { reason: String },

    #[error("short-locate or margin violation: {reason}")]
    ShortMarginViolation { reason: String },

    #[error("insufficient unreserved equity shares for {corp_id:?}: available {available}, requested {requested}")]
    InsufficientShares {
        corp_id: CorpId,
        available: u64,
        requested: u64,
    },

    #[error("decimal arithmetic overflow in economic calculation")]
    DecimalOverflow,
}
