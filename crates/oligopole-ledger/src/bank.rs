use crate::account::{EntityId, StandardAccountCode};
use crate::error::LedgerError;
use crate::journal::{JournalEntry, LedgerEngine};
use crate::money::Money;
use crate::rounding::RoundingReserve;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Active loan contract record tracked by the `CommercialBankFacility`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BankLoanAccount {
    pub borrower_id: EntityId,
    pub principal_outstanding: Money,
    pub annual_rate_bps: u32,
    pub interest_reserve: RoundingReserve,
}

/// Endogenous Commercial & Central Bank facility managing deposit endowments,
/// credit creation (`originate_loan`), principal amortization (`repay_loan_principal`),
/// 30/360 daily interest collection, and insolvency debt write-offs (`liquidate_and_write_off_default`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommercialBankFacility {
    pub bank_entity_id: Option<EntityId>,
    pub default_loan_rate_bps: u32,
    pub deposit_rate_bps: u32,
    pub loans: BTreeMap<EntityId, BankLoanAccount>,
    pub deposit_interest_reserves: BTreeMap<EntityId, RoundingReserve>,
}

impl CommercialBankFacility {
    pub fn new(bank_entity_id: EntityId, default_loan_rate_bps: u32, deposit_rate_bps: u32) -> Self {
        Self {
            bank_entity_id: Some(bank_entity_id),
            default_loan_rate_bps,
            deposit_rate_bps,
            loans: BTreeMap::new(),
            deposit_interest_reserves: BTreeMap::new(),
        }
    }

    pub fn bank_id(&self) -> Result<EntityId, LedgerError> {
        self.bank_entity_id
            .ok_or_else(|| LedgerError::TrialBalanceViolation {
                reason: "CommercialBankFacility bank_entity_id is not initialized".to_owned(),
            })
    }

