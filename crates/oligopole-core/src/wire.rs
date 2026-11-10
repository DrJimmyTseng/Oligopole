use crate::clob::{OrderBookDepthLevel, TradeFill};
use crate::command::PlayerCommand;
use crate::ids::{ContractId, CorpId, InstrumentId, ShortLoanId, SkuId, WireMessageId};
use crate::snapshot::WorldView;
use oligopole_ledger::{EntityId, Money, RoundingReserve};
use serde::{Deserialize, Serialize};

/// Maximum resolved (`Accepted`/`Rejected`/`Expired`) wire messages retained per corporation
/// in `WorldState` so `postcard` + `blake3` snapshot hashing remains $O(1)$ bounded.
pub const MAX_RESOLVED_WIRE_MESSAGES_PER_CORP: usize = 64;

/// Per-entity `ServiceCredit` (`PLEX` equivalent) unit balance and CLOB Ask reservation tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServiceCreditPosition {
    pub owned_credits: u64,
    pub reserved_credits: u64,
}

impl ServiceCreditPosition {
    #[inline]
    pub const fn available_credits(self) -> u64 {
        self.owned_credits.saturating_sub(self.reserved_credits)
    }
}

/// Physical share-borrow short-selling contract tracked in `WorldState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortPositionRecord {
    pub short_loan_id: ShortLoanId,
    pub borrower_entity_id: EntityId,
    pub lender_entity_id: EntityId,
    pub corp_id: CorpId,
    pub borrowed_shares: u64,
    pub collateral_reserved_cash: Money,
    pub annual_borrow_fee_bps: u32,
    pub fee_reserve: RoundingReserve,
}

/// Organised crime vectors resolved deterministically via `ChaCha8Rng(world_seed, tick, seq_no)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrimeOperationKind {
    /// Intercepts up to 30% of an unshielded `B2BSupplyContract` shipment's `WacInventoryLot` units.
    HijackCargo { contract_id: ContractId },
    /// Coerces an unshielded retail SME to transfer `demand_amount` of `CashAndDeposits` into `IllicitCash`.
    ExtortRetailer { demand_amount: Money },
    /// Degrades target's `daily_capacity` by 40% for 15 ticks unless protected by Tier 3 Cyber-Firewall.
    CyberRansomware { ransom_demand: Money },
    /// Grants an active intelligence wiretap lease unmasking the target's private `MSG` wires & `IllicitCash`.
    CorporateWiretap { duration_ticks: u64 },
}

/// 1-click executable smart contract attachment embedded in a Corporate Wire (`MSG`) message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireAttachment {
    /// Bilateral B2B supply contract proposal (`Accept` creates a `B2BSupplyContract`).
    B2BContractOffer {
        sku_id: SkuId,
        daily_qty: u64,
        unit_price: Money,
        duration_days: u32,
    },
    /// Over-the-Counter (OTC) Delivery-versus-Payment (`DvP`) block trade offer (`Accept` settles atomically).
    OtcBlockTradeOffer {
        instrument_id: InstrumentId,
        qty: u64,
        unit_price: Money,
    },
    /// Syndicate protection-racket demand (`Accept` pays `demand_amount` into sender's `IllicitCash` for immunity).
    SyndicateExtortionDemand {
        demand_amount: Money,
        immunity_days: u32,
    },
    /// Logistics freight or Private Security retainer contract (`Accept` activates shield & pays fee).
    ServiceRetainerOffer {
        is_security: bool,
        daily_fee: Money,
        duration_days: u32,
    },
}

/// Lifecycle state of a `WireAttachment` inside `WorldState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WireAttachmentStatus {
    NoAttachment,
    Pending,
    Accepted,
    Rejected,
    Expired,
}

/// Encrypted Corp-to-Corp / Player-to-Player wire message stored in `WorldState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMessage {
    pub message_id: WireMessageId,
    pub tick: u64,
    pub sender_corp_id: CorpId,
    pub recipient_corp_id: CorpId,
    pub subject: String,
    pub body: String,
    pub attachment: Option<WireAttachment>,
    pub status: WireAttachmentStatus,
}

/// Lightweight real-time CLOB delta pushed immediately upon intraday order submission/cancellation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClobDeltaEvent {
    pub tick: u64,
    pub seq_no: u32,
    pub instrument_id: InstrumentId,
    pub ticker: String,
    pub last_price: Money,
    pub best_bid: Option<Money>,
    pub best_ask: Option<Money>,
    pub top_bids: Vec<OrderBookDepthLevel>,
    pub top_asks: Vec<OrderBookDepthLevel>,
    pub recent_fills: Vec<TradeFill>,
}

/// Client-to-Server WebSocket frame (`rmp-serde` `msgpack`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientWireFrame {
    Authenticate { token: String },
    SubmitCommand(PlayerCommand),
    RequestCheckpointSave,
    Ping { client_ts_ms: u64 },
}

/// Server-to-Client WebSocket frame (`rmp-serde` `msgpack`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerWireFrame {
    Authenticated {
        assigned_corp_id: Option<CorpId>,
        server_motd: String,
    },
    ClobDelta(ClobDeltaEvent),
    WorldViewUpdate(Box<WorldView>),
    CommandRejected {
        reason: String,
    },
    CheckpointSaved {
        tick: u64,
        blake3_hex: String,
    },
    Pong {
        client_ts_ms: u64,
        server_tick: u64,
    },
}
