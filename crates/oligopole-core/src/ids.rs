use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

new_key_type! {
    /// Identifier for a Corporation in the `WorldState` arena.
    pub struct CorpId;
    /// Identifier for a Household Income Cohort in the `WorldState` arena.
    pub struct CohortId;
    /// Identifier for a tradable CLOB instrument (Commodity or Corporate Equity).
    pub struct InstrumentId;
    /// Identifier for a physical good / SKU (`IRON_ORE`, `APPLIANCE`, etc.).
    pub struct SkuId;
}

/// Monotonically increasing deterministic identifier for a CLOB Limit Order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize,
)]
pub struct OrderId(pub u64);

/// Monotonically increasing deterministic identifier for a B2B Supply Contract.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize,
)]
pub struct ContractId(pub u64);

/// Monotonically increasing deterministic identifier for a Corporate Wire (`MSG`) message.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize,
)]
pub struct WireMessageId(pub u64);

/// Monotonically increasing deterministic identifier for a Physical Share-Borrow Short Loan.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize,
)]
pub struct ShortLoanId(pub u64);
