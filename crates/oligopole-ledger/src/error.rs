use crate::account::{AccountId, EntityId};
use crate::money::Money;
use thiserror::Error;

/// Canonical error type for all `oligopole-ledger` operations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LedgerError {
    #[error("arithmetic overflow in monetary calculation")]
    ArithmeticOverflow,

    #[error("division by zero in monetary or pro-rata calculation")]
    DivisionByZero,

    #[error("negative amount {0} is not permitted for this operation")]
    NegativeAmountNotPermitted(Money),

    #[error("empty or zero-sum weights in Hamilton pro-rata allocation")]
    EmptyOrZeroWeights,

    #[error("journal entry has no postings")]
    EmptyJournalEntry,

    #[error("posting amount must be strictly positive, got {0}")]
    NonPositivePostingAmount(Money),

    #[error("unbalanced journal entry: total debits ({debits}) != total credits ({credits})")]
    UnbalancedJournalEntry { debits: Money, credits: Money },

    #[error("unknown account id: {0:?}")]
    UnknownAccount(AccountId),

    #[error("unknown entity id: {0:?}")]
    UnknownEntity(EntityId),

    #[error("insufficient balance or unreserved funds in account {account:?}: available {available}, required {required}")]
    InsufficientFunds {
        account: AccountId,
        available: Money,
        required: Money,
    },

    #[error("insufficient inventory quantity: available {available}, requested {requested}")]
    InsufficientInventoryQty { available: u64, requested: u64 },

    #[error("reservation release {release} exceeds currently reserved amount {reserved}")]
    ExcessiveReservationRelease { reserved: u128, release: u128 },

    #[error("trial balance invariant violated: {reason}")]
    TrialBalanceViolation { reason: String },
}
