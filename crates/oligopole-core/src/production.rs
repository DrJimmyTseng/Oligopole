use crate::ids::{CorpId, InstrumentId, SkuId};
use oligopole_ledger::{EntityId, Money, StandardAccountCode, WacInventoryLot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Physical good / SKU metadata in the simulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkuDefinition {
    pub sku_id: SkuId,
    pub code: String,
    pub name: String,
    pub ledger_inventory_account: StandardAccountCode,
    pub base_reference_price: Money,
}

/// Industrial classification of a Corporation in the supply-chain DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CorporationRole {
    /// Extracts primary raw materials (`IRON_ORE`) using labor and posts asks to the Commodity CLOB.
    ResourceExtractor { output_sku: SkuId },
    /// Procures raw materials on the CLOB/B2B, transforms them into finished goods via a recipe, and sells B2B wholesale.
    Manufacturer {
        input_sku: SkuId,
        input_units_per_batch: u64,
        output_sku: SkuId,
        output_units_per_batch: u64,
    },
    /// Procures finished goods wholesale from manufacturers and sells on B2C retail shelves to households.
    Retailer { retail_sku: SkuId },
}

/// Executive AI delegation switches allowing the Player (or Rival CEOs) to delegate specific domains to `Clef-Flash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiDelegationConfig {
    pub is_player_controlled: bool,
    pub delegate_pricing_to_ai: bool,
    pub delegate_capex_and_financing_to_ai: bool,
    pub delegate_procurement_to_ai: bool,
}

impl AiDelegationConfig {
    pub const fn autonomous_rival() -> Self {
        Self {
            is_player_controlled: false,
            delegate_pricing_to_ai: true,
            delegate_capex_and_financing_to_ai: true,
            delegate_procurement_to_ai: true,
        }
    }

    pub const fn player_default() -> Self {
        Self {
            is_player_controlled: true,
            delegate_pricing_to_ai: false,
            delegate_capex_and_financing_to_ai: false,
            delegate_procurement_to_ai: false,
        }
    }
}

/// Corporation aggregate root combining operational facilities, WAC inventory lots,
/// pricing dials, labor payroll, and equity share structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Corporation {
    pub corp_id: CorpId,
    pub entity_id: EntityId,
    pub equity_instrument_id: Option<InstrumentId>,
    pub ticker: String,
    pub name: String,
    pub role: CorporationRole,
    pub delegation: AiDelegationConfig,
    /// Daily batch capacity (batches/day for extractors & manufacturers; max daily replenishment units for retailers).
    pub daily_capacity: u64,
    /// Daily payroll paid to the `WageEarners` cohort.
    pub daily_wage_bill: Money,
    /// Daily executive/management salary paid to the `ManagersSkilled` cohort.
    pub daily_manager_bill: Money,
    /// Posted selling price per unit (CLOB ask limit price for extractors, wholesale price for manufacturers, shelf price for retailers).
    pub posted_selling_price: Money,
    /// Maximum willingness-to-pay bid price for input procurement.
    pub max_input_bid_price: Money,
    /// Product quality score in basis points (`0..=20_000`).
    pub quality_score_bps: u32,
    /// Brand awareness score in basis points (`0..=20_000`).
    pub brand_score_bps: u32,
    /// Quarterly/Monthly target dividend payout ratio in basis points (`0..=10_000`).
    pub target_dividend_payout_bps: u32,
    /// Total issued shares outstanding.
    pub total_shares_outstanding: u64,
    /// Weighted Average Cost (WAC) inventory lots indexed by `SkuId`.
    pub inventory: BTreeMap<SkuId, WacInventoryLot>,
    /// Cumulative units produced or procured in the current month.
    pub monthly_units_output: u64,
    /// Cumulative units sold in the current month.
    pub monthly_units_sold: u64,
}

impl Corporation {
    pub fn inventory_lot(&self, sku_id: SkuId) -> WacInventoryLot {
        self.inventory.get(&sku_id).copied().unwrap_or_default()
    }

    pub fn inventory_lot_mut(&mut self, sku_id: SkuId) -> &mut WacInventoryLot {
        self.inventory.entry(sku_id).or_default()
    }

    /// Computes the corporation's N-day working-capital floor (`days * (daily_wage_bill + daily_manager_bill + daily_loan_interest)`).
    pub fn working_capital_floor(
        &self,
        days: u64,
        outstanding_debt: Money,
        loan_rate_bps: u32,
    ) -> Result<Money, oligopole_ledger::LedgerError> {
        let daily_payroll = self.daily_wage_bill.checked_add(self.daily_manager_bill)?;
        let (daily_interest, _) = outstanding_debt
            .mul_div_rem_u64(u64::from(loan_rate_bps), 360 * oligopole_ledger::BPS_DENOMINATOR)?;
        let daily_burn = daily_payroll.checked_add(daily_interest)?;
        daily_burn.checked_mul_u64(days)
    }
}