    /// Seeds initial sovereign/central-bank reserve & deposit endowment for a non-bank entity at Genesis:
    /// - Bank: `Dr CentralBankReserves (Asset) / Cr CustomerDepositsLiability (Liability)`
    /// - Entity: `Dr CashAndDeposits (Asset) / Cr CommonStock (Equity)`
    pub fn seed_initial_deposit_endowment(
        &self,
        ledger: &mut LedgerEngine,
        tick: u64,
        entity_id: EntityId,
        amount: Money,
    ) -> Result<(), LedgerError> {
        if amount.is_zero() {
            return Ok(());
        }
        let bank_id = self.bank_id()?;
        let bank_reserves =
            ledger.entity_account_id(bank_id, StandardAccountCode::CentralBankReserves)?;
        let bank_dep_liab =
            ledger.entity_account_id(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
        let ent_cash =
            ledger.entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
        let ent_equity = ledger.entity_account_id(entity_id, StandardAccountCode::CommonStock)?;

        let entry = JournalEntry::new(tick, "Genesis Initial Deposit & Reserve Endowment")
            .debit(bank_reserves, amount)
            .credit(bank_dep_liab, amount)
            .debit(ent_cash, amount)
            .credit(ent_equity, amount);

        ledger.post_entry(&entry)
    }

    /// Seeds initial bank equity capital backed by Central Bank Reserves:
    /// - Bank: `Dr CentralBankReserves (Asset) / Cr CommonStock (Equity)`
    pub fn seed_bank_equity_reserves(
        &self,
        ledger: &mut LedgerEngine,
        tick: u64,
        amount: Money,
    ) -> Result<(), LedgerError> {
        if amount.is_zero() {
            return Ok(());
        }
        let bank_id = self.bank_id()?;
        let bank_reserves =
            ledger.entity_account_id(bank_id, StandardAccountCode::CentralBankReserves)?;
        let bank_equity = ledger.entity_account_id(bank_id, StandardAccountCode::CommonStock)?;

        let entry = JournalEntry::new(tick, "Genesis Bank Equity Reserve Capitalization")
            .debit(bank_reserves, amount)
            .credit(bank_equity, amount);

        ledger.post_entry(&entry)
    }

    /// Endogenously originates a commercial loan of `principal` to `borrower_id`:
    /// - Bank: `Dr LoansReceivable (Asset) / Cr CustomerDepositsLiability (Liability)`
    /// - Borrower: `Dr CashAndDeposits (Asset) / Cr LoansPayable (Liability)`
    pub fn originate_loan(
        &mut self,
        ledger: &mut LedgerEngine,
        tick: u64,
        borrower_id: EntityId,
        principal: Money,
        rate_bps_override: Option<u32>,
    ) -> Result<(), LedgerError> {
        if !principal.is_positive() {
            return Err(LedgerError::NonPositivePostingAmount(principal));
        }
        let bank_id = self.bank_id()?;
        let bank_loans =
            ledger.entity_account_id(bank_id, StandardAccountCode::LoansReceivable)?;
        let bank_dep_liab =
            ledger.entity_account_id(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
        let bor_cash =
            ledger.entity_account_id(borrower_id, StandardAccountCode::CashAndDeposits)?;
        let bor_loans =
            ledger.entity_account_id(borrower_id, StandardAccountCode::LoansPayable)?;

        let entry = JournalEntry::new(tick, "Endogenous Bank Loan Origination")
            .debit(bank_loans, principal)
            .credit(bank_dep_liab, principal)
            .debit(bor_cash, principal)
            .credit(bor_loans, principal);

        ledger.post_entry(&entry)?;

        let rate_bps = rate_bps_override.unwrap_or(self.default_loan_rate_bps);
        let loan = self
            .loans
            .entry(borrower_id)
            .or_insert_with(|| BankLoanAccount {
                borrower_id,
                principal_outstanding: Money::ZERO,
                annual_rate_bps: rate_bps,
                interest_reserve: RoundingReserve::new(),
            });
        loan.principal_outstanding = loan.principal_outstanding.checked_add(principal)?;
        loan.annual_rate_bps = rate_bps;
        Ok(())
    }

    /// Repays `amount` of loan principal using the borrower's unreserved deposit cash,
    /// extinguishing matching deposit liabilities and loan assets:
    /// - Bank: `Dr CustomerDepositsLiability (Liability) / Cr LoansReceivable (Asset)`
    /// - Borrower: `Dr LoansPayable (Liability) / Cr CashAndDeposits (Asset)`
    pub fn repay_loan_principal(
        &mut self,
        ledger: &mut LedgerEngine,
        tick: u64,
        borrower_id: EntityId,
        amount: Money,
    ) -> Result<Money, LedgerError> {
        if amount.is_zero() {
            return Ok(Money::ZERO);
        }
        if amount.is_negative() {
            return Err(LedgerError::NegativeAmountNotPermitted(amount));
        }

        let outstanding = self
            .loans
            .get(&borrower_id)
            .map_or(Money::ZERO, |l| l.principal_outstanding);
        let actual = amount.min(outstanding);
        if actual.is_zero() {
            return Ok(Money::ZERO);
        }

        let bank_id = self.bank_id()?;
        let bank_dep_liab =
            ledger.entity_account_id(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
        let bank_loans =
            ledger.entity_account_id(bank_id, StandardAccountCode::LoansReceivable)?;
        let bor_loans =
            ledger.entity_account_id(borrower_id, StandardAccountCode::LoansPayable)?;
        let bor_cash =
            ledger.entity_account_id(borrower_id, StandardAccountCode::CashAndDeposits)?;

        let entry = JournalEntry::new(tick, "Endogenous Bank Loan Principal Repayment")
            .debit(bank_dep_liab, actual)
            .credit(bank_loans, actual)
            .debit(bor_loans, actual)
            .credit(bor_cash, actual);

        ledger.post_entry(&entry)?;

        if let Some(loan) = self.loans.get_mut(&borrower_id) {
            loan.principal_outstanding = loan.principal_outstanding.checked_sub(actual)?;
        }
        Ok(actual)
    }

    /// Accrues one 30/360 day of loan interest on `borrower_id` and collects as much as
    /// the borrower's available unreserved cash above `protected_cash_floor` can cover.
    ///
    /// Returns `(interest_paid, unpaid_shortfall)`.
    pub fn accrue_and_collect_daily_loan_interest(
        &mut self,
        ledger: &mut LedgerEngine,
        tick: u64,
        borrower_id: EntityId,
    ) -> Result<(Money, Money), LedgerError> {
        self.accrue_and_collect_daily_loan_interest_with_floor(
            ledger,
            tick,
            borrower_id,
            Money::ZERO,
        )
    }

    /// Accrues one 30/360 day of loan interest on `borrower_id` while preserving `protected_cash_floor`
    /// in the borrower's cash account. Any unpaid interest shortfall is capitalized onto `LoansPayable` /
    /// `LoansReceivable` so solvent operating firms never drop to `$0.00` cash intraday.
    pub fn accrue_and_collect_daily_loan_interest_with_floor(
        &mut self,
        ledger: &mut LedgerEngine,
        tick: u64,
        borrower_id: EntityId,
        protected_cash_floor: Money,
    ) -> Result<(Money, Money), LedgerError> {
        let bank_id = self.bank_id()?;
        let Some(loan) = self.loans.get_mut(&borrower_id) else {
            return Ok((Money::ZERO, Money::ZERO));
        };
        if loan.principal_outstanding.is_zero() {
            return Ok((Money::ZERO, Money::ZERO));
        }

        let due = loan
            .interest_reserve
            .accrue_daily_360_bps(loan.principal_outstanding, loan.annual_rate_bps)?;
        if due.is_zero() {
            return Ok((Money::ZERO, Money::ZERO));
        }

        let avail = ledger.available_cash(borrower_id)?;
        let spendable = if avail > protected_cash_floor {
            avail.checked_sub(protected_cash_floor)?
        } else {
            Money::ZERO
        };
        let paid = due.min(spendable);
        let shortfall = due.checked_sub(paid)?;

        if paid.is_positive() {
            let bank_dep_liab = ledger
                .entity_account_id(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
            let bank_int_inc =
                ledger.entity_account_id(bank_id, StandardAccountCode::InterestIncome)?;
            let bor_int_exp =
                ledger.entity_account_id(borrower_id, StandardAccountCode::InterestExpense)?;
            let bor_cash =
                ledger.entity_account_id(borrower_id, StandardAccountCode::CashAndDeposits)?;

            let entry = JournalEntry::new(tick, "Daily 30/360 Loan Interest Payment")
                .debit(bank_dep_liab, paid)
                .credit(bank_int_inc, paid)
                .debit(bor_int_exp, paid)
                .credit(bor_cash, paid);
            ledger.post_entry(&entry)?;
        }

        Ok((paid, shortfall))
    }

    /// Accrues and pays one 30/360 day of deposit interest from the Bank to `depositor_id`:
    /// - Bank: `Dr InterestExpense (Expense) / Cr CustomerDepositsLiability (Liability)`
    /// - Depositor: `Dr CashAndDeposits (Asset) / Cr InterestIncome (Revenue)`
    pub fn accrue_and_pay_daily_deposit_interest(
        &mut self,
        ledger: &mut LedgerEngine,
        tick: u64,
        depositor_id: EntityId,
    ) -> Result<Money, LedgerError> {
        if self.deposit_rate_bps == 0 {
            return Ok(Money::ZERO);
        }
        let bank_id = self.bank_id()?;
        if depositor_id == bank_id {
            return Ok(Money::ZERO);
        }

        let cash_bal = ledger.cash_balance(depositor_id)?;
        if !cash_bal.is_positive() {
            return Ok(Money::ZERO);
        }

        let reserve = self
            .deposit_interest_reserves
            .entry(depositor_id)
            .or_default();
        let interest = reserve.accrue_daily_360_bps(cash_bal, self.deposit_rate_bps)?;
        if interest.is_zero() {
            return Ok(Money::ZERO);
        }

        let bank_int_exp =
            ledger.entity_account_id(bank_id, StandardAccountCode::InterestExpense)?;
        let bank_dep_liab =
            ledger.entity_account_id(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
        let dep_cash =
            ledger.entity_account_id(depositor_id, StandardAccountCode::CashAndDeposits)?;
        let dep_int_inc =
            ledger.entity_account_id(depositor_id, StandardAccountCode::InterestIncome)?;

        let entry = JournalEntry::new(tick, "Daily 30/360 Deposit Interest Credit")
            .debit(bank_int_exp, interest)
            .credit(bank_dep_liab, interest)
            .debit(dep_cash, interest)
            .credit(dep_int_inc, interest);
        ledger.post_entry(&entry)?;
        Ok(interest)
    }

    /// Executes the corporate default & insolvency liquidation waterfall for `borrower_id`:
    /// 1. Releases any cash reservations held by `borrower_id`.
    /// 2. Sweeps all remaining `CashAndDeposits` toward `LoansPayable` principal.
    /// 3. Writes off any unpaid deficiency `D` symmetrically:
    ///    - Bank: `Dr BadDebtExpense (Expense) / Cr LoansReceivable (Asset)`
    ///    - Borrower: `Dr LoansPayable (Liability) / Cr DebtExtinguishmentGain (Revenue)`
    ///
    /// Returns `(recovered_principal, written_off_deficiency)`.
    pub fn liquidate_and_write_off_default(
        &mut self,
        ledger: &mut LedgerEngine,
        tick: u64,
        borrower_id: EntityId,
    ) -> Result<(Money, Money), LedgerError> {
        let reserved = ledger.reserved_cash(borrower_id)?;
        if reserved.is_positive() {
            ledger.release_reserved_cash(borrower_id, reserved)?;
        }

        let outstanding = self
            .loans
            .get(&borrower_id)
            .map_or(Money::ZERO, |l| l.principal_outstanding);
        if outstanding.is_zero() {
            return Ok((Money::ZERO, Money::ZERO));
        }

        let cash = ledger.cash_balance(borrower_id)?;
        let recover = cash.min(outstanding);
        let recovered = self.repay_loan_principal(ledger, tick, borrower_id, recover)?;

        let remaining_debt = self
            .loans
            .get(&borrower_id)
            .map_or(Money::ZERO, |l| l.principal_outstanding);

        if remaining_debt.is_positive() {
            let bank_id = self.bank_id()?;
            let bank_bad_debt =
                ledger.entity_account_id(bank_id, StandardAccountCode::BadDebtExpense)?;
            let bank_loans =
                ledger.entity_account_id(bank_id, StandardAccountCode::LoansReceivable)?;
            let bor_loans =
                ledger.entity_account_id(borrower_id, StandardAccountCode::LoansPayable)?;
            let bor_gain = ledger
                .entity_account_id(borrower_id, StandardAccountCode::DebtExtinguishmentGain)?;

            let writeoff_entry = JournalEntry::new(tick, "Insolvency Loan Deficiency Write-Off")
                .debit(bank_bad_debt, remaining_debt)
                .credit(bank_loans, remaining_debt)
                .debit(bor_loans, remaining_debt)
                .credit(bor_gain, remaining_debt);

            ledger.post_entry(&writeoff_entry)?;

            if let Some(loan) = self.loans.get_mut(&borrower_id) {
                loan.principal_outstanding = Money::ZERO;
            }
        }

        Ok((recovered, remaining_debt))
    }
}
