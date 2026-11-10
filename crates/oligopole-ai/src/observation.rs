use crate::error::AiError;
use oligopole_core::{CorpId, CorporationRole, Tick, WorldState};
use oligopole_ledger::{FinancialStatementReport, Money, StandardAccountCode};
use serde::{Deserialize, Serialize};

/// Strict asymmetric information boundary projection constructed for a single `CorpId`
/// prior to a Monthly Slow Tick.
///
/// Exposes ONLY:
/// 1. The actor's private internal books and inventory lots.
/// 2. Public market quotes (CLOB last prices, bank loan/deposit interest rates).
/// 3. Lagged public competitor selling prices (no access to rival unreserved cash or private order queues).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorObservationView {
    pub tick: Tick,
    pub corp_id: CorpId,
    pub ticker: String,
    pub role: CorporationRole,
    pub available_cash: Money,
    pub outstanding_debt: Money,
    pub net_income: Money,
    pub daily_capacity: u64,
    pub daily_wage_bill: Money,
    pub daily_manager_bill: Money,
    pub posted_selling_price: Money,
    pub max_input_bid_price: Money,
    pub quality_score_bps: u32,
    pub brand_score_bps: u32,
    pub primary_inventory_units: u64,
    pub monthly_units_output: u64,
    pub monthly_units_sold: u64,
    pub competitor_average_price: Money,
    pub input_market_price: Money,
    pub bank_loan_rate_bps: u32,
}

impl ActorObservationView {
    /// Builds an asymmetric `ActorObservationView` for `corp_id` from `world`.
    pub fn build(world: &WorldState, corp_id: CorpId) -> Result<Self, AiError> {
        let corp = world
            .corporations
            .get(corp_id)
            .ok_or(oligopole_core::CoreError::UnknownCorporation(corp_id))?;

        let available_cash = world.ledger.available_cash(corp.entity_id)?;
        let outstanding_debt = world
            .ledger
            .entity_balance(corp.entity_id, StandardAccountCode::LoansPayable)?;
        let report =
            FinancialStatementReport::generate(&world.ledger, corp.entity_id, world.current_tick.0)?;

        let primary_sku = match corp.role {
            CorporationRole::ResourceExtractor { output_sku }
            | CorporationRole::Manufacturer { output_sku, .. } => output_sku,
            CorporationRole::Retailer { retail_sku } => retail_sku,
        };
        let primary_inventory_units = corp.inventory_lot(primary_sku).available_qty();

        // Compute average posted price of direct peers in the same role
        let mut peer_sum = Money::ZERO;
        let mut peer_count = 0i128;
        for (other_id, other) in &world.corporations {
            if other_id != corp_id
                && std::mem::discriminant(&other.role) == std::mem::discriminant(&corp.role)
            {
                peer_sum = peer_sum.checked_add(other.posted_selling_price)?;
                peer_count += 1;
            }
        }
        let competitor_average_price = if peer_count > 0 {
            Money::from_cents(peer_sum.cents() / peer_count)
        } else {
            corp.posted_selling_price
        };

        let input_market_price = match corp.role {
            CorporationRole::ResourceExtractor { output_sku } => world
                .skus
                .get(output_sku)
                .map_or(Money::from_dollars(40), |s| s.base_reference_price),
            CorporationRole::Manufacturer { input_sku, .. } => world
                .order_books
                .values()
                .find(|b| {
                    matches!(
                        b.kind,
                        oligopole_core::InstrumentKind::Commodity { sku_id } if sku_id == input_sku
                    )
                })
                .map_or(Money::from_dollars(42), |b| b.last_price),
            CorporationRole::Retailer { retail_sku } => world
                .corporations
                .values()
                .filter(|c| {
                    matches!(
                        c.role,
                        CorporationRole::Manufacturer { output_sku, .. } if output_sku == retail_sku
                    )
                })
                .map(|c| c.posted_selling_price)
                .min()
                .unwrap_or(Money::from_dollars(130)),
        };

        Ok(Self {
            tick: world.current_tick,
            corp_id,
            ticker: corp.ticker.clone(),
            role: corp.role,
            available_cash,
            outstanding_debt,
            net_income: report.net_income,
            daily_capacity: corp.daily_capacity,
            daily_wage_bill: corp.daily_wage_bill,
            daily_manager_bill: corp.daily_manager_bill,
            posted_selling_price: corp.posted_selling_price,
            max_input_bid_price: corp.max_input_bid_price,
            quality_score_bps: corp.quality_score_bps,
            brand_score_bps: corp.brand_score_bps,
            primary_inventory_units,
            monthly_units_output: corp.monthly_units_output,
            monthly_units_sold: corp.monthly_units_sold,
            competitor_average_price,
            input_market_price,
            bank_loan_rate_bps: world.bank.default_loan_rate_bps,
        })
    }

    /// Formats a compact, deterministic text dossier for `Clef-Flash` 9B single-pass prefill.
    pub fn to_clef_dossier(&self) -> String {
        format!(
            "TICK={} CORP={} ROLE={:?} CASH={} DEBT={} NET_INC={} CAP={} INV={} OUT_30D={} SOLD_30D={} PRICE={} PEER_PRICE={} IN_COST={} LOAN_BPS={}",
            self.tick.0,
            self.ticker,
            self.role,
            self.available_cash,
            self.outstanding_debt,
            self.net_income,
            self.daily_capacity,
            self.primary_inventory_units,
            self.monthly_units_output,
            self.monthly_units_sold,
            self.posted_selling_price,
            self.competitor_average_price,
            self.input_market_price,
            self.bank_loan_rate_bps
        )
    }
}
