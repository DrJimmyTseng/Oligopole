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
    /// Provides B2B freight transport capacity, earning per-unit freight tariffs and shielding cargo against hijacking.
    LogisticsTrucking { freight_sku: SkuId },
    /// Provides corporate security retainers, shielding clients against extortion, cargo hijacking, and wiretaps.
    PrivateSecurity { protected_sku: SkuId },
}

/// Recurring monthly `SovereignCloud` Web Services subscription tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum CloudTier {
    /// Unpaid / degraded offline POS mode (-50% retail brand effectiveness, L1-only book, AI/firewall disabled).
    Offline,
    /// Tier 1 ($1,000 VC or 1 ServiceCredit/mo): Standard POS clearing & `MSG` corporate wire.
    #[default]
    Tier1PosWire,
    /// Tier 2 ($5,000 VC or 5 ServiceCredits/mo): Full 10-level L2 `OB` depth, multi-site `OPS` & Short-Locate Vault.
    Tier2L2Telemetry,
    /// Tier 3 ($15,000 VC or 15 ServiceCredits/mo): Autonomous `Clef-Flash` 9B delegation & Cyber-Defense Firewall.
    Tier3ClefFirewall,
}

impl CloudTier {
    pub const ALL: [Self; 4] = [
        Self::Offline,
        Self::Tier1PosWire,
        Self::Tier2L2Telemetry,
        Self::Tier3ClefFirewall,
    ];

    #[inline]
    pub const fn monthly_service_credits(self) -> u64 {
        match self {
            Self::Offline => 0,
            Self::Tier1PosWire => 1,
            Self::Tier2L2Telemetry => 5,
            Self::Tier3ClefFirewall => 15,
        }
    }

    #[inline]
    pub const fn monthly_vc_cost(self) -> Money {
        match self {
            Self::Offline => Money::ZERO,
            Self::Tier1PosWire => Money::from_dollars(1_000),
            Self::Tier2L2Telemetry => Money::from_dollars(5_000),
            Self::Tier3ClefFirewall => Money::from_dollars(15_000),
        }
    }

    #[inline]
    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::Offline => "OFFLINE (DEGRADED)",
            Self::Tier1PosWire => "TIER 1: POS + WIRE",
            Self::Tier2L2Telemetry => "TIER 2: L2 + VAULT",
            Self::Tier3ClefFirewall => "TIER 3: CLEF + FIREWALL",
        }
    }
}

/// Institutional corporate credit rating tier determining borrowing spread over the bank's base rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum CreditRating {
    #[default]
    AAA,
    AA,
    A,
    BBB,
    BB,
    CCC,
}

impl CreditRating {
    #[inline]
    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::AAA => "AAA",
            Self::AA => "AA",
            Self::A => "A",
            Self::BBB => "BBB",
            Self::BB => "BB (HY)",
            Self::CCC => "CCC (DISTRESSED)",
        }
    }

    /// Credit spread in basis points added on top of `bank.default_loan_rate_bps`.
    #[inline]
    pub const fn spread_bps(self) -> u32 {
        match self {
            Self::AAA => 0,
            Self::AA => 40,
            Self::A => 90,
            Self::BBB => 160,
            Self::BB => 275,
            Self::CCC => 450,
        }
    }

    /// Evaluates a corporation's institutional credit rating from its available cash,
    /// 20-day working-capital floor, outstanding debt, and GAAP book equity.
    pub fn evaluate(
        available_cash: Money,
        working_floor_20d: Money,
        outstanding_debt: Money,
        total_equity: Money,
    ) -> Self {
        if total_equity <= Money::ZERO || available_cash.cents() * 4 < working_floor_20d.cents() {
            return Self::CCC;
        }
        let debt_c = outstanding_debt.cents().max(0);
        let eq_c = total_equity.cents().max(1);
        let dte_bps = (debt_c * 10_000) / eq_c;

        if debt_c == 0 && available_cash >= working_floor_20d {
            Self::AAA
        } else if dte_bps <= 2_500 && available_cash >= working_floor_20d {
            Self::AA
        } else if dte_bps <= 5_500 && available_cash.cents() * 2 >= working_floor_20d.cents() {
            Self::A
        } else if dte_bps <= 10_000 && available_cash.cents() * 2 >= working_floor_20d.cents() {
            Self::BBB
        } else if dte_bps <= 18_000 {
            Self::BB
        } else {
            Self::CCC
        }
    }
}

