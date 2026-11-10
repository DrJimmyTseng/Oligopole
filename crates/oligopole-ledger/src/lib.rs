//! `oligopole-ledger`: Closed-loop `i128` double-entry accounting, widening 256-bit math,
//! Hamilton Largest Remainder pro-rata allocations, $O(1)$ Weighted Average Cost (WAC) inventory,
//! and endogenous Commercial & Central Bank credit facilities.

pub mod account;
pub mod bank;
pub mod error;
pub mod inventory_wac;
pub mod journal;
pub mod money;
pub mod rounding;
pub mod statement;
pub mod u256_math;

pub use account::{
    Account, AccountCategory, AccountId, EntityChartOfAccounts, EntityId, EntrySide,
    StandardAccountCode,
};
pub use bank::{BankLoanAccount, CommercialBankFacility};
pub use error::LedgerError;
pub use inventory_wac::WacInventoryLot;
pub use journal::{JournalEntry, LedgerEngine, Posting};
pub use money::{Money, SIGN_FLIP_MASK};
pub use rounding::{
    BPS_DENOMINATOR, DAILY_360_BPS_DENOMINATOR, RoundingReserve, allocate_hamilton,
    allocate_units_hamilton,
};
pub use statement::FinancialStatementReport;
pub use u256_math::u256_mul_div_rem;
