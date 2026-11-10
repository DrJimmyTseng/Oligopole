use crate::b2b_market::B2BSupplyContract;
use crate::calendar::{Date360, Tick};
use crate::clob::{InstrumentKind, OhlcvBar, OrderBookDepthLevel, TradeFill};
use crate::household::HouseholdCohortKind;
use crate::ids::{CohortId, CorpId, InstrumentId, SkuId};
use crate::production::{
    AiDelegationConfig, CloudTier, CorporationRole, CreditRating, ExecutivePersonaKind,
};
use crate::wire::{ShortPositionRecord, WireMessage};
use oligopole_ledger::{EntityId, FinancialStatementReport, Money, WacInventoryLot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Category tag for a live Bloomberg corporate/macro news wire headline (`NEWS <GO>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NewsCategory {
    CapExAndExpansion,
    RdAndMaintenance,
    CreditAndDebt,
    DividendAndBuyback,
    QuarterlyEarnings,
}

impl NewsCategory {
    #[inline]
    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::CapExAndExpansion => "CAPEX",
            Self::RdAndMaintenance => "R&D/MAINT",
            Self::CreditAndDebt => "CREDIT",
            Self::DividendAndBuyback => "CAPITAL",
            Self::QuarterlyEarnings => "EARNINGS",
        }
    }
}

/// A timestamped deterministic headline recorded on the simulation's Bloomberg News Wire (`NEWS <GO>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsHeadline {
    pub seq_id: u64,
    pub tick: Tick,
    pub date: Date360,
    pub ticker: String,
    pub category: NewsCategory,
    pub headline: String,
}

/// Per-shareholder ownership breakdown row for the Cap Table Inspector (`CAP <GO>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareholderRecord {
    pub holder_entity_id: EntityId,
    pub holder_name: String,
    pub owned_shares: u64,
    pub reserved_shares: u64,
    pub ownership_bps: u32,
}

/// Macroeconomic & industrial concentration telemetry for the `ECO <GO>` dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroTelemetrySnapshot {
    /// Retail Consumer Price Index in basis points (`10_000 = 100.0` baseline).
    pub retail_cpi_index_bps: u32,
    /// Trailing 30-day gross trade & retail transaction velocity (`GDP` proxy).
    pub trailing_30d_gdp: Money,
    /// Average physical plant condition across all corporations (`3_500..=10_000 bps`).
    pub avg_facility_condition_bps: u32,
    /// Average product quality score across all corporations (`bps`).
    pub avg_quality_score_bps: u32,
    /// Herfindahl-Hirschman Index (`0..=10_000`) for the Resource Extraction sector.
    pub extractor_hhi: u32,
    /// Herfindahl-Hirschman Index (`0..=10_000`) for the Manufacturing sector.
    pub manufacturer_hhi: u32,
    /// Herfindahl-Hirschman Index (`0..=10_000`) for the B2C Retail sector.
    pub retail_hhi: u32,
}

/// Read-only projection of a tradable CLOB instrument for the 120 FPS `egui` terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSnapshot {
    pub instrument_id: InstrumentId,
    pub ticker: String,
    pub kind: InstrumentKind,
    pub last_price: Money,
    pub luld_reference_price: Money,
    pub luld_band_bps: u32,
    pub short_interest_shares: u64,
    pub best_bid: Option<Money>,
    pub best_ask: Option<Money>,
    pub top_bids: Vec<OrderBookDepthLevel>,
    pub top_asks: Vec<OrderBookDepthLevel>,
    pub ohlcv_bars: Vec<OhlcvBar>,
    pub recent_fills: Vec<TradeFill>,
}

/// Read-only projection of a Corporation's operational and financial status for the 120 FPS `egui` terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorporationSnapshot {
    pub corp_id: CorpId,
    pub entity_id: EntityId,
    pub equity_instrument_id: Option<InstrumentId>,
    pub ticker: String,
    pub name: String,
    pub role: CorporationRole,
    pub persona_kind: ExecutivePersonaKind,
    pub delegation: AiDelegationConfig,
    pub cash_balance: Money,
    pub available_cash: Money,
    pub illicit_cash_balance: Money,
    pub tax_lien_payable: Money,
    pub service_credits: u64,
    pub cloud_tier: CloudTier,
    pub subscribed_cloud_tier: CloudTier,
    pub credit_rating: CreditRating,
    pub effective_loan_rate_bps: u32,
    pub loans_payable: Money,
    pub daily_capacity: u64,
    pub effective_daily_capacity: u64,
    pub facility_condition_bps: u32,
    pub daily_wage_bill: Money,
    pub daily_manager_bill: Money,
    pub posted_selling_price: Money,
    pub max_input_bid_price: Money,
    pub quality_score_bps: u32,
    pub brand_score_bps: u32,
    pub target_dividend_payout_bps: u32,
    pub total_shares_outstanding: u64,
    pub share_price: Money,
    pub market_cap: Money,
    pub last_monthly_dividend_paid: Money,
    pub founded_tick: u64,
    pub founder_lockup_until_tick: u64,
    pub monthly_genuine_b2c_revenue: Money,
    pub monthly_laundered_cash: Money,
    pub ransomware_degraded_until_tick: u64,
    pub security_shield_until_tick: u64,
    pub logistics_shield_until_tick: u64,
    pub cap_table: Vec<ShareholderRecord>,
    pub inventory: BTreeMap<SkuId, WacInventoryLot>,
    pub financial_statement: FinancialStatementReport,
}

/// Read-only projection of a Household Income Cohort for the 120 FPS `egui` terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CohortSnapshot {
    pub cohort_id: CohortId,
    pub entity_id: EntityId,
    pub kind: HouseholdCohortKind,
    pub name: String,
    pub population_count: u64,
    pub cash_deposits: Money,
    pub savings_subsistence_floor: Money,
    pub financial_statement: FinancialStatementReport,
}

/// Immutable, zero-lock world snapshot published via `tokio::sync::watch<Arc<WorldView>>`
/// from the simulation thread (or authoritative Axum WebSocket stream) to the 120 FPS `egui` Bloomberg Terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldView {
    pub tick: Tick,
    pub date: Date360,
    pub bank_entity_id: EntityId,
    pub bank_statement: FinancialStatementReport,
    pub bank_loan_rate_bps: u32,
    pub bank_deposit_rate_bps: u32,
    pub sovereign_entity_id: EntityId,
    pub sovereign_cash_balance: Money,
    pub sovereign_dr_reserve_buffer: Money,
    pub total_m1_money_supply: Money,
    pub total_bank_loans: Money,
    pub total_service_credits_supply: u64,
    pub macro_telemetry: MacroTelemetrySnapshot,
    pub corporations: Vec<CorporationSnapshot>,
    pub cohorts: Vec<CohortSnapshot>,
    pub instruments: Vec<InstrumentSnapshot>,
    pub active_b2b_contracts: Vec<B2BSupplyContract>,
    pub wire_messages: Vec<WireMessage>,
    pub visible_short_positions: Vec<ShortPositionRecord>,
    pub news_feed: Vec<NewsHeadline>,
    pub sku_codes: BTreeMap<SkuId, String>,
}
