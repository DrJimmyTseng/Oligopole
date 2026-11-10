use crate::clob::OrderSide;
use crate::ids::{CorpId, InstrumentId, OrderId, SkuId};
use oligopole_ledger::{EntityId, Money};
use serde::{Deserialize, Serialize};

/// Discrete pricing stance selected on a Slow Tick (`Clef-Flash` `choice` or `System0Heuristic`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PricingStance {
    /// Cut selling price by 5% to capture market share and clear inventory.
    AggressiveUndercut,
    /// Hold or align selling price with market reference/competitor average.
    MatchMarket,
    /// Raise selling price by 5% to expand gross margins.
    PremiumMargin,
    /// Raise selling price by 10% when inventory is scarce.
    ScarcitySkimming,
}

/// Discrete wage/labor stance selected on a Slow Tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WageStance {
    Cut5Pct,
    HoldSteady,
    Raise5Pct,
}

/// Discrete dividend payout ratio policy selected on a Slow Tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DividendStance {
    RetainAll,
    Low20Pct,
    Moderate40Pct,
    High70Pct,
}

impl DividendStance {
    #[inline]
    pub const fn payout_bps(self) -> u32 {
        match self {
            Self::RetainAll => 0,
            Self::Low20Pct => 2_000,
            Self::Moderate40Pct => 4_000,
            Self::High70Pct => 7_000,
        }
    }
}

/// Pre-bundled composite capital allocation & financing choice evaluated by `Clef-Flash`
/// so Question A (`CapEx`) and Question B (Financing) never conflict in a single parallel pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JointCapitalAction {
    /// Preserve cash and pay down up to `repay_amount` of outstanding bank debt.
    HoldAndDelever { repay_amount: Money },
    /// Expand daily production/retail capacity by `added_capacity` funded 100% from internal cash.
    ExpandCapacityCashFunded {
        added_capacity: u64,
        capex_cost: Money,
        estimated_npv: Money,
    },
    /// Expand daily production/retail capacity by `added_capacity` funded via an endogenous commercial bank loan.
    ExpandCapacityDebtFunded {
        added_capacity: u64,
        capex_cost: Money,
        loan_principal: Money,
        estimated_npv: Money,
    },
    /// Invest in R&D and Marketing to boost `quality_score_bps` and `brand_score_bps`.
    BrandAndQualityCampaign {
        campaign_cost: Money,
        quality_boost_bps: u32,
        brand_boost_bps: u32,
        estimated_npv: Money,
    },
    /// Sign a bilateral B2B supply contract with a supplier.
    SignB2BContract {
        supplier_corp_id: CorpId,
        sku_id: SkuId,
        daily_qty: u64,
        unit_price: Money,
        duration_days: u32,
        estimated_npv: Money,
    },
    /// Execute a corporate share buyback on the secondary Equity CLOB to retire shares.
    RepurchaseShares {
        buyback_budget: Money,
        max_limit_price: Money,
    },
    /// Issue new primary shares on the secondary Equity CLOB to raise equity capital.
    IssueEquityShares {
        shares_to_issue: u64,
        min_ask_price: Money,
    },
}

/// Complete monthly Slow-Tick strategic directive package for a single `CorpId`,
/// reconciled by the deterministic Rust CFO constraint solver before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategicDirective {
    pub corp_id: CorpId,
    pub capital_action: JointCapitalAction,
    pub pricing_stance: PricingStance,
    pub wage_stance: WageStance,
    pub dividend_stance: DividendStance,
    pub confidence_score: u8,
}

/// Real-time command issued by the Player from the 120 FPS `egui` Bloomberg Terminal (`oligopole-ui`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerCommand {
    /// Submit a limit order on a Commodity or Equity CLOB.
    SubmitClobOrder {
        trader_entity_id: EntityId,
        instrument_id: InstrumentId,
        side: OrderSide,
        limit_price: Money,
        qty: u64,
    },
    /// Cancel a resting CLOB order and release its collateral reservation.
    CancelClobOrder {
        instrument_id: InstrumentId,
        order_id: OrderId,
    },
    /// Directly set a corporation's posted selling price (`IRON` ask, wholesale price, or retail shelf price).
    SetCorporationPrice {
        corp_id: CorpId,
        new_price: Money,
    },
    /// Directly set a corporation's maximum input procurement bid price.
    SetMaxInputBidPrice {
        corp_id: CorpId,
        new_max_bid: Money,
    },
    /// Draw an endogenous commercial bank loan for a corporation.
    RequestBankLoan {
        corp_id: CorpId,
        principal: Money,
    },
    /// Repay commercial bank loan principal from unreserved cash.
    RepayBankLoan {
        corp_id: CorpId,
        amount: Money,
    },
    /// Execute an immediate capacity expansion for `corp_id`.
    ExpandFacilityCapacity {
        corp_id: CorpId,
        added_capacity: u64,
        capex_cost: Money,
        debt_funded: bool,
    },
    /// Toggle AI COO/CFO delegation switches on a corporation.
    UpdateAiDelegation {
        corp_id: CorpId,
        delegate_pricing: bool,
        delegate_capex: bool,
        delegate_procurement: bool,
    },
    /// Apply a full Slow-Tick `StrategicDirective` (from `Clef-Flash` or `System0Heuristic`).
    ApplyStrategicDirective(StrategicDirective),
}
