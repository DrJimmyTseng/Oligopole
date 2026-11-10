use crate::account::{AccountCategory, EntityId, StandardAccountCode};
use crate::error::LedgerError;
use crate::journal::LedgerEngine;
use crate::money::Money;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Snapshot of an entity's Balance Sheet & Income Statement at a given tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinancialStatementReport {
    pub entity_id: EntityId,
    pub entity_name: String,
    pub tick: u64,
    pub total_assets: Money,
    pub total_liabilities: Money,
    pub total_equity_including_net_income: Money,
    pub total_revenue: Money,
    pub total_expenses: Money,
    pub net_income: Money,
    pub line_items: BTreeMap<StandardAccountCode, Money>,
}

impl FinancialStatementReport {
    /// Generates a complete `FinancialStatementReport` for `entity_id` from `ledger`.
    pub fn generate(
        ledger: &LedgerEngine,
        entity_id: EntityId,
        tick: u64,
    ) -> Result<Self, LedgerError> {
        let entity = ledger.entity(entity_id)?;
        let mut total_assets = Money::ZERO;
        let mut total_liabilities = Money::ZERO;
        let mut base_equity = Money::ZERO;
        let mut total_revenue = Money::ZERO;
        let mut total_expenses = Money::ZERO;
        let mut line_items = BTreeMap::new();

        for (&code, &acc_id) in &entity.accounts_by_code {
            let bal = ledger.account(acc_id)?.normal_balance();
            line_items.insert(code, bal);
            match code.category() {
                AccountCategory::Asset => total_assets = total_assets.checked_add(bal)?,
                AccountCategory::Liability => {
                    total_liabilities = total_liabilities.checked_add(bal)?;
                }
                AccountCategory::Equity => base_equity = base_equity.checked_add(bal)?,
                AccountCategory::Revenue => total_revenue = total_revenue.checked_add(bal)?,
                AccountCategory::Expense => total_expenses = total_expenses.checked_add(bal)?,
            }
        }

        let net_income = total_revenue.checked_sub(total_expenses)?;
        let total_equity_including_net_income = base_equity.checked_add(net_income)?;

        Ok(Self {
            entity_id,
            entity_name: entity.name.clone(),
            tick,
            total_assets,
            total_liabilities,
            total_equity_including_net_income,
            total_revenue,
            total_expenses,
            net_income,
            line_items,
        })
    }
}
