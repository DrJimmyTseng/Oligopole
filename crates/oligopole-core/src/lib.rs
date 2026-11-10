//! `oligopole-core`: Deterministic 30/360 Fast-Tick simulation engine, stratified 3-tier
//! market clearing (Price-Time Priority CLOB with reservation escrow, B2B supply contracts,
//! and B2C Stone-Geary + scaling-and-squaring Padé `(2,2)` MNL retail), WAC production DAGs,
//! deterministic CFO constraint solver, and Vertical Slice 0.

pub mod b2b_market;
pub mod calendar;
pub mod cfo;
pub mod clob;
pub mod command;
pub mod error;
pub mod household;
pub mod ids;
pub mod production;
pub mod retail_market;
pub mod snapshot;
pub mod vertical_slice;
pub mod wire;
pub mod world;

pub use b2b_market::{B2BSupplyContract, WholesaleSpotOffer};
pub use calendar::{DAYS_PER_MONTH, DAYS_PER_QUARTER, DAYS_PER_YEAR, DailySubPhase, Date360, Tick};
pub use cfo::{apply_pricing_stance, apply_wage_stance, reconcile_strategic_directive};
pub use clob::{
    CentralLimitOrderBook, InstrumentKind, LimitOrder, OhlcvBar, OrderBookDepthLevel, OrderSide,
    TradeFill,
};
pub use command::{
    DividendStance, JointCapitalAction, PlayerCommand, PricingStance, StrategicDirective,
    WageStance,
};
pub use error::CoreError;
pub use household::{
    HouseholdCohort, HouseholdCohortKind, RetailStoreOffer, decimal_bytes_serde, pade_exp_neg,
};
pub use ids::{CohortId, ContractId, CorpId, InstrumentId, OrderId, ShortLoanId, SkuId, WireMessageId};
pub use production::{
    AiDelegationConfig, CloudTier, Corporation, CorporationRole, ExecutivePersonaKind,
    SkuDefinition,
};
pub use retail_market::{RetailSaleSettlement, compute_retail_clearing_plan};
pub use snapshot::{CohortSnapshot, CorporationSnapshot, InstrumentSnapshot, WorldView};
pub use vertical_slice::{VerticalSliceHandles, build_vertical_slice_0};
pub use wire::{
    ClientWireFrame, ClobDeltaEvent, CrimeOperationKind, MAX_RESOLVED_WIRE_MESSAGES_PER_CORP,
    ServerWireFrame, ServiceCreditPosition, ShortPositionRecord, WireAttachment,
    WireAttachmentStatus, WireMessage,
};
pub use world::{EquityPosition, WorldState};
