use crate::b2b_market::{B2BSupplyContract, WholesaleSpotOffer};
use crate::calendar::Tick;
use crate::cfo::{apply_pricing_stance, apply_wage_stance, reconcile_strategic_directive};
use crate::clob::{CentralLimitOrderBook, InstrumentKind, LimitOrder, OrderSide, TradeFill};
use crate::command::{JointCapitalAction, PlayerCommand, StrategicDirective};
use crate::error::CoreError;
use crate::household::{HouseholdCohort, HouseholdCohortKind, RetailStoreOffer};
use crate::ids::{CohortId, ContractId, CorpId, InstrumentId, OrderId, SkuId};
use crate::production::{Corporation, CorporationRole, SkuDefinition};
use crate::retail_market::compute_retail_clearing_plan;
use crate::snapshot::{CohortSnapshot, CorporationSnapshot, InstrumentSnapshot, WorldView};
use oligopole_ledger::{
    BPS_DENOMINATOR, CommercialBankFacility, EntityId, FinancialStatementReport, JournalEntry,
    LedgerEngine, LedgerError, Money, StandardAccountCode, allocate_hamilton,
};
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::BTreeMap;

/// Per-shareholder position in a corporation's common stock, tracking open Equity CLOB Ask reservations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EquityPosition {
    pub owned_shares: u64,
    pub reserved_shares: u64,
}

impl EquityPosition {
    #[inline]
    pub const fn available_shares(self) -> u64 {
        self.owned_shares.saturating_sub(self.reserved_shares)
    }
}

/// Complete deterministic simulation state aggregate root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldState {
    pub world_seed: [u8; 32],
    pub current_tick: Tick,
    pub occ_version: u64,
    pub next_order_seq: u64,
    pub next_contract_seq: u64,
    pub ledger: LedgerEngine,
    pub bank: CommercialBankFacility,
    pub skus: SlotMap<SkuId, SkuDefinition>,
    pub corporations: SlotMap<CorpId, Corporation>,
    pub cohorts: SlotMap<CohortId, HouseholdCohort>,
    pub order_books: SlotMap<InstrumentId, CentralLimitOrderBook>,
    pub b2b_contracts: Vec<B2BSupplyContract>,
    /// Keyed by `(CorpId, Shareholder EntityId)` for canonical iteration and dividend splits.
    pub equity_positions: BTreeMap<(CorpId, EntityId), EquityHoldingRecord>,
}

/// Serializable wrapper for `EquityPosition`.
pub type EquityHoldingRecord = EquityPosition;

impl WorldState {
    pub fn new(world_seed: [u8; 32], loan_rate_bps: u32, deposit_rate_bps: u32) -> Self {
        let mut ledger = LedgerEngine::new();
        let bank_id = ledger.register_entity("Central & Commercial Bank", true);
        let bank = CommercialBankFacility::new(bank_id, loan_rate_bps, deposit_rate_bps);

        Self {
            world_seed,
            current_tick: Tick::ZERO,
            occ_version: 1,
            next_order_seq: 1,
            next_contract_seq: 1,
            ledger,
            bank,
            skus: SlotMap::with_key(),
            corporations: SlotMap::with_key(),
            cohorts: SlotMap::with_key(),
            order_books: SlotMap::with_key(),
            b2b_contracts: Vec::new(),
            equity_positions: BTreeMap::new(),
        }
    }

    pub fn find_corp_by_entity(&self, entity_id: EntityId) -> Option<CorpId> {
        self.corporations
            .iter()
            .find(|(_, c)| c.entity_id == entity_id)
            .map(|(id, _)| id)
    }