/// Executive persona archetype distinguishing corporate leadership behavior across the 6 firms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExecutivePersonaKind {
    /// Balanced player-controlled or neutral steward.
    BalancedExecutive,
    /// Conservative dividend aristocrat prioritizing balance-sheet fortress and high shareholder payouts.
    DividendAristocrat,
    /// Aggressive debt-funded capacity builder pursuing volume dominance.
    AggressiveEmpireBuilder,
    /// Quality & brand moat compounding specialist defending premium gross margins.
    QualityMoatCompounder,
    /// Lean cost-cutting & undercut specialist competing on price velocity.
    CostUndercutChallenger,
    /// Opportunistic share-cannibal & capital allocator balancing high ROE with aggressive buybacks.
    OpportunisticCapitalAllocator,
}

impl ExecutivePersonaKind {
    #[inline]
    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::BalancedExecutive => "BALANCED STEWARD",
            Self::DividendAristocrat => "DIVIDEND ARISTOCRAT",
            Self::AggressiveEmpireBuilder => "EMPIRE BUILDER",
            Self::QualityMoatCompounder => "MOAT COMPOUNDER",
            Self::CostUndercutChallenger => "PRICE CHALLENGER",
            Self::OpportunisticCapitalAllocator => "CAPITAL ALLOCATOR",
        }
    }
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

    /// Returns `true` if at least one Slow-Tick strategic domain (`pricing` or `capex_and_financing`)
    /// is delegated to the AI policy engine.
    #[inline]
    pub const fn accepts_slow_tick_ai(self) -> bool {
        !self.is_player_controlled
            || self.delegate_pricing_to_ai
            || self.delegate_capex_and_financing_to_ai
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
    pub persona_kind: ExecutivePersonaKind,
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
    /// Trailing monthly dividend paid (for secondary Equity CLOB dividend-yield valuation).
    pub last_monthly_dividend_paid: Money,
    /// Tick when this corporation was founded (used for the >= 90-day IPO GAAP operating history gate).
    pub founded_tick: u64,
    /// Optional founder `EntityId` subject to post-IPO share lockup escrow.
    pub founder_entity_id: Option<EntityId>,
    /// Tick until which `founder_locked_shares` cannot be sold on the CLOB (`180` ticks post-IPO).
    pub founder_lockup_until_tick: u64,
    /// Number of founder shares locked in post-IPO escrow.
    pub founder_locked_shares: u64,
    /// Currently active `SovereignCloud` tier (after month-end billing resolution).
    pub cloud_tier: CloudTier,
    /// Desired `SovereignCloud` subscription tier for next month-end billing cycle.
    pub subscribed_cloud_tier: CloudTier,
    /// Cumulative genuine `WacInventoryLot`-backed B2C retail revenue in the current month.
    pub monthly_genuine_b2c_revenue: Money,
    /// Cumulative `IllicitCash` laundered through retail placement in the current month.
    pub monthly_laundered_cash: Money,
    /// Tick until which `CyberRansomware` degrades `daily_capacity` by 40%.
    pub ransomware_degraded_until_tick: u64,
    /// Tick until which an active `PrivateSecurity` retainer shields against extortion & hijacking.
    pub security_shield_until_tick: u64,
    /// Tick until which an active `LogisticsTrucking` retainer shields B2B cargo deliveries.
    pub logistics_shield_until_tick: u64,
    /// Physical facility condition in basis points (`3_500..=10_000`), scaling effective daily capacity.
    pub facility_condition_bps: u32,
    /// Institutional credit rating (`AAA`..=`CCC`) governing dynamic commercial bank loan spreads.
    pub credit_rating: CreditRating,
}

impl Corporation {
    pub fn inventory_lot(&self, sku_id: SkuId) -> WacInventoryLot {
        self.inventory.get(&sku_id).copied().unwrap_or_default()
    }

    pub fn inventory_lot_mut(&mut self, sku_id: SkuId) -> &mut WacInventoryLot {
        self.inventory.entry(sku_id).or_default()
    }

    /// Effective daily production/extraction capacity scaled by `facility_condition_bps` (`>= 1` when `daily_capacity > 0`).
    #[inline]
    pub fn effective_daily_capacity(&self) -> u64 {
        if self.daily_capacity == 0 {
            return 0;
        }
        let cond = u64::from(self.facility_condition_bps.clamp(3_500, 10_000));
        ((self.daily_capacity.saturating_mul(cond) + 5_000) / 10_000).max(1)
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
