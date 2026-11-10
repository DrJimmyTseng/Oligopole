use crate::account::{
    Account, AccountCategory, AccountId, EntityChartOfAccounts, EntityId, EntrySide,
    StandardAccountCode,
};
use crate::error::LedgerError;
use crate::money::Money;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::BTreeMap;

/// A single leg (Debit or Credit) of a balanced double-entry `JournalEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posting {
    pub account_id: AccountId,
    pub side: EntrySide,
    pub amount: Money,
}

/// A balanced multi-legged double-entry journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub tick: u64,
    pub description: String,
    pub postings: Vec<Posting>,
}

impl JournalEntry {
    pub fn new(tick: u64, description: impl Into<String>) -> Self {
        Self {
            tick,
            description: description.into(),
            postings: Vec::with_capacity(4),
        }
    }

    #[must_use]
    pub fn debit(mut self, account_id: AccountId, amount: Money) -> Self {
        self.postings.push(Posting {
            account_id,
            side: EntrySide::Debit,
            amount,
        });
        self
    }

    #[must_use]
    pub fn credit(mut self, account_id: AccountId, amount: Money) -> Self {
        self.postings.push(Posting {
            account_id,
            side: EntrySide::Credit,
            amount,
        });
        self
    }

    /// Validates that the entry is non-empty, all posting amounts are strictly positive,
    /// and `sum(Debits) == sum(Credits)`.
    pub fn validate(&self) -> Result<Money, LedgerError> {
        if self.postings.is_empty() {
            return Err(LedgerError::EmptyJournalEntry);
        }
        let mut debits = Money::ZERO;
        let mut credits = Money::ZERO;
        for posting in &self.postings {
            if !posting.amount.is_positive() {
                return Err(LedgerError::NonPositivePostingAmount(posting.amount));
            }
            match posting.side {
                EntrySide::Debit => {
                    debits = debits.checked_add(posting.amount)?;
                }
                EntrySide::Credit => {
                    credits = credits.checked_add(posting.amount)?;
                }
            }
        }
        if debits != credits {
            return Err(LedgerError::UnbalancedJournalEntry { debits, credits });
        }
        Ok(debits)
    }
}

/// Closed-loop double-entry accounting engine managing all entities and general ledger accounts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LedgerEngine {
    entities: SlotMap<EntityId, EntityChartOfAccounts>,
    accounts: SlotMap<AccountId, Account>,
    posted_entry_count: u64,
}