    /// Executes a single `PlayerCommand` (or Slow-Tick `StrategicDirective`) deterministically.
    pub fn execute_command(&mut self, cmd: PlayerCommand) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        match cmd {
            PlayerCommand::SubmitClobOrder {
                trader_entity_id,
                instrument_id,
                side,
                limit_price,
                qty,
            } => {
                self.submit_clob_limit_order(
                    trader_entity_id,
                    instrument_id,
                    side,
                    limit_price,
                    qty,
                )?;
            }
            PlayerCommand::CancelClobOrder {
                instrument_id,
                order_id,
            } => {
                self.cancel_clob_limit_order(instrument_id, order_id)?;
            }
            PlayerCommand::SetCorporationPrice { corp_id, new_price } => {
                if !new_price.is_positive() {
                    return Err(CoreError::InvalidOrder {
                        reason: "price must be strictly positive".to_owned(),
                    });
                }
                let corp = self
                    .corporations
                    .get_mut(corp_id)
                    .ok_or(CoreError::UnknownCorporation(corp_id))?;
                corp.posted_selling_price = new_price;
            }
            PlayerCommand::SetMaxInputBidPrice {
                corp_id,
                new_max_bid,
            } => {
                if !new_max_bid.is_positive() {
                    return Err(CoreError::InvalidOrder {
                        reason: "max bid price must be strictly positive".to_owned(),
                    });
                }
                let corp = self
                    .corporations
                    .get_mut(corp_id)
                    .ok_or(CoreError::UnknownCorporation(corp_id))?;
                corp.max_input_bid_price = new_max_bid;
            }
            PlayerCommand::RequestBankLoan { corp_id, principal } => {
                let entity_id = self
                    .corporations
                    .get(corp_id)
                    .ok_or(CoreError::UnknownCorporation(corp_id))?
                    .entity_id;
                self.bank
                    .originate_loan(&mut self.ledger, tick, entity_id, principal, None)?;
            }
            PlayerCommand::RepayBankLoan { corp_id, amount } => {
                let entity_id = self
                    .corporations
                    .get(corp_id)
                    .ok_or(CoreError::UnknownCorporation(corp_id))?
                    .entity_id;
                let avail = self.ledger.available_cash(entity_id)?;
                let pay = amount.min(avail);
                if pay.is_positive() {
                    self.bank
                        .repay_loan_principal(&mut self.ledger, tick, entity_id, pay)?;
                }
            }
            PlayerCommand::ExpandFacilityCapacity {
                corp_id,
                added_capacity,
                capex_cost,
                debt_funded,
            } => {
                self.execute_capacity_expansion(
                    corp_id,
                    added_capacity,
                    capex_cost,
                    if debt_funded { capex_cost } else { Money::ZERO },
                )?;
            }
            PlayerCommand::UpdateAiDelegation {
                corp_id,
                delegate_pricing,
                delegate_capex,
                delegate_procurement,
            } => {
                let corp = self
                    .corporations
                    .get_mut(corp_id)
                    .ok_or(CoreError::UnknownCorporation(corp_id))?;
                corp.delegation.delegate_pricing_to_ai = delegate_pricing;
                corp.delegation.delegate_capex_and_financing_to_ai = delegate_capex;
                corp.delegation.delegate_procurement_to_ai = delegate_procurement;
            }
            PlayerCommand::ApplyStrategicDirective(directive) => {
                self.apply_reconciled_directive(directive)?;
            }
        }
        Ok(())
    }

    /// Reconciles a `StrategicDirective` through the Rust CFO guardrail and applies its actions.
    pub fn apply_reconciled_directive(
        &mut self,
        raw_directive: StrategicDirective,
    ) -> Result<StrategicDirective, CoreError> {
        let tick = self.current_tick.0;
        let corp_id = raw_directive.corp_id;
        let (entity_id, avail_cash, debt, reconciled) = {
            let corp = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            let avail_cash = self.ledger.available_cash(corp.entity_id)?;
            let debt = self
                .bank
                .loans
                .get(&corp.entity_id)
                .map_or(Money::ZERO, |l| l.principal_outstanding);
            let reconciled =
                reconcile_strategic_directive(corp, avail_cash, debt, raw_directive)?;
            (corp.entity_id, avail_cash, debt, reconciled)
        };
        let _ = (avail_cash, debt);

        // 1. Execute JointCapitalAction
        match &reconciled.capital_action {
            JointCapitalAction::HoldAndDelever { repay_amount } => {
                if repay_amount.is_positive() {
                    self.bank.repay_loan_principal(
                        &mut self.ledger,
                        tick,
                        entity_id,
                        *repay_amount,
                    )?;
                }
            }
            JointCapitalAction::ExpandCapacityCashFunded {
                added_capacity,
                capex_cost,
                ..
            } => {
                self.execute_capacity_expansion(
                    corp_id,
                    *added_capacity,
                    *capex_cost,
                    Money::ZERO,
                )?;
            }
            JointCapitalAction::ExpandCapacityDebtFunded {
                added_capacity,
                capex_cost,
                loan_principal,
                ..
            } => {
                self.execute_capacity_expansion(
                    corp_id,
                    *added_capacity,
                    *capex_cost,
                    *loan_principal,
                )?;
            }
            JointCapitalAction::BrandAndQualityCampaign {
                campaign_cost,
                quality_boost_bps,
                brand_boost_bps,
                ..
            } => {
                self.execute_marketing_campaign(
                    corp_id,
                    *campaign_cost,
                    *quality_boost_bps,
                    *brand_boost_bps,
                )?;
            }
            JointCapitalAction::SignB2BContract {
                supplier_corp_id,
                sku_id,
                daily_qty,
                unit_price,
                duration_days,
                ..
            } => {
                if self.corporations.contains_key(*supplier_corp_id)
                    && *daily_qty > 0
                    && unit_price.is_positive()
                    && *duration_days > 0
                {
                    let contract_id = ContractId(self.next_contract_seq);
                    self.next_contract_seq = self.next_contract_seq.saturating_add(1);
                    self.b2b_contracts.push(B2BSupplyContract {
                        contract_id,
                        supplier_corp_id: *supplier_corp_id,
                        buyer_corp_id: corp_id,
                        sku_id: *sku_id,
                        daily_qty: *daily_qty,
                        unit_price: *unit_price,
                        remaining_days: *duration_days,
                    });
                }
            }
        }

        // 2. Apply pricing, wage, and dividend policy dials
        if let Some(corp) = self.corporations.get_mut(corp_id) {
            if !corp.delegation.is_player_controlled || corp.delegation.delegate_pricing_to_ai {
                corp.posted_selling_price =
                    apply_pricing_stance(corp.posted_selling_price, reconciled.pricing_stance)?;
            }
            if !corp.delegation.is_player_controlled
                || corp.delegation.delegate_capex_and_financing_to_ai
            {
                corp.daily_wage_bill =
                    apply_wage_stance(corp.daily_wage_bill, reconciled.wage_stance)?;
                corp.target_dividend_payout_bps = reconciled.dividend_stance.payout_bps();
            }
        }

        Ok(reconciled)
    }

    fn execute_capacity_expansion(
        &mut self,
        corp_id: CorpId,
        added_capacity: u64,
        capex_cost: Money,
        loan_principal: Money,
    ) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        let entity_id = self
            .corporations
            .get(corp_id)
            .ok_or(CoreError::UnknownCorporation(corp_id))?
            .entity_id;

        if loan_principal.is_positive() {
            self.bank
                .originate_loan(&mut self.ledger, tick, entity_id, loan_principal, None)?;
        }

        let avail = self.ledger.available_cash(entity_id)?;
        if capex_cost.is_positive() && avail >= capex_cost {
            // Pay CapEx cost: 50% capitalized as PPE transfer to WageEarners (construction labor),
            // 50% to ManagersSkilled (engineering). On the firm's balance sheet, Cash converts into PPE (Asset)!
            if let Some((wage_ent, mgr_ent)) = self.wage_and_manager_entities() {
                let wage_half = Money::from_cents(capex_cost.cents() / 2);
                let mgr_half = capex_cost.checked_sub(wage_half)?;

                let firm_ppe = self.ledger.entity_account_id(
                    entity_id,
                    StandardAccountCode::PropertyPlantEquipment,
                )?;
                let firm_cash = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
                let w_cash = self
                    .ledger
                    .entity_account_id(wage_ent, StandardAccountCode::CashAndDeposits)?;
                let w_rev = self
                    .ledger
                    .entity_account_id(wage_ent, StandardAccountCode::SalesRevenue)?;
                let m_cash = self
                    .ledger
                    .entity_account_id(mgr_ent, StandardAccountCode::CashAndDeposits)?;
                let m_rev = self
                    .ledger
                    .entity_account_id(mgr_ent, StandardAccountCode::SalesRevenue)?;

                let mut entry = JournalEntry::new(tick, "Facility CapEx Construction")
                    .debit(firm_ppe, capex_cost)
                    .credit(firm_cash, capex_cost);
                if wage_half.is_positive() {
                    entry = entry.debit(w_cash, wage_half).credit(w_rev, wage_half);
                }
                if mgr_half.is_positive() {
                    entry = entry.debit(m_cash, mgr_half).credit(m_rev, mgr_half);
                }
                self.ledger.post_entry(&entry)?;
            }

            if let Some(corp) = self.corporations.get_mut(corp_id) {
                corp.daily_capacity = corp.daily_capacity.saturating_add(added_capacity);
            }
        }
        Ok(())
    }

    fn execute_marketing_campaign(
        &mut self,
        corp_id: CorpId,
        campaign_cost: Money,
        quality_boost_bps: u32,
        brand_boost_bps: u32,
    ) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        let entity_id = self
            .corporations
            .get(corp_id)
            .ok_or(CoreError::UnknownCorporation(corp_id))?
            .entity_id;

        let avail = self.ledger.available_cash(entity_id)?;
        if campaign_cost.is_positive() && avail >= campaign_cost {
            if let Some((_, mgr_ent)) = self.wage_and_manager_entities() {
                let firm_mkt = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::MarketingExpense)?;
                let firm_cash = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
                let m_cash = self
                    .ledger
                    .entity_account_id(mgr_ent, StandardAccountCode::CashAndDeposits)?;
                let m_rev = self
                    .ledger
                    .entity_account_id(mgr_ent, StandardAccountCode::SalesRevenue)?;

                let entry = JournalEntry::new(tick, "Brand & Quality R&D Campaign")
                    .debit(firm_mkt, campaign_cost)
                    .credit(firm_cash, campaign_cost)
                    .debit(m_cash, campaign_cost)
                    .credit(m_rev, campaign_cost);
                self.ledger.post_entry(&entry)?;
            }

            if let Some(corp) = self.corporations.get_mut(corp_id) {
                corp.quality_score_bps = corp
                    .quality_score_bps
                    .saturating_add(quality_boost_bps)
                    .min(20_000);
                corp.brand_score_bps = corp
                    .brand_score_bps
                    .saturating_add(brand_boost_bps)
                    .min(20_000);
            }
        }
        Ok(())
    }

    /// Reserves collateral (`reserved_cash`, `reserved_qty`, or `reserved_shares`), submits a limit order
    /// to `instrument_id`, and immediately settles any matched `TradeFill`s.
    pub fn submit_clob_limit_order(
        &mut self,
        trader_entity_id: EntityId,
        instrument_id: InstrumentId,
        side: OrderSide,
        limit_price: Money,
        qty: u64,
    ) -> Result<(OrderId, Vec<TradeFill>), CoreError> {
        if qty == 0 || !limit_price.is_positive() {
            return Err(CoreError::InvalidOrder {
                reason: "quantity and limit_price must be strictly positive".to_owned(),
            });
        }

        let kind = self
            .order_books
            .get(instrument_id)
            .ok_or(CoreError::UnknownInstrument(instrument_id))?
            .kind;

        // 1. Reserve collateral prior to order insertion
        match side {
            OrderSide::Bid => {
                let notional = limit_price.checked_mul_u64(qty)?;
                self.ledger.reserve_cash(trader_entity_id, notional)?;
            }
            OrderSide::Ask => match kind {
                InstrumentKind::Commodity { sku_id } => {
                    let corp_id = self.find_corp_by_entity(trader_entity_id).ok_or_else(|| {
                        CoreError::InvalidOrder {
                            reason: "only corporations with WAC lots can post commodity asks"
                                .to_owned(),
                        }
                    })?;
                    let corp = self
                        .corporations
                        .get_mut(corp_id)
                        .ok_or(CoreError::UnknownCorporation(corp_id))?;
                    corp.inventory_lot_mut(sku_id).reserve_units(qty)?;
                }
                InstrumentKind::Equity { corp_id } => {
                    let pos = self
                        .equity_positions
                        .entry((corp_id, trader_entity_id))
                        .or_default();
                    let avail = pos.available_shares();
                    if qty > avail {
                        return Err(CoreError::InsufficientShares {
                            corp_id,
                            available: avail,
                            requested: qty,
                        });
                    }
                    pos.reserved_shares = pos.reserved_shares.saturating_add(qty);
                }
            },
        }

        let order_id = OrderId(self.next_order_seq);
        self.next_order_seq = self.next_order_seq.saturating_add(1);

        let order = LimitOrder {
            order_id,
            instrument_id,
            trader_entity_id,
            side,
            limit_price,
            initial_qty: qty,
            remaining_qty: qty,
            submitted_tick: self.current_tick.0,
        };

        let fills = self
            .order_books
            .get_mut(instrument_id)
            .ok_or(CoreError::UnknownInstrument(instrument_id))?
            .submit_and_match(order)?;

        for fill in &fills {
            self.settle_clob_fill(kind, fill)?;
        }

        Ok((order_id, fills))
    }

    /// Cancels a resting CLOB order and releases its remaining collateral reservation.
    pub fn cancel_clob_limit_order(
        &mut self,
        instrument_id: InstrumentId,
        order_id: OrderId,
    ) -> Result<LimitOrder, CoreError> {
        let (kind, removed) = {
            let book = self
                .order_books
                .get_mut(instrument_id)
                .ok_or(CoreError::UnknownInstrument(instrument_id))?;
            let kind = book.kind;
            let removed = book.cancel_order(order_id)?;
            (kind, removed)
        };
        self.release_order_collateral(kind, &removed)?;
        Ok(removed)
    }

    fn cancel_all_orders_for_entity_on_book(
        &mut self,
        instrument_id: InstrumentId,
        trader_entity_id: EntityId,
    ) -> Result<(), CoreError> {
        let (kind, cancelled) = {
            let book = self
                .order_books
                .get_mut(instrument_id)
                .ok_or(CoreError::UnknownInstrument(instrument_id))?;
            (book.kind, book.cancel_all_for_entity(trader_entity_id))
        };
        for order in &cancelled {
            self.release_order_collateral(kind, order)?;
        }
        Ok(())
    }

    fn release_order_collateral(
        &mut self,
        kind: InstrumentKind,
        order: &LimitOrder,
    ) -> Result<(), CoreError> {
        if order.remaining_qty == 0 {
            return Ok(());
        }
        match order.side {
            OrderSide::Bid => {
                let locked_cash = order.limit_price.checked_mul_u64(order.remaining_qty)?;
                self.ledger
                    .release_reserved_cash(order.trader_entity_id, locked_cash)?;
            }
            OrderSide::Ask => match kind {
                InstrumentKind::Commodity { sku_id } => {
                    if let Some(corp_id) = self.find_corp_by_entity(order.trader_entity_id) {
                        if let Some(corp) = self.corporations.get_mut(corp_id) {
                            corp.inventory_lot_mut(sku_id)
                                .release_reserved_units(order.remaining_qty)?;
                        }
                    }
                }
                InstrumentKind::Equity { corp_id } => {
                    if let Some(pos) = self
                        .equity_positions
                        .get_mut(&(corp_id, order.trader_entity_id))
                    {
                        pos.reserved_shares =
                            pos.reserved_shares.saturating_sub(order.remaining_qty);
                    }
                }
            },
        }
        Ok(())
    }

    fn settle_clob_fill(&mut self, kind: InstrumentKind, fill: &TradeFill) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        let reserved_cash_to_release = fill.buyer_limit_price.checked_mul_u64(fill.qty)?;
        self.ledger
            .release_reserved_cash(fill.buyer_entity_id, reserved_cash_to_release)?;

        let trade_notional = fill.execution_price.checked_mul_u64(fill.qty)?;

        match kind {
            InstrumentKind::Commodity { sku_id } => {
                let inv_code = self
                    .skus
                    .get(sku_id)
                    .ok_or(CoreError::UnknownSku(sku_id))?
                    .ledger_inventory_account;

                let seller_corp_id =
                    self.find_corp_by_entity(fill.seller_entity_id)
                        .ok_or_else(|| CoreError::InvalidOrder {
                            reason: "commodity seller is not a corporation".to_owned(),
                        })?;
                let buyer_corp_id =
                    self.find_corp_by_entity(fill.buyer_entity_id)
                        .ok_or_else(|| CoreError::InvalidOrder {
                            reason: "commodity buyer is not a corporation".to_owned(),
                        })?;

                let seller_cogs = self
                    .corporations
                    .get_mut(seller_corp_id)
                    .ok_or(CoreError::UnknownCorporation(seller_corp_id))?
                    .inventory_lot_mut(sku_id)
                    .settle_reserved_units(fill.qty)?;

                self.corporations
                    .get_mut(buyer_corp_id)
                    .ok_or(CoreError::UnknownCorporation(buyer_corp_id))?
                    .inventory_lot_mut(sku_id)
                    .record_addition(fill.qty, trade_notional)?;

                if let Some(seller_corp) = self.corporations.get_mut(seller_corp_id) {
                    seller_corp.monthly_units_sold =
                        seller_corp.monthly_units_sold.saturating_add(fill.qty);
                }

                let buyer_inv = self
                    .ledger
                    .entity_account_id(fill.buyer_entity_id, inv_code)?;
                let buyer_cash = self
                    .ledger
                    .entity_account_id(fill.buyer_entity_id, StandardAccountCode::CashAndDeposits)?;
                let seller_cash = self.ledger.entity_account_id(
                    fill.seller_entity_id,
                    StandardAccountCode::CashAndDeposits,
                )?;
                let seller_rev = self
                    .ledger
                    .entity_account_id(fill.seller_entity_id, StandardAccountCode::SalesRevenue)?;
                let seller_cogs_acc = self.ledger.entity_account_id(
                    fill.seller_entity_id,
                    StandardAccountCode::CostOfGoodsSold,
                )?;
                let seller_inv = self
                    .ledger
                    .entity_account_id(fill.seller_entity_id, inv_code)?;

                let mut entry = JournalEntry::new(tick, "Commodity CLOB Fill Settlement")
                    .debit(buyer_inv, trade_notional)
                    .credit(buyer_cash, trade_notional)
                    .debit(seller_cash, trade_notional)
                    .credit(seller_rev, trade_notional);

                if seller_cogs.is_positive() {
                    entry = entry
                        .debit(seller_cogs_acc, seller_cogs)
                        .credit(seller_inv, seller_cogs);
                }

                self.ledger.post_entry(&entry)?;
            }
            InstrumentKind::Equity { corp_id } => {
                // Secondary market equity transfer between seller and buyer:
                // Seller transfers `fill.qty` reserved shares to Buyer; Buyer transfers `trade_notional` cash to Seller
                // (recorded as CommonStock/RetainedEarnings/Cash transfer on their investment books).
                let seller_pos = self
                    .equity_positions
                    .entry((corp_id, fill.seller_entity_id))
                    .or_default();
                if fill.qty > seller_pos.reserved_shares || fill.qty > seller_pos.owned_shares {
                    return Err(CoreError::InsufficientShares {
                        corp_id,
                        available: seller_pos.reserved_shares,
                        requested: fill.qty,
                    });
                }
                seller_pos.reserved_shares -= fill.qty;
                seller_pos.owned_shares -= fill.qty;

                let buyer_pos = self
                    .equity_positions
                    .entry((corp_id, fill.buyer_entity_id))
                    .or_default();
                buyer_pos.owned_shares = buyer_pos.owned_shares.saturating_add(fill.qty);

                let buyer_eq = self
                    .ledger
                    .entity_account_id(fill.buyer_entity_id, StandardAccountCode::CommonStock)?;
                let buyer_cash = self
                    .ledger
                    .entity_account_id(fill.buyer_entity_id, StandardAccountCode::CashAndDeposits)?;
                let seller_cash = self.ledger.entity_account_id(
                    fill.seller_entity_id,
                    StandardAccountCode::CashAndDeposits,
                )?;
                let seller_eq = self
                    .ledger
                    .entity_account_id(fill.seller_entity_id, StandardAccountCode::CommonStock)?;

                let entry = JournalEntry::new(tick, "Secondary Equity CLOB Trade Settlement")
                    .debit(buyer_eq, trade_notional)
                    .credit(buyer_cash, trade_notional)
                    .debit(seller_cash, trade_notional)
                    .credit(seller_eq, trade_notional);
                self.ledger.post_entry(&entry)?;
            }
        }

        Ok(())
    }

    /// Advances the world by 1 Fast Tick (1 simulated 30/360 day), executing all 5 intraday sub-phases
    /// and asserting global double-entry trial balance and WAC inventory book-value conservation.
    pub fn step_fast_tick(&mut self, pending_commands: &[PlayerCommand]) -> Result<(), CoreError> {
        let next_tick = self.current_tick.next();
        self.current_tick = next_tick;
        self.occ_version = self.occ_version.saturating_add(1);

        if (next_tick.0 - 1).is_multiple_of(30) {
            for (_, corp) in &mut self.corporations {
                corp.monthly_units_output = 0;
                corp.monthly_units_sold = 0;
            }
        }

        // 0. Drain pending commands at start of day
        for cmd in pending_commands {
            self.execute_command(cmd.clone())?;
        }

        // Sub-Phase 1: Morning Logistics, B2B Contract Deliveries & Wholesale Replenishment
        self.run_subphase_1_b2b_and_wholesale()?;

        // Sub-Phase 2: Factory DAG Production & Resource Extraction
        self.run_subphase_2_production()?;

        // Sub-Phase 3: Commodity & Equity CLOB Matching & OHLCV Finalization
        self.run_subphase_3_clob_matching()?;

        // Sub-Phase 4: Household Stone-Geary + Padé (2,2) MNL Retail Purchasing
        self.run_subphase_4_retail_consumption()?;

        // Sub-Phase 5: Payroll, 30/360 Interest Accrual, Dividends, Quarterly Closing & Audit
        self.run_subphase_5_settlement_and_audit()?;

        Ok(())
    }

    fn run_subphase_1_b2b_and_wholesale(&mut self) -> Result<(), CoreError> {
        let tick = self.current_tick.0;

        // 1A. Deliver scheduled bilateral B2B contract tranches
        let mut active_contracts = Vec::with_capacity(self.b2b_contracts.len());
        let contracts_snapshot = std::mem::take(&mut self.b2b_contracts);

        for mut contract in contracts_snapshot {
            if contract.remaining_days == 0 {
                continue;
            }
            self.settle_b2b_transfer(
                tick,
                contract.supplier_corp_id,
                contract.buyer_corp_id,
                contract.sku_id,
                contract.daily_qty,
                contract.unit_price,
            )?;
            contract.remaining_days -= 1;
            if contract.remaining_days > 0 {
                active_contracts.push(contract);
            }
        }
        self.b2b_contracts = active_contracts;

        // 1B. Retailers procure finished goods wholesale from Manufacturers (sorted by lowest price, then CorpId)
        let retailer_ids: Vec<CorpId> = self
            .corporations
            .iter()
            .filter_map(|(id, c)| match c.role {
                CorporationRole::Retailer { .. } => Some(id),
                _ => None,
            })
            .collect();

        for ret_id in retailer_ids {
            let (retail_sku, target_stock, current_stock, max_bid) = {
                let Some(ret) = self.corporations.get(ret_id) else {
                    continue;
                };
                let CorporationRole::Retailer { retail_sku } = ret.role else {
                    continue;
                };
                let current_stock = ret.inventory_lot(retail_sku).total_qty();
                let target_stock = ret.daily_capacity.saturating_mul(3);
                (retail_sku, target_stock, current_stock, ret.max_input_bid_price)
            };

            if current_stock >= target_stock {
                continue;
            }
            let mut needed = target_stock - current_stock;

            let mut offers: Vec<WholesaleSpotOffer> = self
                .corporations
                .iter()
                .filter_map(|(sup_id, sup)| match sup.role {
                    CorporationRole::Manufacturer { output_sku, .. }
                        if output_sku == retail_sku
                            && sup.posted_selling_price <= max_bid
                            && sup.inventory_lot(output_sku).available_qty() > 0 =>
                    {
                        Some(WholesaleSpotOffer {
                            supplier_corp_id: sup_id,
                            sku_id: output_sku,
                            unit_price: sup.posted_selling_price,
                            available_qty: sup.inventory_lot(output_sku).available_qty(),
                            quality_score_bps: sup.quality_score_bps,
                        })
                    }
                    _ => None,
                })
                .collect();

            offers.sort_by_key(|o| (o.unit_price, o.supplier_corp_id));

            for offer in offers {
                if needed == 0 {
                    break;
                }
                let buy_qty = needed.min(offer.available_qty);
                let delivered = self.settle_b2b_transfer(
                    tick,
                    offer.supplier_corp_id,
                    ret_id,
                    retail_sku,
                    buy_qty,
                    offer.unit_price,
                )?;
                needed = needed.saturating_sub(delivered);
            }
        }

        Ok(())
    }

    fn settle_b2b_transfer(
        &mut self,
        tick: u64,
        supplier_id: CorpId,
        buyer_id: CorpId,
        sku_id: SkuId,
        requested_qty: u64,
        unit_price: Money,
    ) -> Result<u64, CoreError> {
        if requested_qty == 0 || !unit_price.is_positive() || supplier_id == buyer_id {
            return Ok(0);
        }

        let inv_code = self
            .skus
            .get(sku_id)
            .ok_or(CoreError::UnknownSku(sku_id))?
            .ledger_inventory_account;

        let (sup_entity, sup_avail) = {
            let sup = self
                .corporations
                .get(supplier_id)
                .ok_or(CoreError::UnknownCorporation(supplier_id))?;
            (sup.entity_id, sup.inventory_lot(sku_id).available_qty())
        };
        let buyer_entity = self
            .corporations
            .get(buyer_id)
            .ok_or(CoreError::UnknownCorporation(buyer_id))?
            .entity_id;

        let buyer_cash = self.ledger.available_cash(buyer_entity)?;
        let affordable_i128 = buyer_cash.checked_div_euclid(unit_price.cents())?.cents();
        let affordable_qty = u64::try_from(affordable_i128).unwrap_or(0);

        let actual_qty = requested_qty.min(sup_avail).min(affordable_qty);
        if actual_qty == 0 {
            return Ok(0);
        }

        let trade_notional = unit_price.checked_mul_u64(actual_qty)?;

        let sup_cogs = self
            .corporations
            .get_mut(supplier_id)
            .ok_or(CoreError::UnknownCorporation(supplier_id))?
            .inventory_lot_mut(sku_id)
            .draw_unreserved_units(actual_qty)?;

        self.corporations
            .get_mut(buyer_id)
            .ok_or(CoreError::UnknownCorporation(buyer_id))?
            .inventory_lot_mut(sku_id)
            .record_addition(actual_qty, trade_notional)?;

        if let Some(sup) = self.corporations.get_mut(supplier_id) {
            sup.monthly_units_sold = sup.monthly_units_sold.saturating_add(actual_qty);
        }
        if let Some(buyer) = self.corporations.get_mut(buyer_id) {
            buyer.monthly_units_output = buyer.monthly_units_output.saturating_add(actual_qty);
        }

        let b_inv = self.ledger.entity_account_id(buyer_entity, inv_code)?;
        let b_cash = self
            .ledger
            .entity_account_id(buyer_entity, StandardAccountCode::CashAndDeposits)?;
        let s_cash = self
            .ledger
            .entity_account_id(sup_entity, StandardAccountCode::CashAndDeposits)?;
        let s_rev = self
            .ledger
            .entity_account_id(sup_entity, StandardAccountCode::SalesRevenue)?;
        let s_cogs = self
            .ledger
            .entity_account_id(sup_entity, StandardAccountCode::CostOfGoodsSold)?;
        let s_inv = self.ledger.entity_account_id(sup_entity, inv_code)?;

        let mut entry = JournalEntry::new(tick, "B2B Wholesale Delivery Settlement")
            .debit(b_inv, trade_notional)
            .credit(b_cash, trade_notional)
            .debit(s_cash, trade_notional)
            .credit(s_rev, trade_notional);

        if sup_cogs.is_positive() {
            entry = entry.debit(s_cogs, sup_cogs).credit(s_inv, sup_cogs);
        }

        self.ledger.post_entry(&entry)?;
        Ok(actual_qty)
    }

    fn run_subphase_2_production(&mut self) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        let Some((wage_entity, _)) = self.wage_and_manager_entities() else {
            return Ok(());
        };

        let corp_ids: Vec<CorpId> = self.corporations.keys().collect();

        for corp_id in corp_ids {
            let (entity_id, role, daily_capacity, daily_wage_bill) = {
                let Some(corp) = self.corporations.get(corp_id) else {
                    continue;
                };
                (
                    corp.entity_id,
                    corp.role,
                    corp.daily_capacity,
                    corp.daily_wage_bill,
                )
            };

            match role {
                CorporationRole::ResourceExtractor { output_sku } => {
                    // Extractor capitalizes daily extraction labor directly into InventoryRaw
                    let avail_cash = self.ledger.available_cash(entity_id)?;
                    if daily_capacity > 0 && avail_cash >= daily_wage_bill && daily_wage_bill.is_positive() {
                        let inv_code = self
                            .skus
                            .get(output_sku)
                            .ok_or(CoreError::UnknownSku(output_sku))?
                            .ledger_inventory_account;
                        let f_inv = self.ledger.entity_account_id(entity_id, inv_code)?;
                        let f_cash = self
                            .ledger
                            .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
                        let w_cash = self
                            .ledger
                            .entity_account_id(wage_entity, StandardAccountCode::CashAndDeposits)?;
                        let w_rev = self
                            .ledger
                            .entity_account_id(wage_entity, StandardAccountCode::SalesRevenue)?;

                        let entry = JournalEntry::new(tick, "Resource Extraction Labor Capitalization")
                            .debit(f_inv, daily_wage_bill)
                            .credit(f_cash, daily_wage_bill)
                            .debit(w_cash, daily_wage_bill)
                            .credit(w_rev, daily_wage_bill);
                        self.ledger.post_entry(&entry)?;

                        if let Some(corp) = self.corporations.get_mut(corp_id) {
                            corp.inventory_lot_mut(output_sku)
                                .record_addition(daily_capacity, daily_wage_bill)?;
                            corp.monthly_units_output =
                                corp.monthly_units_output.saturating_add(daily_capacity);
                        }
                    }
                }
                CorporationRole::Manufacturer {
                    input_sku,
                    input_units_per_batch,
                    output_sku,
                    output_units_per_batch,
                } => {
                    if input_units_per_batch == 0 || output_units_per_batch == 0 {
                        continue;
                    }
                    let avail_input = self
                        .corporations
                        .get(corp_id)
                        .map_or(0, |c| c.inventory_lot(input_sku).available_qty());
                    let max_batches_by_input = avail_input / input_units_per_batch;
                    let batches = daily_capacity.min(max_batches_by_input);
                    let avail_cash = self.ledger.available_cash(entity_id)?;

                    if batches > 0 && avail_cash >= daily_wage_bill {
                        let input_consumed = batches.saturating_mul(input_units_per_batch);
                        let output_produced = batches.saturating_mul(output_units_per_batch);

                        let raw_material_cost = self
                            .corporations
                            .get_mut(corp_id)
                            .ok_or(CoreError::UnknownCorporation(corp_id))?
                            .inventory_lot_mut(input_sku)
                            .draw_unreserved_units(input_consumed)?;

                        let total_finished_cost =
                            raw_material_cost.checked_add(daily_wage_bill)?;

                        self.corporations
                            .get_mut(corp_id)
                            .ok_or(CoreError::UnknownCorporation(corp_id))?
                            .inventory_lot_mut(output_sku)
                            .record_addition(output_produced, total_finished_cost)?;

                        if let Some(corp) = self.corporations.get_mut(corp_id) {
                            corp.monthly_units_output =
                                corp.monthly_units_output.saturating_add(output_produced);
                        }

                        let in_code = self
                            .skus
                            .get(input_sku)
                            .ok_or(CoreError::UnknownSku(input_sku))?
                            .ledger_inventory_account;
                        let out_code = self
                            .skus
                            .get(output_sku)
                            .ok_or(CoreError::UnknownSku(output_sku))?
                            .ledger_inventory_account;

                        let f_out_inv = self.ledger.entity_account_id(entity_id, out_code)?;
                        let f_in_inv = self.ledger.entity_account_id(entity_id, in_code)?;
                        let f_cash = self
                            .ledger
                            .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
                        let w_cash = self
                            .ledger
                            .entity_account_id(wage_entity, StandardAccountCode::CashAndDeposits)?;
                        let w_rev = self
                            .ledger
                            .entity_account_id(wage_entity, StandardAccountCode::SalesRevenue)?;

                        let mut entry =
                            JournalEntry::new(tick, "Factory DAG Production Transformation")
                                .debit(f_out_inv, total_finished_cost);
                        if raw_material_cost.is_positive() {
                            entry = entry.credit(f_in_inv, raw_material_cost);
                        }
                        if daily_wage_bill.is_positive() {
                            entry = entry
                                .credit(f_cash, daily_wage_bill)
                                .debit(w_cash, daily_wage_bill)
                                .credit(w_rev, daily_wage_bill);
                        }
                        self.ledger.post_entry(&entry)?;
                    }
                }
                CorporationRole::Retailer { .. } => {}
            }
        }

        Ok(())
    }

    fn run_subphase_3_clob_matching(&mut self) -> Result<(), CoreError> {
        let commodity_books: Vec<(InstrumentId, SkuId)> = self
            .order_books
            .iter()
            .filter_map(|(inst_id, book)| match book.kind {
                InstrumentKind::Commodity { sku_id } => Some((inst_id, sku_id)),
                InstrumentKind::Equity { .. } => None,
            })
            .collect();

        let corp_ids: Vec<CorpId> = self.corporations.keys().collect();

        for (inst_id, book_sku) in commodity_books {
            // 1. Extractors refresh Commodity Ask orders
            for &corp_id in &corp_ids {
                let (entity_id, role, ask_price, avail_qty, auto_procure) = {
                    let Some(c) = self.corporations.get(corp_id) else {
                        continue;
                    };
                    (
                        c.entity_id,
                        c.role,
                        c.posted_selling_price,
                        c.inventory_lot(book_sku).available_qty(),
                        !c.delegation.is_player_controlled || c.delegation.delegate_procurement_to_ai,
                    )
                };
                if let CorporationRole::ResourceExtractor { output_sku } = role {
                    if output_sku == book_sku && auto_procure {
                        self.cancel_all_orders_for_entity_on_book(inst_id, entity_id)?;
                        let fresh_avail = self
                            .corporations
                            .get(corp_id)
                            .map_or(0, |c| c.inventory_lot(book_sku).available_qty());
                        if fresh_avail > 0 && ask_price.is_positive() {
                            self.submit_clob_limit_order(
                                entity_id,
                                inst_id,
                                OrderSide::Ask,
                                ask_price,
                                fresh_avail,
                            )?;
                        }
                    }
                }
                let _ = avail_qty;
            }

            // 2. Manufacturers refresh Commodity Bid orders
            for &corp_id in &corp_ids {
                let (entity_id, role, bid_price, daily_cap, auto_procure) = {
                    let Some(c) = self.corporations.get(corp_id) else {
                        continue;
                    };
                    (
                        c.entity_id,
                        c.role,
                        c.max_input_bid_price,
                        c.daily_capacity,
                        !c.delegation.is_player_controlled || c.delegation.delegate_procurement_to_ai,
                    )
                };
                if let CorporationRole::Manufacturer {
                    input_sku,
                    input_units_per_batch,
                    ..
                } = role
                {
                    if input_sku == book_sku && auto_procure && bid_price.is_positive() {
                        self.cancel_all_orders_for_entity_on_book(inst_id, entity_id)?;
                        let current_qty = self
                            .corporations
                            .get(corp_id)
                            .map_or(0, |c| c.inventory_lot(book_sku).total_qty());
                        let target_qty = daily_cap
                            .saturating_mul(input_units_per_batch)
                            .saturating_mul(3);
                        if current_qty < target_qty {
                            let needed = target_qty - current_qty;
                            let avail_cash = self.ledger.available_cash(entity_id)?;
                            // Keep 50% of cash for payroll/working capital when bidding
                            let bid_budget = Money::from_cents(avail_cash.cents() / 2);
                            let affordable_i128 =
                                bid_budget.checked_div_euclid(bid_price.cents())?.cents();
                            let affordable = u64::try_from(affordable_i128).unwrap_or(0);
                            let order_qty = needed.min(affordable);
                            if order_qty > 0 {
                                self.submit_clob_limit_order(
                                    entity_id,
                                    inst_id,
                                    OrderSide::Bid,
                                    bid_price,
                                    order_qty,
                                )?;
                            }
                        }
                    }
                }
            }
        }

        // Finalize daily OHLCV bar across all Commodity & Equity order books
        let tick = self.current_tick.0;
        for (_, book) in &mut self.order_books {
            book.finalize_daily_bar(tick);
        }

        Ok(())
    }

    fn run_subphase_4_retail_consumption(&mut self) -> Result<(), CoreError> {
        let tick = self.current_tick.0;

        // Group retailers by retail_sku
        let mut skus_sold: Vec<SkuId> = self
            .corporations
            .values()
            .filter_map(|c| match c.role {
                CorporationRole::Retailer { retail_sku } => Some(retail_sku),
                _ => None,
            })
            .collect();
        skus_sold.sort();
        skus_sold.dedup();

        for sku_id in skus_sold {
            let (ref_price, inv_code) = {
                let sku = self.skus.get(sku_id).ok_or(CoreError::UnknownSku(sku_id))?;
                (sku.base_reference_price, sku.ledger_inventory_account)
            };

            let offers: Vec<RetailStoreOffer> = self
                .corporations
                .iter()
                .filter_map(|(corp_id, c)| match c.role {
                    CorporationRole::Retailer { retail_sku } if retail_sku == sku_id => {
                        Some(RetailStoreOffer {
                            seller_corp_id: corp_id,
                            unit_price: c.posted_selling_price,
                            quality_score_bps: c.quality_score_bps,
                            brand_score_bps: c.brand_score_bps,
                            available_units: c.inventory_lot(sku_id).available_qty(),
                        })
                    }
                    _ => None,
                })
                .collect();

            let mut cohort_views = Vec::with_capacity(self.cohorts.len());
            for (cid, cohort) in &self.cohorts {
                let avail_cash = self.ledger.available_cash(cohort.entity_id)?;
                cohort_views.push((cid, cohort, avail_cash));
            }

            let settlements =
                compute_retail_clearing_plan(sku_id, &cohort_views, &offers, ref_price)?;

            for settlement in settlements {
                let seller_entity = self
                    .corporations
                    .get(settlement.seller_corp_id)
                    .ok_or(CoreError::UnknownCorporation(settlement.seller_corp_id))?
                    .entity_id;
                let buyer_entity = self
                    .cohorts
                    .get(settlement.buyer_cohort_id)
                    .ok_or(LedgerError::UnknownEntity(seller_entity))?
                    .entity_id;

                let cogs = self
                    .corporations
                    .get_mut(settlement.seller_corp_id)
                    .ok_or(CoreError::UnknownCorporation(settlement.seller_corp_id))?
                    .inventory_lot_mut(sku_id)
                    .draw_unreserved_units(settlement.units_sold)?;

                if let Some(seller) = self.corporations.get_mut(settlement.seller_corp_id) {
                    seller.monthly_units_sold = seller
                        .monthly_units_sold
                        .saturating_add(settlement.units_sold);
                }

                let h_exp = self
                    .ledger
                    .entity_account_id(buyer_entity, StandardAccountCode::ConsumptionExpense)?;
                let h_cash = self
                    .ledger
                    .entity_account_id(buyer_entity, StandardAccountCode::CashAndDeposits)?;
                let r_cash = self
                    .ledger
                    .entity_account_id(seller_entity, StandardAccountCode::CashAndDeposits)?;
                let r_rev = self
                    .ledger
                    .entity_account_id(seller_entity, StandardAccountCode::SalesRevenue)?;
                let r_cogs = self
                    .ledger
                    .entity_account_id(seller_entity, StandardAccountCode::CostOfGoodsSold)?;
                let r_inv = self.ledger.entity_account_id(seller_entity, inv_code)?;

                let mut entry = JournalEntry::new(tick, "B2C Retail Sale Settlement")
                    .debit(h_exp, settlement.gross_revenue)
                    .credit(h_cash, settlement.gross_revenue)
                    .debit(r_cash, settlement.gross_revenue)
                    .credit(r_rev, settlement.gross_revenue);

                if cogs.is_positive() {
                    entry = entry.debit(r_cogs, cogs).credit(r_inv, cogs);
                }

                self.ledger.post_entry(&entry)?;
            }
        }

        Ok(())
    }

    fn run_subphase_5_settlement_and_audit(&mut self) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        let Some((wage_entity, mgr_entity)) = self.wage_and_manager_entities() else {
            self.audit_world_invariants()?;
            return Ok(());
        };

        let corp_ids: Vec<CorpId> = self.corporations.keys().collect();

        // 5A. Pay daily non-capitalized wages (Retailers pay WageEarners; all corporations pay ManagersSkilled)
        for &corp_id in &corp_ids {
            let (entity_id, role, wage_bill, mgr_bill) = {
                let Some(c) = self.corporations.get(corp_id) else {
                    continue;
                };
                (c.entity_id, c.role, c.daily_wage_bill, c.daily_manager_bill)
            };

            if matches!(role, CorporationRole::Retailer { .. }) && wage_bill.is_positive() {
                let avail = self.ledger.available_cash(entity_id)?;
                let pay = wage_bill.min(avail);
                if pay.is_positive() {
                    self.post_payroll_transfer(tick, entity_id, wage_entity, pay)?;
                }
            }

            if mgr_bill.is_positive() {
                let avail = self.ledger.available_cash(entity_id)?;
                let pay = mgr_bill.min(avail);
                if pay.is_positive() {
                    self.post_payroll_transfer(tick, entity_id, mgr_entity, pay)?;
                }
            }
        }

        // 5B. Daily 30/360 Loan Interest & Deposit Interest Accruals
        for &corp_id in &corp_ids {
            let Some(c) = self.corporations.get(corp_id) else {
                continue;
            };
            let entity_id = c.entity_id;
            self.bank
                .accrue_and_collect_daily_loan_interest(&mut self.ledger, tick, entity_id)?;
            self.bank
                .accrue_and_pay_daily_deposit_interest(&mut self.ledger, tick, entity_id)?;
        }

        for (_, cohort) in &self.cohorts {
            self.bank.accrue_and_pay_daily_deposit_interest(
                &mut self.ledger,
                tick,
                cohort.entity_id,
            )?;
        }

        // 5C. Monthly Dividend Distribution (at Day 30 of each month)
        if self.current_tick.is_month_end() {
            for &corp_id in &corp_ids {
                self.distribute_monthly_dividend(tick, corp_id)?;
            }
        }

        // 5D. Quarterly Income Statement Closing into Retained Earnings (every 90 ticks)
        if self.current_tick.is_quarter_end() {
            let bank_id = self.bank.bank_id()?;
            self.ledger
                .close_income_statement_for_entity(tick, bank_id)?;
            for &corp_id in &corp_ids {
                if let Some(c) = self.corporations.get(corp_id) {
                    self.ledger
                        .close_income_statement_for_entity(tick, c.entity_id)?;
                }
            }
            for (_, cohort) in &self.cohorts {
                self.ledger
                    .close_income_statement_for_entity(tick, cohort.entity_id)?;
            }
        }

        // 5E. Strict Global Double-Entry Trial Balance & WAC Inventory Conservation Audit
        self.audit_world_invariants()?;
        Ok(())
    }

    fn post_payroll_transfer(
        &mut self,
        tick: u64,
        firm_entity: EntityId,
        cohort_entity: EntityId,
        amount: Money,
    ) -> Result<(), CoreError> {
        let f_wage = self
            .ledger
            .entity_account_id(firm_entity, StandardAccountCode::WageExpense)?;
        let f_cash = self
            .ledger
            .entity_account_id(firm_entity, StandardAccountCode::CashAndDeposits)?;
        let h_cash = self
            .ledger
            .entity_account_id(cohort_entity, StandardAccountCode::CashAndDeposits)?;
        let h_rev = self
            .ledger
            .entity_account_id(cohort_entity, StandardAccountCode::SalesRevenue)?;

        let entry = JournalEntry::new(tick, "Daily Payroll Settlement")
            .debit(f_wage, amount)
            .credit(f_cash, amount)
            .debit(h_cash, amount)
            .credit(h_rev, amount);
        self.ledger.post_entry(&entry)?;
        Ok(())
    }

    fn distribute_monthly_dividend(&mut self, tick: u64, corp_id: CorpId) -> Result<(), CoreError> {
        let (entity_id, payout_bps, wage_floor) = {
            let Some(c) = self.corporations.get(corp_id) else {
                return Ok(());
            };
            if c.target_dividend_payout_bps == 0 {
                return Ok(());
            }
            let daily_pay = c.daily_wage_bill.checked_add(c.daily_manager_bill)?;
            (
                c.entity_id,
                c.target_dividend_payout_bps,
                daily_pay.checked_mul_u64(15)?,
            )
        };

        let report = FinancialStatementReport::generate(&self.ledger, entity_id, tick)?;
        if !report.net_income.is_positive() {
            return Ok(());
        }

        let avail_cash = self.ledger.available_cash(entity_id)?;
        if avail_cash <= wage_floor {
            return Ok(());
        }
        let distributable_cash = avail_cash.checked_sub(wage_floor)?;
        let (desired_div, _) = report
            .net_income
            .mul_div_rem_u64(u64::from(payout_bps), BPS_DENOMINATOR)?;
        let total_dividend = desired_div.min(distributable_cash);
        if !total_dividend.is_positive() {
            return Ok(());
        }

        // Collect all shareholders of `corp_id` (note: reserved_shares still belong to `owned_shares` until fill!)
        let shareholder_weights: Vec<(EntityId, u64)> = self
            .equity_positions
            .range((corp_id, EntityId::default())..)
            .take_while(|&(&(cid, _), _)| cid == corp_id)
            .filter_map(|(&(_, holder_id), pos)| {
                if pos.owned_shares > 0 && holder_id != entity_id {
                    Some((holder_id, pos.owned_shares))
                } else {
                    None
                }
            })
            .collect();

        if shareholder_weights.is_empty() {
            return Ok(());
        }

        let allocations = allocate_hamilton(total_dividend, &shareholder_weights)?;
        let f_re = self
            .ledger
            .entity_account_id(entity_id, StandardAccountCode::RetainedEarnings)?;
        let f_cash = self
            .ledger
            .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;

        let mut entry = JournalEntry::new(tick, "Monthly Pro-Rata Shareholder Dividend")
            .debit(f_re, total_dividend)
            .credit(f_cash, total_dividend);

        for (holder_id, share_div) in allocations {
            if share_div.is_positive() {
                let h_cash = self
                    .ledger
                    .entity_account_id(holder_id, StandardAccountCode::CashAndDeposits)?;
                let h_div = self
                    .ledger
                    .entity_account_id(holder_id, StandardAccountCode::DividendIncome)?;
                entry = entry.debit(h_cash, share_div).credit(h_div, share_div);
            }
        }

        self.ledger.post_entry(&entry)?;
        Ok(())
    }

    fn wage_and_manager_entities(&self) -> Option<(EntityId, EntityId)> {
        let mut wage = None;
        let mut mgr = None;
        for (_, c) in &self.cohorts {
            match c.kind {
                HouseholdCohortKind::WageEarners => wage = Some(c.entity_id),
                HouseholdCohortKind::ManagersSkilled => mgr = Some(c.entity_id),
                HouseholdCohortKind::CapitalistsExecutives => {}
            }
        }
        Some((wage?, mgr?))
    }

    /// Audits both:
    /// 1. `self.ledger.audit_trial_balance()` (global double-entry conservation + endogenous M1 conservation).
    /// 2. Every corporation's `WacInventoryLot` book values summed by account code match the corporation's
    ///    ledger `InventoryRaw` / `InventoryWip` / `InventoryFinished` balance to the exact cent.
    pub fn audit_world_invariants(&self) -> Result<(), CoreError> {
        self.ledger.audit_trial_balance()?;

        for (_, corp) in &self.corporations {
            let mut expected_by_code: BTreeMap<StandardAccountCode, Money> = BTreeMap::new();
            for (&sku_id, lot) in &corp.inventory {
                let sku = self.skus.get(sku_id).ok_or(CoreError::UnknownSku(sku_id))?;
                let entry = expected_by_code
                    .entry(sku.ledger_inventory_account)
                    .or_insert(Money::ZERO);
                *entry = entry.checked_add(lot.total_book_value())?;
            }

            for code in [
                StandardAccountCode::InventoryRaw,
                StandardAccountCode::InventoryWip,
                StandardAccountCode::InventoryFinished,
            ] {
                let expected = expected_by_code.get(&code).copied().unwrap_or(Money::ZERO);
                let actual = self.ledger.entity_balance(corp.entity_id, code)?;
                if expected != actual {
                    return Err(CoreError::Ledger(LedgerError::TrialBalanceViolation {
                        reason: format!(
                            "corporation {} ({}) WAC inventory book value ({expected}) != ledger {:?} balance ({actual})",
                            corp.ticker, corp.name, code
                        ),
                    }));
                }
            }
        }

        Ok(())
    }

    /// Projects an immutable `WorldView` snapshot for the 120 FPS `egui` terminal.
    pub fn build_world_view(&self) -> Result<WorldView, CoreError> {
        let tick = self.current_tick;
        let bank_id = self.bank.bank_id()?;
        let bank_statement = FinancialStatementReport::generate(&self.ledger, bank_id, tick.0)?;
        let total_m1 = self
            .ledger
            .entity_balance(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
        let total_loans = self
            .ledger
            .entity_balance(bank_id, StandardAccountCode::LoansReceivable)?;

        let mut instruments = Vec::with_capacity(self.order_books.len());
        for (_, book) in &self.order_books {
            instruments.push(InstrumentSnapshot {
                instrument_id: book.instrument_id,
                ticker: book.ticker.clone(),
                kind: book.kind,
                last_price: book.last_price,
                best_bid: book.best_bid(),
                best_ask: book.best_ask(),
                top_bids: book.top_bid_levels(10),
                top_asks: book.top_ask_levels(10),
                ohlcv_bars: book.history_bars.iter().copied().collect(),
                recent_fills: book.recent_fills.iter().cloned().collect(),
            });
        }

        let mut corporations = Vec::with_capacity(self.corporations.len());
        for (_, corp) in &self.corporations {
            let cash_balance = self.ledger.cash_balance(corp.entity_id)?;
            let available_cash = self.ledger.available_cash(corp.entity_id)?;
            let loans_payable = self
                .ledger
                .entity_balance(corp.entity_id, StandardAccountCode::LoansPayable)?;
            let share_price = corp
                .equity_instrument_id
                .and_then(|id| self.order_books.get(id))
                .map_or(Money::from_dollars(10), |b| b.last_price);
            let market_cap = share_price.checked_mul_u64(corp.total_shares_outstanding)?;
            let financial_statement =
                FinancialStatementReport::generate(&self.ledger, corp.entity_id, tick.0)?;

            corporations.push(CorporationSnapshot {
                corp_id: corp.corp_id,
                entity_id: corp.entity_id,
                equity_instrument_id: corp.equity_instrument_id,
                ticker: corp.ticker.clone(),
                name: corp.name.clone(),
                role: corp.role,
                delegation: corp.delegation,
                cash_balance,
                available_cash,
                loans_payable,
                daily_capacity: corp.daily_capacity,
                daily_wage_bill: corp.daily_wage_bill,
                daily_manager_bill: corp.daily_manager_bill,
                posted_selling_price: corp.posted_selling_price,
                max_input_bid_price: corp.max_input_bid_price,
                quality_score_bps: corp.quality_score_bps,
                brand_score_bps: corp.brand_score_bps,
                target_dividend_payout_bps: corp.target_dividend_payout_bps,
                total_shares_outstanding: corp.total_shares_outstanding,
                share_price,
                market_cap,
                inventory: corp.inventory.clone(),
                financial_statement,
            });
        }

        let mut cohorts = Vec::with_capacity(self.cohorts.len());
        for (_, cohort) in &self.cohorts {
            let cash_deposits = self.ledger.cash_balance(cohort.entity_id)?;
            let financial_statement =
                FinancialStatementReport::generate(&self.ledger, cohort.entity_id, tick.0)?;
            cohorts.push(CohortSnapshot {
                cohort_id: cohort.cohort_id,
                entity_id: cohort.entity_id,
                kind: cohort.kind,
                name: cohort.name.clone(),
                population_count: cohort.population_count,
                cash_deposits,
                savings_subsistence_floor: cohort.savings_subsistence_floor,
                financial_statement,
            });
        }

        let mut sku_codes = BTreeMap::new();
        for (sku_id, sku) in &self.skus {
            sku_codes.insert(sku_id, sku.code.clone());
        }

        Ok(WorldView {
            tick,
            date: tick.to_date360(),
            bank_entity_id: bank_id,
            bank_statement,
            bank_loan_rate_bps: self.bank.default_loan_rate_bps,
            bank_deposit_rate_bps: self.bank.deposit_rate_bps,
            total_m1_money_supply: total_m1,
            total_bank_loans: total_loans,
            corporations,
            cohorts,
            instruments,
            active_b2b_contracts: self.b2b_contracts.clone(),
            sku_codes,
        })
    }
}
