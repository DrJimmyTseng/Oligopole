use crate::money::Money;
use serde::{Deserialize, Serialize};
use slotmap::new_key_type;
use std::collections::BTreeMap;

new_key_type! {
    /// Unique identifier for an economic entity (Bank, Corporation, Household Cohort) in the ledger.
    pub struct EntityId;
    /// Unique identifier for a specific ledger account.
    pub struct AccountId;
}

/// Fundamental double-entry accounting equation category:
/// `Assets + Expenses == Liabilities + Equity + Revenues`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AccountCategory {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

/// Side of a double-entry ledger posting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EntrySide {
    Debit,
    Credit,
}

impl AccountCategory {
    /// Returns the normal balance side (`Debit` for Assets & Expenses; `Credit` for Liabilities, Equity & Revenues).
    #[inline]
    pub const fn normal_side(self) -> EntrySide {
        match self {
            Self::Asset | Self::Expense => EntrySide::Debit,
            Self::Liability | Self::Equity | Self::Revenue => EntrySide::Credit,
        }
    }
}

/// Canonical Chart of Accounts codes instantiated for every economic entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StandardAccountCode {
    // --- Assets (Debit normal) ---
    CashAndDeposits,
    CentralBankReserves,
    InventoryRaw,
    InventoryWip,
    InventoryFinished,
    PropertyPlantEquipment,
    LoansReceivable,
    AccountsReceivable,

    // --- Liabilities (Credit normal) ---
    CustomerDepositsLiability,
    CentralBankAdvancesLiability,
    LoansPayable,
    AccountsPayable,
    TaxesPayable,

    // --- Equity (Credit normal) ---
    CommonStock,
    RetainedEarnings,

    // --- Revenues (Credit normal) ---
    SalesRevenue,
    InterestIncome,
    DividendIncome,
    DebtExtinguishmentGain,

    // --- Expenses (Debit normal) ---
    CostOfGoodsSold,
    WageExpense,
    ConsumptionExpense,
    MarketingExpense,
    InterestExpense,
    BadDebtExpense,
    DepreciationExpense,
}

impl StandardAccountCode {
    #[inline]
    pub const fn category(self) -> AccountCategory {
        match self {
            Self::CashAndDeposits
            | Self::CentralBankReserves
            | Self::InventoryRaw
            | Self::InventoryWip
            | Self::InventoryFinished
            | Self::PropertyPlantEquipment
            | Self::LoansReceivable
            | Self::AccountsReceivable => AccountCategory::Asset,

            Self::CustomerDepositsLiability
            | Self::CentralBankAdvancesLiability
            | Self::LoansPayable
            | Self::AccountsPayable
            | Self::TaxesPayable => AccountCategory::Liability,

            Self::CommonStock | Self::RetainedEarnings => AccountCategory::Equity,

            Self::SalesRevenue
            | Self::InterestIncome
            | Self::DividendIncome
            | Self::DebtExtinguishmentGain => AccountCategory::Revenue,

            Self::CostOfGoodsSold
            | Self::WageExpense
            | Self::ConsumptionExpense
            | Self::MarketingExpense
            | Self::InterestExpense
            | Self::BadDebtExpense
            | Self::DepreciationExpense => AccountCategory::Expense,
        }
    }

    /// Whether this account is prohibited from dropping below a zero normal balance.
    #[inline]
    pub const fn requires_non_negative_normal_balance(self) -> bool {
        matches!(
            self,
            Self::CashAndDeposits
                | Self::CentralBankReserves
                | Self::InventoryRaw
                | Self::InventoryWip
                | Self::InventoryFinished
                | Self::PropertyPlantEquipment
                | Self::LoansReceivable
                | Self::AccountsReceivable
                | Self::CustomerDepositsLiability
                | Self::LoansPayable
                | Self::AccountsPayable
        )
    }

    pub const ALL: [Self; 26] = [
        Self::CashAndDeposits,
        Self::CentralBankReserves,
        Self::InventoryRaw,
        Self::InventoryWip,
        Self::InventoryFinished,
        Self::PropertyPlantEquipment,
        Self::LoansReceivable,
        Self::AccountsReceivable,
        Self::CustomerDepositsLiability,
        Self::CentralBankAdvancesLiability,
        Self::LoansPayable,
        Self::AccountsPayable,
        Self::TaxesPayable,
        Self::CommonStock,
        Self::RetainedEarnings,
        Self::SalesRevenue,
        Self::InterestIncome,
        Self::DividendIncome,
        Self::DebtExtinguishmentGain,
        Self::CostOfGoodsSold,
        Self::WageExpense,
        Self::ConsumptionExpense,
        Self::MarketingExpense,
        Self::InterestExpense,
        Self::BadDebtExpense,
        Self::DepreciationExpense,
    ];
}

/// A single general-ledger account belonging to an `EntityId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub entity_id: EntityId,
    pub code: StandardAccountCode,
    pub category: AccountCategory,
    pub total_debits: Money,
    pub total_credits: Money,
}

impl Account {
    pub const fn new(id: AccountId, entity_id: EntityId, code: StandardAccountCode) -> Self {
        Self {
            id,
            entity_id,
            code,
            category: code.category(),
            total_debits: Money::ZERO,
            total_credits: Money::ZERO,
        }
    }

    /// Returns the signed balance in the account's normal direction:
    /// - `Debits - Credits` for `Asset` and `Expense` accounts.
    /// - `Credits - Debits` for `Liability`, `Equity`, and `Revenue` accounts.
    #[inline]
    pub const fn normal_balance(&self) -> Money {
        match self.category.normal_side() {
            EntrySide::Debit => Money::from_cents(self.total_debits.cents() - self.total_credits.cents()),
            EntrySide::Credit => Money::from_cents(self.total_credits.cents() - self.total_debits.cents()),
        }
    }
}

/// Chart of accounts and cash-reservation state for a single economic entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityChartOfAccounts {
    pub entity_id: EntityId,
    pub name: String,
    pub is_bank: bool,
    pub accounts_by_code: BTreeMap<StandardAccountCode, AccountId>,
    /// Cash locked in open CLOB Bid orders (cannot be spent or double-bid until filled or cancelled).
    pub reserved_cash: Money,
}

impl EntityChartOfAccounts {
    pub fn account_id(&self, code: StandardAccountCode) -> Option<AccountId> {
        self.accounts_by_code.get(&code).copied()
    }
}
