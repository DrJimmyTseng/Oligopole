use crate::clob::OrderSide;
use crate::ids::{CorpId, InstrumentId, OrderId, ShortLoanId, SkuId, WireMessageId};
use crate::production::{CloudTier, CorporationRole};
use crate::wire::{CrimeOperationKind, WireAttachment};
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
    /// Execute a plant maintenance & R&D overhaul (`DepreciationExpense`), restoring `facility_condition_bps`
    /// and boosting `quality_score_bps`.
    MaintenanceAndRdOverhaul {
        overhaul_cost: Money,
        condition_restore_bps: u32,
        quality_boost_bps: u32,
        estimated_npv: Money,
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
    /// Authoritative payment webhook minting `ServiceCredit` (`PLEX`) units idempotently (`zero M1 impact`).
    AuthorityMintServiceCredit {
        recipient_entity_id: EntityId,
        credits: u64,
        webhook_ref: String,
    },
    /// Ingest an external delayed/live market oracle price tick on a `DepositaryReceipt` instrument,
    /// re-anchoring its LULD $\pm 10\%$ circuit-breaker reference price.
    IngestOraclePriceTick {
        instrument_id: InstrumentId,
        new_oracle_price: Money,
    },
    /// Incorporate a new private Brick-and-Mortar SME (`equity_instrument_id: None`) funded by `founder_entity_id`.
    FoundPrivateSme {
        founder_entity_id: EntityId,
        ticker: String,
        name: String,
        role: CorporationRole,
        seed_capital: Money,
    },
    /// Execute a programmatic GAAP IPO auction to take a private SME public on the Domestic Equity Board.
    LaunchIpoAuction {
        corp_id: CorpId,
        primary_shares_to_issue: u64,
        offer_price: Money,
    },
    /// Borrow physical `u64` shares from the `CapitalistsExecutives` institutional lending pool and sell short.
    BorrowSharesToShort {
        borrower_entity_id: EntityId,
        corp_id: CorpId,
        shares: u64,
        sell_limit_price: Money,
    },
    /// Cover and close an open `ShortPositionRecord` by buying back shares and releasing remaining margin collateral.
    CoverShortPosition {
        short_loan_id: ShortLoanId,
        max_buy_price: Money,
    },
    /// Execute a deterministic closed-loop organised crime operation (`HijackCargo`, `ExtortRetailer`, `CyberRansomware`, `CorporateWiretap`).
    ExecuteCrimeOperation {
        actor_corp_id: CorpId,
        target_corp_id: CorpId,
        kind: CrimeOperationKind,
    },
    /// Launder `amount` of `IllicitCash` into `CashAndDeposits` through a retail SME (`500 bps` tax + forensic audit check).
    LaunderIllicitCash {
        corp_id: CorpId,
        amount: Money,
    },
    /// Configure a corporation's monthly `SovereignCloud` Web Services subscription tier.
    SetCloudSubscriptionTier {
        corp_id: CorpId,
        tier: CloudTier,
    },
    /// Send an encrypted Corporate Wire (`MSG`) message with an optional 1-click `WireAttachment`.
    SendWireMessage {
        sender_corp_id: CorpId,
        recipient_corp_id: CorpId,
        subject: String,
        body: String,
        attachment: Option<WireAttachment>,
    },
    /// Accept or reject a pending `WireAttachment` on an incoming Corporate Wire (`MSG`) message.
    RespondWireAttachment {
        responder_corp_id: CorpId,
        message_id: WireMessageId,
        accept: bool,
    },
    /// Execute an immediate plant maintenance & R&D overhaul (`$18,000` cash -> `WageEarners` & `ManagersSkilled`,
    /// restoring `+1,500 bps` `facility_condition_bps` and `+200 bps` `quality_score_bps`).
    ExecuteFacilityMaintenance {
        corp_id: CorpId,
    },
}
