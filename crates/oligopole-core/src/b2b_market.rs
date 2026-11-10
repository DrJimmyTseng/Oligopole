use crate::ids::{ContractId, CorpId, SkuId};
use oligopole_ledger::Money;
use serde::{Deserialize, Serialize};

/// Long-term bilateral B2B supply contract negotiated on a Slow Tick or via `PlayerCommand`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct B2BSupplyContract {
    pub contract_id: ContractId,
    pub supplier_corp_id: CorpId,
    pub buyer_corp_id: CorpId,
    pub sku_id: SkuId,
    pub daily_qty: u64,
    pub unit_price: Money,
    pub remaining_days: u32,
}

/// Spot wholesale offer posted by a manufacturer for retailers or downstream factories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WholesaleSpotOffer {
    pub supplier_corp_id: CorpId,
    pub sku_id: SkuId,
    pub unit_price: Money,
    pub available_qty: u64,
    pub quality_score_bps: u32,
}