impl LedgerEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new economic entity and initializes its full standard Chart of Accounts.
    pub fn register_entity(&mut self, name: impl Into<String>, is_bank: bool) -> EntityId {
        let entity_name = name.into();
        let entity_id = self.entities.insert_with_key(|eid| EntityChartOfAccounts {
            entity_id: eid,
            name: entity_name,
            is_bank,
            accounts_by_code: BTreeMap::new(),
            reserved_cash: Money::ZERO,
        });

        let mut map = BTreeMap::new();
        for code in StandardAccountCode::ALL {
            let acc_id = self
                .accounts
                .insert_with_key(|aid| Account::new(aid, entity_id, code));
            map.insert(code, acc_id);
        }

        if let Some(entity) = self.entities.get_mut(entity_id) {
            entity.accounts_by_code = map;
        }
        entity_id
    }

    pub fn entity(&self, entity_id: EntityId) -> Result<&EntityChartOfAccounts, LedgerError> {
        self.entities
            .get(entity_id)
            .ok_or(LedgerError::UnknownEntity(entity_id))
    }

    pub fn account(&self, account_id: AccountId) -> Result<&Account, LedgerError> {
        self.accounts
            .get(account_id)
            .ok_or(LedgerError::UnknownAccount(account_id))
    }

    pub fn entity_account_id(
        &self,
        entity_id: EntityId,
        code: StandardAccountCode,
    ) -> Result<AccountId, LedgerError> {
        let entity = self.entity(entity_id)?;
        entity
            .account_id(code)
            .ok_or(LedgerError::UnknownEntity(entity_id))
    }

    pub fn entity_balance(
        &self,
        entity_id: EntityId,
        code: StandardAccountCode,
    ) -> Result<Money, LedgerError> {
        let acc_id = self.entity_account_id(entity_id, code)?;
        Ok(self.account(acc_id)?.normal_balance())
    }

    /// Returns total cash (`CashAndDeposits`) owned by `entity_id`.
    pub fn cash_balance(&self, entity_id: EntityId) -> Result<Money, LedgerError> {
        self.entity_balance(entity_id, StandardAccountCode::CashAndDeposits)
    }

    /// Returns cash currently locked in open CLOB Bid orders for `entity_id`.
    pub fn reserved_cash(&self, entity_id: EntityId) -> Result<Money, LedgerError> {
        Ok(self.entity(entity_id)?.reserved_cash)
    }

    /// Returns unreserved spendable cash (`CashAndDeposits - reserved_cash`) for `entity_id`.
    pub fn available_cash(&self, entity_id: EntityId) -> Result<Money, LedgerError> {
        let cash = self.cash_balance(entity_id)?;
        let reserved = self.reserved_cash(entity_id)?;
        cash.checked_sub(reserved)
    }

    /// Locks `amount` of unreserved cash into CLOB Bid escrow without mutating ledger accounts.
    pub fn reserve_cash(&mut self, entity_id: EntityId, amount: Money) -> Result<(), LedgerError> {
        if amount.is_negative() {
            return Err(LedgerError::NegativeAmountNotPermitted(amount));
        }
        if amount.is_zero() {
            return Ok(());
        }
        let avail = self.available_cash(entity_id)?;
        let cash_acc = self.entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
        if amount > avail {
            return Err(LedgerError::InsufficientFunds {
                account: cash_acc,
                available: avail,
                required: amount,
            });
        }
        let entity = self
            .entities
            .get_mut(entity_id)
            .ok_or(LedgerError::UnknownEntity(entity_id))?;
        entity.reserved_cash = entity.reserved_cash.checked_add(amount)?;
        Ok(())
    }

    /// Releases `amount` of previously reserved cash from CLOB Bid escrow.
    pub fn release_reserved_cash(
        &mut self,
        entity_id: EntityId,
        amount: Money,
    ) -> Result<(), LedgerError> {
        if amount.is_negative() {
            return Err(LedgerError::NegativeAmountNotPermitted(amount));
        }
        if amount.is_zero() {
            return Ok(());
        }
        let entity = self
            .entities
            .get_mut(entity_id)
            .ok_or(LedgerError::UnknownEntity(entity_id))?;
        if amount > entity.reserved_cash {
            return Err(LedgerError::ExcessiveReservationRelease {
                reserved: entity.reserved_cash.cents().cast_unsigned(),
                release: amount.cents().cast_unsigned(),
            });
        }
        entity.reserved_cash = entity.reserved_cash.checked_sub(amount)?;
        Ok(())
    }

    /// Atomically validates and posts a balanced `JournalEntry`.
    ///
    /// Enforces:
    /// 1. `sum(Debits) == sum(Credits)` and all amounts `> 0`.
    /// 2. All referenced `AccountId`s exist.
    /// 3. No account with `requires_non_negative_normal_balance() == true` drops below `Money::ZERO`.
    /// 4. No entity's `CashAndDeposits` drops below its `reserved_cash` escrow floor.
    pub fn post_entry(&mut self, entry: &JournalEntry) -> Result<(), LedgerError> {
        entry.validate()?;

        // Aggregate net debit/credit deltas per unique AccountId to validate resulting balances atomically
        let mut deltas: BTreeMap<AccountId, (Money, Money)> = BTreeMap::new();
        for posting in &entry.postings {
            if !self.accounts.contains_key(posting.account_id) {
                return Err(LedgerError::UnknownAccount(posting.account_id));
            }
            let pair = deltas
                .entry(posting.account_id)
                .or_insert((Money::ZERO, Money::ZERO));
            match posting.side {
                EntrySide::Debit => {
                    pair.0 = pair.0.checked_add(posting.amount)?;
                }
                EntrySide::Credit => {
                    pair.1 = pair.1.checked_add(posting.amount)?;
                }
            }
        }

        // Dry-run balance and reservation checks before mutating any state
        for (&acc_id, &(added_debits, added_credits)) in &deltas {
            let acc = self
                .accounts
                .get(acc_id)
                .ok_or(LedgerError::UnknownAccount(acc_id))?;
            let updated_debits = acc.total_debits.checked_add(added_debits)?;
            let updated_credits = acc.total_credits.checked_add(added_credits)?;
            let new_normal = match acc.category.normal_side() {
                EntrySide::Debit => updated_debits.checked_sub(updated_credits)?,
                EntrySide::Credit => updated_credits.checked_sub(updated_debits)?,
            };

            if acc.code.requires_non_negative_normal_balance() && new_normal.is_negative() {
                let current_normal = acc.normal_balance();
                let required = current_normal.checked_sub(new_normal)?;
                return Err(LedgerError::InsufficientFunds {
                    account: acc_id,
                    available: current_normal,
                    required,
                });
            }

            if acc.code == StandardAccountCode::CashAndDeposits {
                let reserved = self.reserved_cash(acc.entity_id)?;
                if new_normal < reserved {
                    let avail = acc.normal_balance().checked_sub(reserved)?;
                    let net_outflow = acc.normal_balance().checked_sub(new_normal)?;
                    return Err(LedgerError::InsufficientFunds {
                        account: acc_id,
                        available: avail,
                        required: net_outflow,
                    });
                }
            }
        }

        // Apply mutations atomically
        for (acc_id, (added_debits, added_credits)) in deltas {
            let acc = self
                .accounts
                .get_mut(acc_id)
                .ok_or(LedgerError::UnknownAccount(acc_id))?;
            acc.total_debits = acc.total_debits.checked_add(added_debits)?;
            acc.total_credits = acc.total_credits.checked_add(added_credits)?;
        }

        self.posted_entry_count = self.posted_entry_count.saturating_add(1);
        Ok(())
    }

    pub const fn posted_entry_count(&self) -> u64 {
        self.posted_entry_count
    }

    /// Closes temporary income statement accounts (`Revenue` and `Expense`) into `RetainedEarnings`
    /// for all registered entities (typically called at fiscal year-end or quarter-end).
    pub fn close_income_statement_for_entity(
        &mut self,
        tick: u64,
        entity_id: EntityId,
    ) -> Result<(), LedgerError> {
        let re_acc = self.entity_account_id(entity_id, StandardAccountCode::RetainedEarnings)?;
        let mut entry = JournalEntry::new(tick, "Close Income Statement to Retained Earnings");
        let mut net_income = Money::ZERO;

        for code in StandardAccountCode::ALL {
            let cat = code.category();
            if !matches!(cat, AccountCategory::Revenue | AccountCategory::Expense) {
                continue;
            }
            let acc_id = self.entity_account_id(entity_id, code)?;
            let bal = self.account(acc_id)?.normal_balance();
            if bal.is_zero() {
                continue;
            }
            match cat {
                AccountCategory::Revenue => {
                    if bal.is_positive() {
                        entry = entry.debit(acc_id, bal);
                    } else {
                        entry = entry.credit(acc_id, bal.checked_neg()?);
                    }
                    net_income = net_income.checked_add(bal)?;
                }
                AccountCategory::Expense => {
                    if bal.is_positive() {
                        entry = entry.credit(acc_id, bal);
                    } else {
                        entry = entry.debit(acc_id, bal.checked_neg()?);
                    }
                    net_income = net_income.checked_sub(bal)?;
                }
                _ => {}
            }
        }

        if entry.postings.is_empty() {
            return Ok(());
        }

        if net_income.is_positive() {
            entry = entry.credit(re_acc, net_income);
        } else if net_income.is_negative() {
            entry = entry.debit(re_acc, net_income.checked_neg()?);
        }

        self.post_entry(&entry)
    }

    /// Audits the closed-loop double-entry accounting invariants across every account and entity:
    /// 1. Global `sum(Debits) == sum(Credits)`.
    /// 2. Every entity satisfies `Assets - Liabilities - (Equity + Revenues - Expenses) == Money::ZERO`.
    /// 3. Total non-bank `CashAndDeposits` across all entities equals total bank `CustomerDepositsLiability`
    ///    (endogenous M1 conservation).
    pub fn audit_trial_balance(&self) -> Result<(), LedgerError> {
        let mut global_debits = Money::ZERO;
        let mut global_credits = Money::ZERO;

        for (_, acc) in &self.accounts {
            global_debits = global_debits.checked_add(acc.total_debits)?;
            global_credits = global_credits.checked_add(acc.total_credits)?;
        }

        if global_debits != global_credits {
            return Err(LedgerError::TrialBalanceViolation {
                reason: format!(
                    "global debits ({global_debits}) != global credits ({global_credits})"
                ),
            });
        }

        let mut total_non_bank_cash = Money::ZERO;
        let mut total_bank_deposit_liabilities = Money::ZERO;

        for (eid, entity) in &self.entities {
            let mut assets = Money::ZERO;
            let mut liabilities = Money::ZERO;
            let mut equity = Money::ZERO;
            let mut revenues = Money::ZERO;
            let mut expenses = Money::ZERO;

            for (&code, &acc_id) in &entity.accounts_by_code {
                let acc = self.account(acc_id)?;
                let bal = acc.normal_balance();
                match code.category() {
                    AccountCategory::Asset => assets = assets.checked_add(bal)?,
                    AccountCategory::Liability => liabilities = liabilities.checked_add(bal)?,
                    AccountCategory::Equity => equity = equity.checked_add(bal)?,
                    AccountCategory::Revenue => revenues = revenues.checked_add(bal)?,
                    AccountCategory::Expense => expenses = expenses.checked_add(bal)?,
                }
            }

            let rhs = liabilities
                .checked_add(equity)?
                .checked_add(revenues)?
                .checked_sub(expenses)?;

            if assets != rhs {
                return Err(LedgerError::TrialBalanceViolation {
                    reason: format!(
                        "entity {:?} ({}) accounting equation mismatch: Assets ({assets}) != L+E+Rev-Exp ({rhs})",
                        eid, entity.name
                    ),
                });
            }

            let cash = self.cash_balance(eid)?;
            if cash < entity.reserved_cash {
                return Err(LedgerError::TrialBalanceViolation {
                    reason: format!(
                        "entity {:?} ({}) cash ({cash}) < reserved_cash ({})",
                        eid, entity.name, entity.reserved_cash
                    ),
                });
            }

            if entity.is_bank {
                let dep_liab =
                    self.entity_balance(eid, StandardAccountCode::CustomerDepositsLiability)?;
                total_bank_deposit_liabilities =
                    total_bank_deposit_liabilities.checked_add(dep_liab)?;
            } else {
                total_non_bank_cash = total_non_bank_cash.checked_add(cash)?;
            }
        }

        if total_non_bank_cash != total_bank_deposit_liabilities {
            return Err(LedgerError::TrialBalanceViolation {
                reason: format!(
                    "M1 deposit conservation mismatch: non-bank CashAndDeposits ({total_non_bank_cash}) != bank CustomerDepositsLiability ({total_bank_deposit_liabilities})"
                ),
            });
        }

        Ok(())
    }
}
