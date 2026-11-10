use crate::b2b_market::{B2BSupplyContract, WholesaleSpotOffer};
use crate::calendar::Tick;
use crate::cfo::{apply_pricing_stance, apply_wage_stance, reconcile_strategic_directive};
use crate::clob::{CentralLimitOrderBook, InstrumentKind, LimitOrder, OrderSide, TradeFill};
use crate::command::{JointCapitalAction, PlayerCommand, StrategicDirective};
use crate::error::CoreError;
use crate::household::{HouseholdCohort, HouseholdCohortKind, RetailStoreOffer};
use crate::ids::{
    CohortId, ContractId, CorpId, InstrumentId, OrderId, ShortLoanId, SkuId, WireMessageId,
};
use crate::production::{
    AiDelegationConfig, CloudTier, Corporation, CorporationRole, CreditRating,
    ExecutivePersonaKind, SkuDefinition,
};
use crate::retail_market::compute_retail_clearing_plan;
use crate::snapshot::{
    CohortSnapshot, CorporationSnapshot, InstrumentSnapshot, MacroTelemetrySnapshot, NewsCategory,
    NewsHeadline, ShareholderRecord, WorldView,
};
use crate::wire::{
    CrimeOperationKind, MAX_RESOLVED_WIRE_MESSAGES_PER_CORP, ServiceCreditPosition,
    ShortPositionRecord, WireAttachment, WireAttachmentStatus, WireMessage,
};
use oligopole_ledger::{
    BPS_DENOMINATOR, CommercialBankFacility, EntityId, FinancialStatementReport, JournalEntry,
    LedgerEngine, LedgerError, Money, RoundingReserve, StandardAccountCode, allocate_hamilton,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Maximum number of corporate & macroeconomic headlines retained in the live Bloomberg News Wire (`NEWS <GO>`).
pub const MAX_NEWS_FEED_HEADLINES: usize = 64;

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
    pub next_wire_seq: u64,
    pub next_short_loan_seq: u64,
    pub ledger: LedgerEngine,
    pub bank: CommercialBankFacility,
    pub sovereign_entity_id: EntityId,
    pub sovereign_dr_reserve_buffer: Money,
    pub skus: SlotMap<SkuId, SkuDefinition>,
    pub corporations: SlotMap<CorpId, Corporation>,
    pub cohorts: SlotMap<CohortId, HouseholdCohort>,
    pub order_books: SlotMap<InstrumentId, CentralLimitOrderBook>,
    pub b2b_contracts: Vec<B2BSupplyContract>,
    /// Keyed by `(CorpId, Shareholder EntityId)` for canonical iteration and dividend splits.
    pub equity_positions: BTreeMap<(CorpId, EntityId), EquityHoldingRecord>,
    /// Keyed by `(InstrumentId, Holder EntityId)` for Global Depositary Receipt holdings.
    pub depositary_positions: BTreeMap<(InstrumentId, EntityId), EquityHoldingRecord>,
    /// Total Depositary Receipt units issued per `InstrumentId`.
    pub depositary_total_units: BTreeMap<InstrumentId, u64>,
    /// Per-entity `ServiceCredit` (`PLEX` equivalent) balances and reservations.
    pub service_credit_positions: BTreeMap<EntityId, ServiceCreditPosition>,
    /// Total `ServiceCredit` units in circulation (`sum(owned_credits)`).
    pub total_service_credits_supply: u64,
    /// Idempotent payment webhook reference registry (Guard #5).
    pub processed_webhook_refs: BTreeSet<String>,
    /// Active physical share-borrow short positions (Guard #2).
    pub short_positions: BTreeMap<ShortLoanId, ShortPositionRecord>,
    /// Senior forensic tax liens owed to `SovereignTreasury` (Guard #4).
    pub tax_liens: BTreeMap<EntityId, Money>,
    /// Corporate Wire (`MSG`) inbox/outbox messages and smart attachments (Guard #6).
    pub wire_messages: Vec<WireMessage>,
    /// Active corporate espionage wiretaps `(Attacker CorpId, Target CorpId) -> expires_at_tick`.
    pub active_wiretaps: BTreeMap<(CorpId, CorpId), u64>,
    /// Monotonic sequence counter for `NewsHeadline` events.
    pub next_news_seq: u64,
    /// Bounded ring buffer of deterministic corporate & macro headlines (`NEWS <GO>`).
    pub news_feed: VecDeque<NewsHeadline>,
    /// Gross trade & B2C transaction volume accumulated in the current 30-day month.
    pub current_month_gdp_accumulator: Money,
    /// Trailing 30-day macroeconomic transaction velocity (`GDP` proxy).
    pub trailing_30d_gdp: Money,
}

/// Serializable wrapper for `EquityPosition`.
pub type EquityHoldingRecord = EquityPosition;

impl WorldState {
    pub fn new(world_seed: [u8; 32], loan_rate_bps: u32, deposit_rate_bps: u32) -> Self {
        let mut ledger = LedgerEngine::new();
        let bank_id = ledger.register_entity("Central & Commercial Bank", true);
        let sovereign_entity_id =
            ledger.register_entity("Sovereign Treasury & Depositary", false);
        let bank = CommercialBankFacility::new(bank_id, loan_rate_bps, deposit_rate_bps);

        Self {
            world_seed,
            current_tick: Tick::ZERO,
            occ_version: 1,
            next_order_seq: 1,
            next_contract_seq: 1,
            next_wire_seq: 1,
            next_short_loan_seq: 1,
            ledger,
            bank,
            sovereign_entity_id,
            sovereign_dr_reserve_buffer: Money::from_dollars(500_000),
            skus: SlotMap::with_key(),
            corporations: SlotMap::with_key(),
            cohorts: SlotMap::with_key(),
            order_books: SlotMap::with_key(),
            b2b_contracts: Vec::new(),
            equity_positions: BTreeMap::new(),
            depositary_positions: BTreeMap::new(),
            depositary_total_units: BTreeMap::new(),
            service_credit_positions: BTreeMap::new(),
            total_service_credits_supply: 0,
            processed_webhook_refs: BTreeSet::new(),
            short_positions: BTreeMap::new(),
            tax_liens: BTreeMap::new(),
            wire_messages: Vec::new(),
            active_wiretaps: BTreeMap::new(),
            next_news_seq: 1,
            news_feed: VecDeque::with_capacity(MAX_NEWS_FEED_HEADLINES),
            current_month_gdp_accumulator: Money::ZERO,
            trailing_30d_gdp: Money::ZERO,
        }
    }

    /// Appends a deterministic headline to the bounded `news_feed` ring buffer (`MAX_NEWS_FEED_HEADLINES = 64`).
    pub fn push_news(&mut self, ticker: &str, category: NewsCategory, headline: String) {
        if self.news_feed.len() >= MAX_NEWS_FEED_HEADLINES {
            self.news_feed.pop_front();
        }
        let seq_id = self.next_news_seq;
        self.next_news_seq = self.next_news_seq.saturating_add(1);
        self.news_feed.push_back(NewsHeadline {
            seq_id,
            tick: self.current_tick,
            date: self.current_tick.to_date360(),
            ticker: ticker.to_owned(),
            category,
            headline,
        });
    }

    pub fn find_corp_by_entity(&self, entity_id: EntityId) -> Option<CorpId> {
        self.corporations
            .iter()
            .find(|(_, c)| c.entity_id == entity_id)
            .map(|(id, _)| id)
    }

    /// Derives a deterministic `ChaCha8Rng` seeded from `(world_seed, current_tick, seq_no)` (Guard #5).
    pub fn seeded_command_rng(&self, seq_no: u32) -> ChaCha8Rng {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.world_seed);
        hasher.update(&self.current_tick.0.to_le_bytes());
        hasher.update(&seq_no.to_le_bytes());
        let digest = hasher.finalize();
        ChaCha8Rng::from_seed(*digest.as_bytes())
    }

    /// Executes a single `PlayerCommand` at `seq_no = 0`.
    pub fn execute_command(&mut self, cmd: PlayerCommand) -> Result<(), CoreError> {
        self.execute_command_sequenced(cmd, 0)
    }

    /// Executes a single `PlayerCommand` (or Slow-Tick `StrategicDirective`) deterministically
    /// with explicit intraday `seq_no` for RNG seeding (Guard #5).
    pub fn execute_command_sequenced(
        &mut self,
        cmd: PlayerCommand,
        seq_no: u32,
    ) -> Result<(), CoreError> {
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
                let (entity_id, ticker, rating) = {
                    let c = self
                        .corporations
                        .get(corp_id)
                        .ok_or(CoreError::UnknownCorporation(corp_id))?;
                    (c.entity_id, c.ticker.clone(), c.credit_rating)
                };
                let eff_rate = self
                    .bank
                    .default_loan_rate_bps
                    .saturating_add(rating.spread_bps());
                self.bank.originate_loan(
                    &mut self.ledger,
                    tick,
                    entity_id,
                    principal,
                    Some(eff_rate),
                )?;
                self.push_news(
                    &ticker,
                    NewsCategory::CreditAndDebt,
                    format!(
                        "{ticker} draws {principal} credit facility at {}.{:02}% ({})",
                        eff_rate / 100,
                        eff_rate % 100,
                        rating.badge_label()
                    ),
                );
            }
            PlayerCommand::RepayBankLoan { corp_id, amount } => {
                let (entity_id, ticker) = {
                    let c = self
                        .corporations
                        .get(corp_id)
                        .ok_or(CoreError::UnknownCorporation(corp_id))?;
                    (c.entity_id, c.ticker.clone())
                };
                let avail = self.ledger.available_cash(entity_id)?;
                let pay = amount.min(avail);
                if pay.is_positive() {
                    let repaid = self
                        .bank
                        .repay_loan_principal(&mut self.ledger, tick, entity_id, pay)?;
                    if repaid.is_positive() {
                        self.push_news(
                            &ticker,
                            NewsCategory::CreditAndDebt,
                            format!("{ticker} retires {repaid} senior bank debt"),
                        );
                    }
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
            PlayerCommand::AuthorityMintServiceCredit {
                recipient_entity_id,
                credits,
                webhook_ref,
            } => {
                self.execute_authority_mint_service_credit(
                    recipient_entity_id,
                    credits,
                    webhook_ref,
                )?;
            }
            PlayerCommand::IngestOraclePriceTick {
                instrument_id,
                new_oracle_price,
            } => {
                self.execute_ingest_oracle_price_tick(instrument_id, new_oracle_price)?;
            }
            PlayerCommand::FoundPrivateSme {
                founder_entity_id,
                ticker,
                name,
                role,
                seed_capital,
            } => {
                self.execute_found_private_sme(
                    founder_entity_id,
                    &ticker,
                    &name,
                    role,
                    seed_capital,
                )?;
            }
            PlayerCommand::LaunchIpoAuction {
                corp_id,
                primary_shares_to_issue,
                offer_price,
            } => {
                self.execute_launch_ipo_auction(corp_id, primary_shares_to_issue, offer_price)?;
            }
            PlayerCommand::BorrowSharesToShort {
                borrower_entity_id,
                corp_id,
                shares,
                sell_limit_price,
            } => {
                self.execute_borrow_shares_to_short(
                    borrower_entity_id,
                    corp_id,
                    shares,
                    sell_limit_price,
                )?;
            }
            PlayerCommand::CoverShortPosition {
                short_loan_id,
                max_buy_price,
            } => {
                self.execute_cover_short_position(short_loan_id, max_buy_price)?;
            }
            PlayerCommand::ExecuteCrimeOperation {
                actor_corp_id,
                target_corp_id,
                kind,
            } => {
                self.execute_crime_operation(actor_corp_id, target_corp_id, kind, seq_no)?;
            }
            PlayerCommand::LaunderIllicitCash { corp_id, amount } => {
                self.execute_launder_illicit_cash(corp_id, amount, seq_no)?;
            }
            PlayerCommand::SetCloudSubscriptionTier { corp_id, tier } => {
                let corp = self
                    .corporations
                    .get_mut(corp_id)
                    .ok_or(CoreError::UnknownCorporation(corp_id))?;
                corp.subscribed_cloud_tier = tier;
            }
            PlayerCommand::SendWireMessage {
                sender_corp_id,
                recipient_corp_id,
                subject,
                body,
                attachment,
            } => {
                self.execute_send_wire_message(
                    sender_corp_id,
                    recipient_corp_id,
                    subject,
                    body,
                    attachment,
                )?;
            }
            PlayerCommand::RespondWireAttachment {
                responder_corp_id,
                message_id,
                accept,
            } => {
                self.execute_respond_wire_attachment(responder_corp_id, message_id, accept)?;
            }
            PlayerCommand::ExecuteFacilityMaintenance { corp_id } => {
                self.execute_maintenance_and_rd_overhaul(
                    corp_id,
                    Money::from_dollars(18_000),
                    1_500,
                    200,
                )?;
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
            let eff_rate = self
                .bank
                .default_loan_rate_bps
                .saturating_add(corp.credit_rating.spread_bps());
            let reconciled = reconcile_strategic_directive(
                corp,
                avail_cash,
                debt,
                eff_rate,
                raw_directive,
            )?;
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
            JointCapitalAction::RepurchaseShares {
                buyback_budget,
                max_limit_price,
            } => {
                self.execute_share_buyback(corp_id, *buyback_budget, *max_limit_price)?;
            }
            JointCapitalAction::IssueEquityShares {
                shares_to_issue,
                min_ask_price,
            } => {
                self.execute_equity_issuance(corp_id, *shares_to_issue, *min_ask_price)?;
            }
            JointCapitalAction::MaintenanceAndRdOverhaul {
                overhaul_cost,
                condition_restore_bps,
                quality_boost_bps,
                ..
            } => {
                self.execute_maintenance_and_rd_overhaul(
                    corp_id,
                    *overhaul_cost,
                    *condition_restore_bps,
                    *quality_boost_bps,
                )?;
            }
        }

        // 2. Apply pricing, wage, and dividend policy dials with role-specific competitive reference band clamping
        let ref_price = self
            .corporations
            .get(corp_id)
            .map_or(Money::from_dollars(100), |c| match c.role {
                CorporationRole::ResourceExtractor { output_sku } => self
                    .skus
                    .get(output_sku)
                    .map_or(Money::from_dollars(42), |s| s.base_reference_price),
                CorporationRole::Manufacturer { output_sku, .. } => {
                    // Wholesale manufacturer reference price is 60% of finished retail SKU reference price
                    let retail_ref = self
                        .skus
                        .get(output_sku)
                        .map_or(Money::from_dollars(220), |s| s.base_reference_price);
                    Money::from_cents((retail_ref.cents() * 60) / 100)
                }
                CorporationRole::Retailer { retail_sku } => self
                    .skus
                    .get(retail_sku)
                    .map_or(Money::from_dollars(220), |s| s.base_reference_price),
                CorporationRole::LogisticsTrucking { .. } => Money::from_dollars(15),
                CorporationRole::PrivateSecurity { .. } => Money::from_dollars(2_500),
            });

        if let Some(corp) = self.corporations.get_mut(corp_id) {
            if !corp.delegation.is_player_controlled || corp.delegation.delegate_pricing_to_ai {
                let raw_new_price = match reconciled.pricing_stance {
                    crate::command::PricingStance::MatchMarket => {
                        Money::from_cents(i128::midpoint(
                            corp.posted_selling_price.cents(),
                            ref_price.cents(),
                        ))
                    }
                    other => apply_pricing_stance(corp.posted_selling_price, other)?,
                };
                let min_band = Money::from_cents((ref_price.cents() * 75) / 100);
                let max_band = Money::from_cents((ref_price.cents() * 125) / 100);
                corp.posted_selling_price = raw_new_price.clamp(min_band, max_band);
                // Scale input willingness-to-pay proportionally (80% of output selling price)
                corp.max_input_bid_price =
                    Money::from_cents((corp.posted_selling_price.cents() * 80) / 100);
            }
            if !corp.delegation.is_player_controlled
                || corp.delegation.delegate_capex_and_financing_to_ai
            {
                let raw_wage =
                    apply_wage_stance(corp.daily_wage_bill, reconciled.wage_stance)?;
                corp.daily_wage_bill =
                    raw_wage.clamp(Money::from_dollars(500), Money::from_dollars(8_000));
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
        let (entity_id, ticker, rating) = {
            let c = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            (c.entity_id, c.ticker.clone(), c.credit_rating)
        };

        if loan_principal.is_positive() {
            let eff_rate = self
                .bank
                .default_loan_rate_bps
                .saturating_add(rating.spread_bps());
            self.bank.originate_loan(
                &mut self.ledger,
                tick,
                entity_id,
                loan_principal,
                Some(eff_rate),
            )?;
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

            let mut new_cap = 0;
            if let Some(corp) = self.corporations.get_mut(corp_id) {
                corp.daily_capacity = corp.daily_capacity.saturating_add(added_capacity);
                corp.facility_condition_bps = corp
                    .facility_condition_bps
                    .saturating_add(400)
                    .min(10_000);
                new_cap = corp.daily_capacity;
            }
            self.push_news(
                &ticker,
                NewsCategory::CapExAndExpansion,
                format!(
                    "{ticker} commissions +{added_capacity}/d plant expansion ({capex_cost} CapEx -> {new_cap}/d total)"
                ),
            );
        }
        Ok(())
    }

    fn execute_maintenance_and_rd_overhaul(
        &mut self,
        corp_id: CorpId,
        overhaul_cost: Money,
        condition_restore_bps: u32,
        quality_boost_bps: u32,
    ) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        let (entity_id, ticker) = {
            let c = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            (c.entity_id, c.ticker.clone())
        };

        let avail = self.ledger.available_cash(entity_id)?;
        if overhaul_cost.is_positive() && avail >= overhaul_cost {
            if let Some((wage_ent, mgr_ent)) = self.wage_and_manager_entities() {
                let wage_half = Money::from_cents(overhaul_cost.cents() / 2);
                let mgr_half = overhaul_cost.checked_sub(wage_half)?;

                let firm_dep = self.ledger.entity_account_id(
                    entity_id,
                    StandardAccountCode::DepreciationExpense,
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

                let mut entry = JournalEntry::new(tick, "Plant Maintenance & R&D Overhaul")
                    .debit(firm_dep, overhaul_cost)
                    .credit(firm_cash, overhaul_cost);
                if wage_half.is_positive() {
                    entry = entry.debit(w_cash, wage_half).credit(w_rev, wage_half);
                }
                if mgr_half.is_positive() {
                    entry = entry.debit(m_cash, mgr_half).credit(m_rev, mgr_half);
                }
                self.ledger.post_entry(&entry)?;
            }

            let mut new_cond = 10_000;
            let mut new_qual = 10_000;
            if let Some(corp) = self.corporations.get_mut(corp_id) {
                corp.facility_condition_bps = corp
                    .facility_condition_bps
                    .saturating_add(condition_restore_bps)
                    .min(10_000);
                corp.quality_score_bps = corp
                    .quality_score_bps
                    .saturating_add(quality_boost_bps)
                    .min(20_000);
                new_cond = corp.facility_condition_bps;
                new_qual = corp.quality_score_bps;
            }
            self.push_news(
                &ticker,
                NewsCategory::RdAndMaintenance,
                format!(
                    "{ticker} completes {overhaul_cost} R&D & plant overhaul (Condition {}.{:01}%, Quality {} bps)",
                    new_cond / 100,
                    (new_cond % 100) / 10,
                    new_qual
                ),
            );
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

    fn execute_share_buyback(
        &mut self,
        corp_id: CorpId,
        buyback_budget: Money,
        max_limit_price: Money,
    ) -> Result<(), CoreError> {
        if !buyback_budget.is_positive() || !max_limit_price.is_positive() {
            return Ok(());
        }
        let (entity_id, eq_inst_id) = {
            let corp = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            let Some(eq_inst_id) = corp.equity_instrument_id else {
                return Ok(());
            };
            (corp.entity_id, eq_inst_id)
        };

        self.cancel_all_orders_for_entity_on_book(eq_inst_id, entity_id)?;
        let avail_cash = self.ledger.available_cash(entity_id)?;
        let working_floor = match self.corporations.get(corp_id) {
            Some(c) => {
                let debt = self
                    .bank
                    .loans
                    .get(&entity_id)
                    .map_or(Money::ZERO, |l| l.principal_outstanding);
                let eff_rate = self
                    .bank
                    .default_loan_rate_bps
                    .saturating_add(c.credit_rating.spread_bps());
                c.working_capital_floor(45, debt, eff_rate)?
            }
            None => Money::ZERO,
        };
        if avail_cash <= working_floor {
            return Ok(());
        }
        let spendable = avail_cash.checked_sub(working_floor)?;
        let budget = buyback_budget.min(spendable);
        let safe_px = self
            .order_books
            .get(eq_inst_id)
            .map_or(max_limit_price, |b| b.clamp_luld_price(max_limit_price));
        let qty_i128 = budget.checked_div_euclid(safe_px.cents())?.cents();
        let qty = u64::try_from(qty_i128).unwrap_or(0);
        if qty > 0 {
            let (order_id, _) = self.submit_clob_limit_order(
                entity_id,
                eq_inst_id,
                OrderSide::Bid,
                safe_px,
                qty,
            )?;
            // Immediate-or-Cancel (IOC) execution for corporate buybacks so operating cash is never locked indefinitely
            let is_still_resting = self
                .order_books
                .get(eq_inst_id)
                .is_some_and(|b| b.bids.values().any(|q| q.iter().any(|o| o.order_id == order_id)));
            if is_still_resting {
                let _ = self.cancel_clob_limit_order(eq_inst_id, order_id);
            }
        }
        Ok(())
    }

    fn execute_equity_issuance(
        &mut self,
        corp_id: CorpId,
        shares_to_issue: u64,
        min_ask_price: Money,
    ) -> Result<(), CoreError> {
        if shares_to_issue == 0 || !min_ask_price.is_positive() {
            return Ok(());
        }
        let (entity_id, eq_inst_id) = {
            let corp = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            let Some(eq_inst_id) = corp.equity_instrument_id else {
                return Ok(());
            };
            (corp.entity_id, eq_inst_id)
        };

        self.cancel_all_orders_for_entity_on_book(eq_inst_id, entity_id)?;
        let safe_px = self
            .order_books
            .get(eq_inst_id)
            .map_or(min_ask_price, |b| b.clamp_luld_price(min_ask_price));
        if let Some(corp) = self.corporations.get_mut(corp_id) {
            corp.total_shares_outstanding = corp
                .total_shares_outstanding
                .saturating_add(shares_to_issue);
        }
        let pos = self
            .equity_positions
            .entry((corp_id, entity_id))
            .or_default();
        pos.owned_shares = pos.owned_shares.saturating_add(shares_to_issue);
        let total_treasury = pos.available_shares();
        if total_treasury > 0 {
            self.submit_clob_limit_order(
                entity_id,
                eq_inst_id,
                OrderSide::Ask,
                safe_px,
                total_treasury,
            )?;
        }
        Ok(())
    }

    /// Reserves collateral (`reserved_cash`, `reserved_qty`, `reserved_shares`, or `reserved_credits`),
    /// validates LULD $\pm 10\%$ circuit-breaker bounds, submits a limit order to `instrument_id`,
    /// and immediately settles any matched `TradeFill`s (including the `10 bps` Sovereign rake).
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

        let kind = {
            let book = self
                .order_books
                .get(instrument_id)
                .ok_or(CoreError::UnknownInstrument(instrument_id))?;
            book.validate_luld_price(limit_price)?;
            book.kind
        };

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
                    let locked_founder_shares = self
                        .corporations
                        .get(corp_id)
                        .map_or(0, |c| {
                            if c.founder_entity_id == Some(trader_entity_id)
                                && self.current_tick.0 < c.founder_lockup_until_tick
                            {
                                c.founder_locked_shares
                            } else {
                                0
                            }
                        });
                    let pos = self
                        .equity_positions
                        .entry((corp_id, trader_entity_id))
                        .or_default();
                    let avail = pos.available_shares().saturating_sub(locked_founder_shares);
                    if qty > avail {
                        return Err(CoreError::InsufficientShares {
                            corp_id,
                            available: avail,
                            requested: qty,
                        });
                    }
                    pos.reserved_shares = pos.reserved_shares.saturating_add(qty);
                }
                InstrumentKind::DepositaryReceipt { .. } => {
                    let pos = self
                        .depositary_positions
                        .entry((instrument_id, trader_entity_id))
                        .or_default();
                    let avail = pos.available_shares();
                    if qty > avail {
                        return Err(CoreError::InvalidOrder {
                            reason: format!(
                                "insufficient Depositary Receipt units: available {avail}, requested {qty}"
                            ),
                        });
                    }
                    pos.reserved_shares = pos.reserved_shares.saturating_add(qty);
                }
                InstrumentKind::ServiceCredit => {
                    let pos = self
                        .service_credit_positions
                        .entry(trader_entity_id)
                        .or_default();
                    let avail = pos.available_credits();
                    if qty > avail {
                        return Err(CoreError::InvalidOrder {
                            reason: format!(
                                "insufficient ServiceCredits: available {avail}, requested {qty}"
                            ),
                        });
                    }
                    pos.reserved_credits = pos.reserved_credits.saturating_add(qty);
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
                InstrumentKind::DepositaryReceipt { .. } => {
                    if let Some(pos) = self
                        .depositary_positions
                        .get_mut(&(order.instrument_id, order.trader_entity_id))
                    {
                        pos.reserved_shares =
                            pos.reserved_shares.saturating_sub(order.remaining_qty);
                    }
                }
                InstrumentKind::ServiceCredit => {
                    if let Some(pos) = self.service_credit_positions.get_mut(&order.trader_entity_id)
                    {
                        pos.reserved_credits =
                            pos.reserved_credits.saturating_sub(order.remaining_qty);
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
        // 10 bps Sovereign Monopoly Toll Gate rake on non-sovereign trades
        let sovereign_rake = if fill.buyer_entity_id == self.sovereign_entity_id
            || fill.seller_entity_id == self.sovereign_entity_id
        {
            Money::ZERO
        } else {
            let (rake, _) = trade_notional.mul_div_rem_u64(10, BPS_DENOMINATOR)?;
            rake
        };
        let seller_net_cash = trade_notional.checked_sub(sovereign_rake)?;

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
                    .credit(seller_rev, trade_notional);

                if seller_net_cash.is_positive() {
                    entry = entry.debit(seller_cash, seller_net_cash);
                }
                if sovereign_rake.is_positive() {
                    let seller_tax = self.ledger.entity_account_id(
                        fill.seller_entity_id,
                        StandardAccountCode::TaxAndFineExpense,
                    )?;
                    let sov_cash = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::CashAndDeposits,
                    )?;
                    let sov_rev = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::TaxAndFeeRevenue,
                    )?;
                    entry = entry
                        .debit(seller_tax, sovereign_rake)
                        .debit(sov_cash, sovereign_rake)
                        .credit(sov_rev, sovereign_rake);
                }

                if seller_cogs.is_positive() {
                    entry = entry
                        .debit(seller_cogs_acc, seller_cogs)
                        .credit(seller_inv, seller_cogs);
                }

                self.ledger.post_entry(&entry)?;
                self.current_month_gdp_accumulator = self
                    .current_month_gdp_accumulator
                    .checked_add(trade_notional)?;
            }
            InstrumentKind::Equity { corp_id } => {
                // Secondary market equity transfer (or corporate share buyback / primary issuance):
                let issuer_entity_id = self
                    .corporations
                    .get(corp_id)
                    .map(|c| c.entity_id);

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

                if issuer_entity_id == Some(fill.buyer_entity_id) {
                    // Corporate Share Buyback: the issuing corporation purchased its own shares and immediately retires them!
                    let mut retired_ticker = None;
                    let mut new_total_shares = 0;
                    if let Some(corp) = self.corporations.get_mut(corp_id) {
                        corp.total_shares_outstanding = corp
                            .total_shares_outstanding
                            .saturating_sub(fill.qty)
                            .max(1);
                        retired_ticker = Some(corp.ticker.clone());
                        new_total_shares = corp.total_shares_outstanding;
                    }
                    if let Some(tkr) = retired_ticker {
                        self.push_news(
                            &tkr,
                            NewsCategory::DividendAndBuyback,
                            format!(
                                "{tkr} repurchases & retires {} shares at {} ({new_total_shares} shares out)",
                                fill.qty, fill.execution_price
                            ),
                        );
                    }
                } else {
                    let buyer_pos = self
                        .equity_positions
                        .entry((corp_id, fill.buyer_entity_id))
                        .or_default();
                    buyer_pos.owned_shares = buyer_pos.owned_shares.saturating_add(fill.qty);
                }

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

                let mut entry = JournalEntry::new(tick, "Secondary Equity CLOB Trade Settlement")
                    .debit(buyer_eq, trade_notional)
                    .credit(buyer_cash, trade_notional)
                    .credit(seller_eq, trade_notional);

                if seller_net_cash.is_positive() {
                    entry = entry.debit(seller_cash, seller_net_cash);
                }
                if sovereign_rake.is_positive() {
                    let seller_tax = self.ledger.entity_account_id(
                        fill.seller_entity_id,
                        StandardAccountCode::TaxAndFineExpense,
                    )?;
                    let sov_cash = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::CashAndDeposits,
                    )?;
                    let sov_rev = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::TaxAndFeeRevenue,
                    )?;
                    entry = entry
                        .debit(seller_tax, sovereign_rake)
                        .debit(sov_cash, sovereign_rake)
                        .credit(sov_rev, sovereign_rake);
                }
                self.ledger.post_entry(&entry)?;
                self.current_month_gdp_accumulator = self
                    .current_month_gdp_accumulator
                    .checked_add(trade_notional)?;
            }
            InstrumentKind::DepositaryReceipt { .. } => {
                let seller_pos = self
                    .depositary_positions
                    .entry((fill.instrument_id, fill.seller_entity_id))
                    .or_default();
                if fill.qty > seller_pos.reserved_shares || fill.qty > seller_pos.owned_shares {
                    return Err(CoreError::InvalidOrder {
                        reason: "insufficient reserved Depositary Receipt units on fill".to_owned(),
                    });
                }
                seller_pos.reserved_shares -= fill.qty;
                seller_pos.owned_shares -= fill.qty;

                let buyer_pos = self
                    .depositary_positions
                    .entry((fill.instrument_id, fill.buyer_entity_id))
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

                let mut entry =
                    JournalEntry::new(tick, "Global Depositary Receipt CLOB Trade Settlement")
                        .debit(buyer_eq, trade_notional)
                        .credit(buyer_cash, trade_notional)
                        .credit(seller_eq, trade_notional);

                if seller_net_cash.is_positive() {
                    entry = entry.debit(seller_cash, seller_net_cash);
                }
                if sovereign_rake.is_positive() {
                    let seller_tax = self.ledger.entity_account_id(
                        fill.seller_entity_id,
                        StandardAccountCode::TaxAndFineExpense,
                    )?;
                    let sov_cash = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::CashAndDeposits,
                    )?;
                    let sov_rev = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::TaxAndFeeRevenue,
                    )?;
                    entry = entry
                        .debit(seller_tax, sovereign_rake)
                        .debit(sov_cash, sovereign_rake)
                        .credit(sov_rev, sovereign_rake);
                }
                self.ledger.post_entry(&entry)?;
                self.current_month_gdp_accumulator = self
                    .current_month_gdp_accumulator
                    .checked_add(trade_notional)?;
            }
            InstrumentKind::ServiceCredit => {
                let seller_pos = self
                    .service_credit_positions
                    .entry(fill.seller_entity_id)
                    .or_default();
                if fill.qty > seller_pos.reserved_credits || fill.qty > seller_pos.owned_credits {
                    return Err(CoreError::InvalidOrder {
                        reason: "insufficient reserved ServiceCredits on fill".to_owned(),
                    });
                }
                seller_pos.reserved_credits -= fill.qty;
                seller_pos.owned_credits -= fill.qty;

                let buyer_pos = self
                    .service_credit_positions
                    .entry(fill.buyer_entity_id)
                    .or_default();
                buyer_pos.owned_credits = buyer_pos.owned_credits.saturating_add(fill.qty);

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

                let mut entry = JournalEntry::new(tick, "ServiceCredit (PLEX) CLOB Trade Settlement")
                    .debit(buyer_eq, trade_notional)
                    .credit(buyer_cash, trade_notional)
                    .credit(seller_eq, trade_notional);

                if seller_net_cash.is_positive() {
                    entry = entry.debit(seller_cash, seller_net_cash);
                }
                if sovereign_rake.is_positive() {
                    let seller_tax = self.ledger.entity_account_id(
                        fill.seller_entity_id,
                        StandardAccountCode::TaxAndFineExpense,
                    )?;
                    let sov_cash = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::CashAndDeposits,
                    )?;
                    let sov_rev = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::TaxAndFeeRevenue,
                    )?;
                    entry = entry
                        .debit(seller_tax, sovereign_rake)
                        .debit(sov_cash, sovereign_rake)
                        .credit(sov_rev, sovereign_rake);
                }
                self.ledger.post_entry(&entry)?;
                self.current_month_gdp_accumulator = self
                    .current_month_gdp_accumulator
                    .checked_add(trade_notional)?;
            }
        }

        Ok(())
    }

    /// Advances the world by 1 Fast Tick (1 simulated 30/360 day), executing all 5 intraday sub-phases
    /// and asserting global double-entry trial balance and WAC inventory book-value conservation.
    pub fn step_fast_tick(&mut self, pending_commands: &[PlayerCommand]) -> Result<(), CoreError> {
        let sequenced: Vec<(u32, PlayerCommand)> = pending_commands
            .iter()
            .enumerate()
            .map(|(idx, cmd)| (u32::try_from(idx).unwrap_or(0), cmd.clone()))
            .collect();
        self.step_fast_tick_sequenced(&sequenced)
    }

    /// Advances the world by 1 Fast Tick (1 simulated 30/360 day) with explicit `(seq_no, PlayerCommand)` pairs.
    pub fn step_fast_tick_sequenced(
        &mut self,
        pending_commands: &[(u32, PlayerCommand)],
    ) -> Result<(), CoreError> {
        let next_tick = self.current_tick.next();
        self.current_tick = next_tick;
        self.occ_version = self.occ_version.saturating_add(1);

        if (next_tick.0 - 1).is_multiple_of(30) {
            self.current_month_gdp_accumulator = Money::ZERO;
            for (_, corp) in &mut self.corporations {
                corp.monthly_units_output = 0;
                corp.monthly_units_sold = 0;
                corp.monthly_genuine_b2c_revenue = Money::ZERO;
                corp.monthly_laundered_cash = Money::ZERO;
            }
        }

        // 0. Drain pending commands at start of day with explicit seq_no
        for (seq_no, cmd) in pending_commands {
            self.execute_command_sequenced(cmd.clone(), *seq_no)?;
        }

        // Sub-Phase 1: Morning Logistics, B2B Contract Deliveries & Wholesale Replenishment
        self.run_subphase_1_b2b_and_wholesale()?;

        // Sub-Phase 2: Factory DAG Production & Resource Extraction
        self.run_subphase_2_production()?;

        // Sub-Phase 3: Commodity, Equity, DR & ServiceCredit CLOB Matching & OHLCV Finalization
        self.run_subphase_3_clob_matching()?;

        // Sub-Phase 4: Household Stone-Geary + Padé (2,2) MNL Retail Purchasing
        self.run_subphase_4_retail_consumption()?;

        // Sub-Phase 5: Payroll, 30/360 Interest Accrual, Dividends, Sovereign Recycling & Audit
        self.run_subphase_5_settlement_and_audit()?;

        Ok(())
    }

    fn run_subphase_1_b2b_and_wholesale(&mut self) -> Result<(), CoreError> {
        let tick = self.current_tick.0;

        // 1A. Deliver scheduled bilateral B2B contract tranches & pay logistics freight tariffs
        let logistics_corps: Vec<(CorpId, EntityId, Money)> = self
            .corporations
            .iter()
            .filter_map(|(cid, c)| match c.role {
                CorporationRole::LogisticsTrucking { .. } if c.posted_selling_price.is_positive() => {
                    Some((cid, c.entity_id, c.posted_selling_price))
                }
                _ => None,
            })
            .collect();

        let mut active_contracts = Vec::with_capacity(self.b2b_contracts.len());
        let contracts_snapshot = std::mem::take(&mut self.b2b_contracts);

        for mut contract in contracts_snapshot {
            if contract.remaining_days == 0 {
                continue;
            }
            let delivered = self.settle_b2b_transfer(
                tick,
                contract.supplier_corp_id,
                contract.buyer_corp_id,
                contract.sku_id,
                contract.daily_qty,
                contract.unit_price,
            )?;
            if delivered > 0 && !logistics_corps.is_empty() {
                let buyer_entity = self
                    .corporations
                    .get(contract.buyer_corp_id)
                    .map(|c| c.entity_id);
                if let Some(b_ent) = buyer_entity {
                    let b_avail = self.ledger.available_cash(b_ent)?;
                    for &(log_cid, log_ent, unit_tariff) in &logistics_corps {
                        if log_ent == b_ent {
                            continue;
                        }
                        let fee = unit_tariff.checked_mul_u64(delivered)?;
                        if fee.is_positive() && b_avail >= fee.checked_mul_u64(4)? {
                            let b_exp = self.ledger.entity_account_id(
                                b_ent,
                                StandardAccountCode::SecurityAndFreightExpense,
                            )?;
                            let b_cash = self
                                .ledger
                                .entity_account_id(b_ent, StandardAccountCode::CashAndDeposits)?;
                            let l_cash = self
                                .ledger
                                .entity_account_id(log_ent, StandardAccountCode::CashAndDeposits)?;
                            let l_rev = self
                                .ledger
                                .entity_account_id(log_ent, StandardAccountCode::SalesRevenue)?;
                            let entry =
                                JournalEntry::new(tick, "B2B Logistics Freight Tariff Settlement")
                                    .debit(b_exp, fee)
                                    .credit(b_cash, fee)
                                    .debit(l_cash, fee)
                                    .credit(l_rev, fee);
                            self.ledger.post_entry(&entry)?;
                            if let Some(lc) = self.corporations.get_mut(log_cid) {
                                lc.monthly_units_sold =
                                    lc.monthly_units_sold.saturating_add(delivered);
                            }
                            break;
                        }
                    }
                }
            }
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
        let (buyer_entity, working_floor) = {
            let buyer = self
                .corporations
                .get(buyer_id)
                .ok_or(CoreError::UnknownCorporation(buyer_id))?;
            let debt = self
                .bank
                .loans
                .get(&buyer.entity_id)
                .map_or(Money::ZERO, |l| l.principal_outstanding);
            let eff_rate = self
                .bank
                .default_loan_rate_bps
                .saturating_add(buyer.credit_rating.spread_bps());
            let floor = buyer.working_capital_floor(20, debt, eff_rate)?;
            (buyer.entity_id, floor)
        };

        let buyer_cash = self.ledger.available_cash(buyer_entity)?;
        let spendable_cash = if buyer_cash > working_floor {
            buyer_cash.checked_sub(working_floor)?
        } else {
            Money::ZERO
        };
        let affordable_i128 = spendable_cash.checked_div_euclid(unit_price.cents())?.cents();
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
        self.current_month_gdp_accumulator = self
            .current_month_gdp_accumulator
            .checked_add(trade_notional)?;
        Ok(actual_qty)
    }

    fn run_subphase_2_production(&mut self) -> Result<(), CoreError> {
        let tick = self.current_tick.0;
        let Some((wage_entity, _)) = self.wage_and_manager_entities() else {
            return Ok(());
        };

        let corp_ids: Vec<CorpId> = self.corporations.keys().collect();

        for corp_id in corp_ids {
            let (entity_id, role, raw_capacity, daily_wage_bill, degraded_until) = {
                let Some(corp) = self.corporations.get(corp_id) else {
                    continue;
                };
                (
                    corp.entity_id,
                    corp.role,
                    corp.effective_daily_capacity(),
                    corp.daily_wage_bill,
                    corp.ransomware_degraded_until_tick,
                )
            };
            let daily_capacity = if degraded_until > tick {
                ((raw_capacity.saturating_mul(60)) / 100).max(1)
            } else {
                raw_capacity
            };

            match role {
                CorporationRole::ResourceExtractor { output_sku } => {
                    // Extractor capitalizes daily extraction labor directly into InventoryRaw
                    // while respecting a 6-day raw-material stockpile cap and 5-day survival floor
                    let current_stock = self
                        .corporations
                        .get(corp_id)
                        .map_or(0, |c| c.inventory_lot(output_sku).total_qty());
                    let max_stock = daily_capacity.saturating_mul(6);
                    let survival_floor = self.corporations.get(corp_id).map_or(Money::ZERO, |c| {
                        c.daily_wage_bill
                            .checked_add(c.daily_manager_bill)
                            .unwrap_or(Money::ZERO)
                            .checked_mul_u64(5)
                            .unwrap_or(Money::ZERO)
                    });
                    let avail_cash = self.ledger.available_cash(entity_id)?;
                    if daily_capacity > 0
                        && current_stock < max_stock
                        && avail_cash >= daily_wage_bill.checked_add(survival_floor)?
                        && daily_wage_bill.is_positive()
                    {
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
                            corp.facility_condition_bps = corp
                                .facility_condition_bps
                                .saturating_sub(3)
                                .max(3_500);
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
                            corp.facility_condition_bps = corp
                                .facility_condition_bps
                                .saturating_sub(3)
                                .max(3_500);
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
                CorporationRole::Retailer { .. }
                | CorporationRole::LogisticsTrucking { .. }
                | CorporationRole::PrivateSecurity { .. } => {}
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
                InstrumentKind::Equity { .. }
                | InstrumentKind::DepositaryReceipt { .. }
                | InstrumentKind::ServiceCredit => None,
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
                        let safe_ask = self
                            .order_books
                            .get(inst_id)
                            .map_or(ask_price, |b| b.clamp_luld_price(ask_price));
                        if fresh_avail > 0 && safe_ask.is_positive() {
                            self.submit_clob_limit_order(
                                entity_id,
                                inst_id,
                                OrderSide::Ask,
                                safe_ask,
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
                        let safe_bid = self
                            .order_books
                            .get(inst_id)
                            .map_or(bid_price, |b| b.clamp_luld_price(bid_price));
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
                            let debt = self
                                .bank
                                .loans
                                .get(&entity_id)
                                .map_or(Money::ZERO, |l| l.principal_outstanding);
                            let working_floor = match self.corporations.get(corp_id) {
                                Some(c) => {
                                    let eff_rate = self
                                        .bank
                                        .default_loan_rate_bps
                                        .saturating_add(c.credit_rating.spread_bps());
                                    c.working_capital_floor(20, debt, eff_rate)?
                                }
                                None => Money::ZERO,
                            };
                            let free_cash = if avail_cash > working_floor {
                                avail_cash.checked_sub(working_floor)?
                            } else {
                                Money::ZERO
                            };
                            let bid_budget = Money::from_cents(free_cash.cents() / 2);
                            let affordable_i128 =
                                bid_budget.checked_div_euclid(safe_bid.cents())?.cents();
                            let affordable = u64::try_from(affordable_i128).unwrap_or(0);
                            let order_qty = needed.min(affordable);
                            if order_qty > 0 {
                                self.submit_clob_limit_order(
                                    entity_id,
                                    inst_id,
                                    OrderSide::Bid,
                                    safe_bid,
                                    order_qty,
                                )?;
                            }
                        }
                    }
                }
            }
        }

        // 3. Closed-loop Secondary Equity CLOB Market-Making & Portfolio Rebalancing
        self.run_subphase_3_equity_market_making()?;

        // 4. Sovereign Depositary Receipt (`GOOGL-DR`, `META-DR`, `NVDA-DR`) & `ServiceCredit` (`PLEX`) Market-Making
        self.run_subphase_3_depositary_and_sc_market_making()?;

        // 5. Physical Short-Locate 115% Maintenance Margin Check & Forced Buy-Ins (Guard #2)
        self.run_subphase_3_short_maintenance_margin()?;

        // Finalize daily OHLCV bar across all order books
        let tick = self.current_tick.0;
        for (_, book) in &mut self.order_books {
            book.finalize_daily_bar(tick);
        }

        Ok(())
    }

    /// Computes a corporation's fundamental per-share valuation from its trailing earnings yield,
    /// dividend yield, and GAAP book value per share.
    pub fn compute_fundamental_share_valuation(&self, corp_id: CorpId) -> Result<Money, CoreError> {
        let corp = self
            .corporations
            .get(corp_id)
            .ok_or(CoreError::UnknownCorporation(corp_id))?;
        let shares = i128::from(corp.total_shares_outstanding.max(1));
        let report = FinancialStatementReport::generate(
            &self.ledger,
            corp.entity_id,
            self.current_tick.0,
        )?;

        let book_value_cents = (report.total_equity_including_net_income.cents() / shares).max(100);
        // Include physical productive capacity replacement value ($6,500 per daily capacity unit, scaled by facility condition) in intrinsic asset backing
        let capacity_value_cents =
            (i128::from(corp.effective_daily_capacity()) * 650_000) / shares;
        let intrinsic_asset_cents = book_value_cents.saturating_add(capacity_value_cents);

        let quarter_days = i128::from(((self.current_tick.0.saturating_sub(1)) % 90) + 1);
        let annualized_ni_cents = (report.net_income.cents().max(0) * 360) / quarter_days;
        // 12x P/E multiple on annualized net income per share (floored at 3.5x intrinsic asset value during ramp-up)
        let ramp_floor_cents = (intrinsic_asset_cents * 35) / 10;
        let earnings_value_cents = ((annualized_ni_cents * 12) / shares).max(ramp_floor_cents);

        let annualized_div_cents = corp.last_monthly_dividend_paid.cents().max(0) * 12;
        // 5% target dividend yield -> 20x annualized dividend per share (floored at 3.5x intrinsic asset value prior to first payout)
        let div_value_cents = ((annualized_div_cents * 20) / shares).max(ramp_floor_cents);

        let blended_cents =
            (intrinsic_asset_cents * 40 + earnings_value_cents * 40 + div_value_cents * 20) / 100;
        Ok(Money::from_cents(blended_cents.clamp(800, 100_000)))
    }

    fn run_subphase_3_equity_market_making(&mut self) -> Result<(), CoreError> {
        let Some((wage_entity, mgr_entity, cap_entity)) = self.all_cohort_entities() else {
            return Ok(());
        };
        let (wage_floor, mgr_floor, cap_floor) = {
            let mut wf = Money::ZERO;
            let mut mf = Money::ZERO;
            let mut cf = Money::ZERO;
            for (_, c) in &self.cohorts {
                match c.kind {
                    HouseholdCohortKind::WageEarners => wf = c.savings_subsistence_floor,
                    HouseholdCohortKind::ManagersSkilled => mf = c.savings_subsistence_floor,
                    HouseholdCohortKind::CapitalistsExecutives => cf = c.savings_subsistence_floor,
                }
            }
            (wf, mf, cf)
        };

        let equity_books: Vec<(InstrumentId, CorpId)> = self
            .order_books
            .iter()
            .filter_map(|(inst_id, book)| match book.kind {
                InstrumentKind::Equity { corp_id } => Some((inst_id, corp_id)),
                InstrumentKind::Commodity { .. }
                | InstrumentKind::DepositaryReceipt { .. }
                | InstrumentKind::ServiceCredit => None,
            })
            .collect();

        let num_books = i128::try_from(equity_books.len().max(1)).unwrap_or(6);
        let tick = self.current_tick.0;

        for (book_idx, (inst_id, corp_id)) in equity_books.into_iter().enumerate() {
            // Cancel prior household quotes on this equity book to release cash/share reservations cleanly
            for holder_ent in [cap_entity, mgr_entity, wage_entity] {
                self.cancel_all_orders_for_entity_on_book(inst_id, holder_ent)?;
            }

            let fair_val = self.compute_fundamental_share_valuation(corp_id)?;
            let last_px = self
                .order_books
                .get(inst_id)
                .map_or(fair_val, |b| b.last_price);
            // Smooth anchor price: 50% fundamental value + 50% trailing market price
            let anchor_cents = i128::midpoint(fair_val.cents(), last_px.cents()).max(200);
            if let Some(book) = self.order_books.get_mut(inst_id) {
                book.luld_reference_price = Money::from_cents(anchor_cents);
            }

            let raw_bid_l1 = Money::from_cents(((anchor_cents * 98) / 100).max(100));
            let raw_bid_l2 = Money::from_cents(((anchor_cents * 95) / 100).max(100));
            let raw_ask_l1 = Money::from_cents(((anchor_cents * 102) / 100).max(raw_bid_l1.cents() + 10));
            let raw_ask_l2 = Money::from_cents(((anchor_cents * 105) / 100).max(raw_ask_l1.cents() + 10));
            let (bid_l1, bid_l2, ask_l1, ask_l2) = self.order_books.get(inst_id).map_or(
                (raw_bid_l1, raw_bid_l2, raw_ask_l1, raw_ask_l2),
                |b| {
                    (
                        b.clamp_luld_price(raw_bid_l1),
                        b.clamp_luld_price(raw_bid_l2),
                        b.clamp_luld_price(raw_ask_l1),
                        b.clamp_luld_price(raw_ask_l2),
                    )
                },
            );

            // 1. Capitalists & Executives Cohort posts two-sided market-making depth (Bids + Asks)
            let cap_avail_shares = self
                .equity_positions
                .get(&(corp_id, cap_entity))
                .map_or(0, |p| p.available_shares());
            if cap_avail_shares >= 40 {
                let ask_qty_1 = (cap_avail_shares / 250).clamp(10, 150);
                let ask_qty_2 = (cap_avail_shares / 200).clamp(10, 200);
                if ask_qty_1 + ask_qty_2 <= cap_avail_shares {
                    self.submit_clob_limit_order(
                        cap_entity,
                        inst_id,
                        OrderSide::Ask,
                        ask_l1,
                        ask_qty_1,
                    )?;
                    self.submit_clob_limit_order(
                        cap_entity,
                        inst_id,
                        OrderSide::Ask,
                        ask_l2,
                        ask_qty_2,
                    )?;
                }
            }

            let cap_avail_cash = self.ledger.available_cash(cap_entity)?;
            let cap_reserve = cap_floor.checked_mul_u64(2)?;
            if cap_avail_cash > cap_reserve {
                let per_book_budget = Money::from_cents(
                    cap_avail_cash.checked_sub(cap_reserve)?.cents() / (num_books * 8),
                );
                let b1_budget = Money::from_cents(per_book_budget.cents() / 2);
                let b2_budget = Money::from_cents(per_book_budget.cents() / 2);

                let b1_qty = u64::try_from(
                    b1_budget.checked_div_euclid(bid_l1.cents())?.cents(),
                )
                .unwrap_or(0)
                .min(150);
                if b1_qty >= 5 {
                    self.submit_clob_limit_order(
                        cap_entity,
                        inst_id,
                        OrderSide::Bid,
                        bid_l1,
                        b1_qty,
                    )?;
                }

                let b2_qty = u64::try_from(
                    b2_budget.checked_div_euclid(bid_l2.cents())?.cents(),
                )
                .unwrap_or(0)
                .min(200);
                if b2_qty >= 5 {
                    self.submit_clob_limit_order(
                        cap_entity,
                        inst_id,
                        OrderSide::Bid,
                        bid_l2,
                        b2_qty,
                    )?;
                }
            }

            // 2. Managers & Skilled Cohort rebalances equity portfolio against Capitalist quotes
            let idx_u64 = u64::try_from(book_idx).unwrap_or(0);
            let cycle = (tick.wrapping_add(idx_u64 * 5)) % 6;
            if fair_val >= last_px || cycle < 4 {
                // Managers accumulate undervalued / high-yield shares by lifting Asks or posting Bids
                let mgr_cash = self.ledger.available_cash(mgr_entity)?;
                let mgr_reserve = mgr_floor.checked_mul_u64(2)?;
                if mgr_cash > mgr_reserve {
                    let alloc = Money::from_cents(
                        mgr_cash.checked_sub(mgr_reserve)?.cents() / (num_books * 16),
                    );
                    let buy_px = if cycle < 3 { ask_l1 } else { bid_l1 };
                    let qty = u64::try_from(alloc.checked_div_euclid(buy_px.cents())?.cents())
                        .unwrap_or(0)
                        .min(60);
                    if qty >= 2 {
                        self.submit_clob_limit_order(
                            mgr_entity,
                            inst_id,
                            OrderSide::Bid,
                            buy_px,
                            qty,
                        )?;
                    }
                }
            } else {
                // Managers trim appreciated positions by hitting Bids or posting Asks
                let mgr_shares = self
                    .equity_positions
                    .get(&(corp_id, mgr_entity))
                    .map_or(0, |p| p.available_shares());
                if mgr_shares >= 100 {
                    let sell_qty = (mgr_shares / 300).clamp(2, 40);
                    let sell_px = ask_l1;
                    self.submit_clob_limit_order(
                        mgr_entity,
                        inst_id,
                        OrderSide::Ask,
                        sell_px,
                        sell_qty,
                    )?;
                }
            }

            // 3. Wage Earners Cohort invests excess savings or takes profits periodically
            if (tick.wrapping_add(idx_u64)) % 5 == 0 {
                let wage_cash = self.ledger.available_cash(wage_entity)?;
                let wage_reserve = wage_floor.checked_mul_u64(3)?;
                if wage_cash > wage_reserve {
                    let alloc = Money::from_cents(
                        wage_cash.checked_sub(wage_reserve)?.cents() / (num_books * 30),
                    );
                    let qty = u64::try_from(alloc.checked_div_euclid(ask_l1.cents())?.cents())
                        .unwrap_or(0)
                        .min(25);
                    if qty >= 1 {
                        self.submit_clob_limit_order(
                            wage_entity,
                            inst_id,
                            OrderSide::Bid,
                            ask_l1,
                            qty,
                        )?;
                    }
                }
            }
        }

        Ok(())
    }

    fn run_subphase_3_depositary_and_sc_market_making(&mut self) -> Result<(), CoreError> {
        let sov_id = self.sovereign_entity_id;
        let dr_books: Vec<InstrumentId> = self
            .order_books
            .iter()
            .filter_map(|(id, b)| match b.kind {
                InstrumentKind::DepositaryReceipt { .. } => Some(id),
                _ => None,
            })
            .collect();

        for inst_id in dr_books {
            self.cancel_all_orders_for_entity_on_book(inst_id, sov_id)?;
            let ref_px = self
                .order_books
                .get(inst_id)
                .map_or(Money::from_dollars(100), |b| b.luld_reference_price);
            let bid_px = Money::from_cents(((ref_px.cents() * 99) / 100).max(100));
            let ask_px = Money::from_cents(((ref_px.cents() * 101) / 100).max(bid_px.cents() + 10));

            // Ensure SovereignTreasury has at least $150k liquidity via its endogenous repo line at CommercialBankFacility
            let avail_cash = self.ledger.available_cash(sov_id)?;
            let min_buffer = Money::from_dollars(150_000);
            if avail_cash < min_buffer {
                let top_up = min_buffer.checked_sub(avail_cash)?;
                self.bank.originate_loan(
                    &mut self.ledger,
                    self.current_tick.0,
                    sov_id,
                    top_up,
                    Some(200),
                )?;
            }

            let sov_dr_avail = self
                .depositary_positions
                .get(&(inst_id, sov_id))
                .map_or(0, |p| p.available_shares());
            if sov_dr_avail >= 50 {
                let ask_qty = (sov_dr_avail / 100).clamp(10, 200);
                self.submit_clob_limit_order(sov_id, inst_id, OrderSide::Ask, ask_px, ask_qty)?;
            }

            let fresh_cash = self.ledger.available_cash(sov_id)?;
            let bid_budget = Money::from_cents(fresh_cash.cents() / 16);
            let bid_qty = u64::try_from(bid_budget.checked_div_euclid(bid_px.cents())?.cents())
                .unwrap_or(0)
                .min(100);
            if bid_qty >= 5 {
                self.submit_clob_limit_order(sov_id, inst_id, OrderSide::Bid, bid_px, bid_qty)?;
            }
        }

        // ServiceCredit (`PLEX SC`) market-making by CapitalistsExecutives
        if let Some((_, _, cap_entity)) = self.all_cohort_entities() {
            let sc_books: Vec<InstrumentId> = self
                .order_books
                .iter()
                .filter_map(|(id, b)| match b.kind {
                    InstrumentKind::ServiceCredit => Some(id),
                    _ => None,
                })
                .collect();
            for inst_id in sc_books {
                self.cancel_all_orders_for_entity_on_book(inst_id, cap_entity)?;
                let ref_px = self
                    .order_books
                    .get(inst_id)
                    .map_or(Money::from_dollars(1_000), |b| b.luld_reference_price);
                let bid_px = Money::from_cents(((ref_px.cents() * 98) / 100).max(1_000));
                let ask_px = Money::from_cents(((ref_px.cents() * 102) / 100).max(bid_px.cents() + 100));

                let cap_sc = self
                    .service_credit_positions
                    .get(&cap_entity)
                    .map_or(0, |p| p.available_credits());
                if cap_sc >= 20 {
                    let ask_qty = (cap_sc / 25).clamp(2, 25);
                    self.submit_clob_limit_order(
                        cap_entity,
                        inst_id,
                        OrderSide::Ask,
                        ask_px,
                        ask_qty,
                    )?;
                }
                let cap_cash = self.ledger.available_cash(cap_entity)?;
                if cap_cash > Money::from_dollars(900_000) {
                    self.submit_clob_limit_order(cap_entity, inst_id, OrderSide::Bid, bid_px, 5)?;
                }
            }
        }

        Ok(())
    }

    /// Guard #2: Checks 115% maintenance margin on all open `ShortPositionRecord`s and triggers
    /// an automatic CLOB Forced Buy-In when `collateral_reserved_cash * 10_000 < mark_liability * 11_500`.
    fn run_subphase_3_short_maintenance_margin(&mut self) -> Result<(), CoreError> {
        let loan_ids: Vec<ShortLoanId> = self.short_positions.keys().copied().collect();
        for loan_id in loan_ids {
            let Some((corp_id, borrowed_shares, collateral)) = self
                .short_positions
                .get(&loan_id)
                .map(|s| (s.corp_id, s.borrowed_shares, s.collateral_reserved_cash))
            else {
                continue;
            };
            let mark_price = self
                .corporations
                .get(corp_id)
                .and_then(|c| c.equity_instrument_id)
                .and_then(|id| self.order_books.get(id))
                .map_or(Money::from_dollars(25), |b| b.last_price);
            let mark_liability = mark_price.checked_mul_u64(borrowed_shares)?;
            let (maint_required, _) = mark_liability.mul_div_rem_u64(11_500, BPS_DENOMINATOR)?;
            if collateral < maint_required {
                let max_buy = Money::from_cents(mark_price.cents().saturating_mul(2));
                self.execute_cover_short_position(loan_id, max_buy)?;
            }
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
                        let effective_brand = if c.cloud_tier == CloudTier::Offline {
                            (c.brand_score_bps / 2).max(1_000)
                        } else {
                            c.brand_score_bps
                        };
                        Some(RetailStoreOffer {
                            seller_corp_id: corp_id,
                            unit_price: c.posted_selling_price,
                            quality_score_bps: c.quality_score_bps,
                            brand_score_bps: effective_brand,
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
                    seller.monthly_genuine_b2c_revenue = seller
                        .monthly_genuine_b2c_revenue
                        .checked_add(settlement.gross_revenue)?;
                    seller.facility_condition_bps = seller
                        .facility_condition_bps
                        .saturating_sub(1)
                        .max(3_500);
                }
                self.current_month_gdp_accumulator = self
                    .current_month_gdp_accumulator
                    .checked_add(settlement.gross_revenue)?;

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

        // 5A. Pay daily non-capitalized wages (Retailers, Logistics, and Security pay WageEarners;
        // all corporations pay ManagersSkilled only when operating above their 5-day survival reserve)
        for &corp_id in &corp_ids {
            let (entity_id, role, wage_bill, mgr_bill, has_stock_or_sales) = {
                let Some(c) = self.corporations.get(corp_id) else {
                    continue;
                };
                let active_ops = match c.role {
                    CorporationRole::Retailer { retail_sku } => {
                        c.inventory_lot(retail_sku).total_qty() > 0 || c.monthly_units_sold > 0
                    }
                    CorporationRole::Manufacturer { output_sku, .. }
                    | CorporationRole::ResourceExtractor { output_sku } => {
                        c.inventory_lot(output_sku).total_qty() > 0 || c.monthly_units_output > 0
                    }
                    CorporationRole::LogisticsTrucking { .. }
                    | CorporationRole::PrivateSecurity { .. } => true,
                };
                (
                    c.entity_id,
                    c.role,
                    c.daily_wage_bill,
                    c.daily_manager_bill,
                    active_ops,
                )
            };

            let survival_floor = wage_bill.checked_add(mgr_bill)?.checked_mul_u64(5)?;

            if matches!(
                role,
                CorporationRole::Retailer { .. }
                    | CorporationRole::LogisticsTrucking { .. }
                    | CorporationRole::PrivateSecurity { .. }
            ) && wage_bill.is_positive()
            {
                let avail = self.ledger.available_cash(entity_id)?;
                if avail > survival_floor {
                    let effective_wage = if has_stock_or_sales {
                        wage_bill
                    } else {
                        Money::from_cents(wage_bill.cents() / 4)
                    };
                    let pay = effective_wage.min(avail.checked_sub(survival_floor)?);
                    if pay.is_positive() {
                        self.post_payroll_transfer(tick, entity_id, wage_entity, pay)?;
                    }
                }
            }

            if mgr_bill.is_positive() {
                let avail = self.ledger.available_cash(entity_id)?;
                if avail > survival_floor {
                    let effective_mgr = if has_stock_or_sales {
                        mgr_bill
                    } else {
                        Money::from_cents(mgr_bill.cents() / 4)
                    };
                    let pay = effective_mgr.min(avail.checked_sub(survival_floor)?);
                    if pay.is_positive() {
                        self.post_payroll_transfer(tick, entity_id, mgr_entity, pay)?;
                    }
                }
            }
        }

        // 5B. Daily 30/360 Loan Interest, Deposit Interest, Short-Locate Borrow Fees & Tax Lien Garnishment
        for &corp_id in &corp_ids {
            let Some((entity_id, survival_floor)) = self.corporations.get(corp_id).and_then(|c| {
                let floor = c
                    .daily_wage_bill
                    .checked_add(c.daily_manager_bill)
                    .ok()?
                    .checked_mul_u64(5)
                    .ok()?;
                Some((c.entity_id, floor))
            }) else {
                continue;
            };
            if let Some(eq_id) = self
                .corporations
                .get(corp_id)
                .and_then(|c| c.equity_instrument_id)
            {
                let avail = self.ledger.available_cash(entity_id)?;
                if avail < survival_floor {
                    self.cancel_all_orders_for_entity_on_book(eq_id, entity_id)?;
                }
            }
            self.bank
                .accrue_and_collect_daily_loan_interest_with_floor(
                    &mut self.ledger,
                    tick,
                    entity_id,
                    survival_floor,
                )?;
            self.bank
                .accrue_and_pay_daily_deposit_interest(&mut self.ledger, tick, entity_id)?;
            self.garnish_tax_lien_if_any(tick, entity_id, survival_floor)?;
        }

        for (_, cohort) in &self.cohorts {
            self.bank.accrue_and_pay_daily_deposit_interest(
                &mut self.ledger,
                tick,
                cohort.entity_id,
            )?;
        }

        self.accrue_daily_short_borrow_fees(tick)?;

        // 5C. Monthly SovereignCloud Billing, Dividend Distribution, Bank & Sovereign Fiscal Recycling & Credit Rating Review (at Day 30)
        if self.current_tick.is_month_end() {
            self.trailing_30d_gdp = self.current_month_gdp_accumulator;
            self.bill_monthly_sovereign_cloud(tick)?;
            self.recycle_monthly_bank_net_income(tick)?;
            self.recycle_monthly_sovereign_surplus(tick)?;
            for &corp_id in &corp_ids {
                self.distribute_monthly_dividend(tick, corp_id)?;
            }
            self.update_monthly_credit_ratings(tick)?;
        }

        // 5D. Quarterly Depositary Dividends & Income Statement Closing into Retained Earnings (every 90 ticks)
        if self.current_tick.is_quarter_end() {
            self.distribute_quarterly_depositary_dividends(tick)?;
            let bank_id = self.bank.bank_id()?;
            self.ledger
                .close_income_statement_for_entity(tick, bank_id)?;
            self.ledger
                .close_income_statement_for_entity(tick, self.sovereign_entity_id)?;
            for &corp_id in &corp_ids {
                if let Some((entity_id, ticker)) = self
                    .corporations
                    .get(corp_id)
                    .map(|c| (c.entity_id, c.ticker.clone()))
                {
                    let q_report =
                        FinancialStatementReport::generate(&self.ledger, entity_id, tick)?;
                    self.push_news(
                        &ticker,
                        NewsCategory::QuarterlyEarnings,
                        format!(
                            "{ticker} reports Q{} earnings: Net Income {}, Equity {}",
                            self.current_tick.to_date360().quarter(),
                            q_report.net_income,
                            q_report.total_equity_including_net_income
                        ),
                    );
                    self.ledger
                        .close_income_statement_for_entity(tick, entity_id)?;
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

    /// Recycles 85% of positive monthly `Central & Commercial Bank` net interest income back into
    /// household deposits (`35%` as bank staff salaries to `ManagersSkilled`, `50%` as bank dividends
    /// to `CapitalistsExecutives`), preventing debt-service spreads from permanently shrinking M1.
    fn recycle_monthly_bank_net_income(&mut self, tick: u64) -> Result<(), CoreError> {
        let bank_id = self.bank.bank_id()?;
        let report = FinancialStatementReport::generate(&self.ledger, bank_id, tick)?;
        if !report.net_income.is_positive() {
            return Ok(());
        }

        let Some((_, mgr_entity, cap_entity)) = self.all_cohort_entities() else {
            return Ok(());
        };

        let (staff_wages, _) = report.net_income.mul_div_rem_u64(3_500, BPS_DENOMINATOR)?;
        let (bank_dividends, _) = report.net_income.mul_div_rem_u64(5_000, BPS_DENOMINATOR)?;

        let bank_wage_exp = self
            .ledger
            .entity_account_id(bank_id, StandardAccountCode::WageExpense)?;
        let bank_re = self
            .ledger
            .entity_account_id(bank_id, StandardAccountCode::RetainedEarnings)?;
        let bank_dep_liab = self
            .ledger
            .entity_account_id(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
        let mgr_cash = self
            .ledger
            .entity_account_id(mgr_entity, StandardAccountCode::CashAndDeposits)?;
        let mgr_rev = self
            .ledger
            .entity_account_id(mgr_entity, StandardAccountCode::SalesRevenue)?;
        let cap_cash = self
            .ledger
            .entity_account_id(cap_entity, StandardAccountCode::CashAndDeposits)?;
        let cap_div = self
            .ledger
            .entity_account_id(cap_entity, StandardAccountCode::DividendIncome)?;

        let mut entry = JournalEntry::new(tick, "Monthly Bank Net Interest Income Recycling");
        let mut posted_any = false;
        if staff_wages.is_positive() {
            entry = entry
                .debit(bank_wage_exp, staff_wages)
                .credit(bank_dep_liab, staff_wages)
                .debit(mgr_cash, staff_wages)
                .credit(mgr_rev, staff_wages);
            posted_any = true;
        }
        if bank_dividends.is_positive() {
            entry = entry
                .debit(bank_re, bank_dividends)
                .credit(bank_dep_liab, bank_dividends)
                .debit(cap_cash, bank_dividends)
                .credit(cap_div, bank_dividends);
            posted_any = true;
        }

        if posted_any {
            self.ledger.post_entry(&entry)?;
        }
        Ok(())
    }

    fn distribute_monthly_dividend(&mut self, tick: u64, corp_id: CorpId) -> Result<(), CoreError> {
        let (entity_id, ticker, payout_bps, working_floor, idle_cash_ceiling, total_shares) = {
            let Some(c) = self.corporations.get(corp_id) else {
                return Ok(());
            };
            let debt = self
                .bank
                .loans
                .get(&c.entity_id)
                .map_or(Money::ZERO, |l| l.principal_outstanding);
            let eff_rate = self
                .bank
                .default_loan_rate_bps
                .saturating_add(c.credit_rating.spread_bps());
            let floor = c.working_capital_floor(20, debt, eff_rate)?;
            let ceiling = c.working_capital_floor(180, debt, eff_rate)?;
            (
                c.entity_id,
                c.ticker.clone(),
                c.target_dividend_payout_bps,
                floor,
                ceiling,
                c.total_shares_outstanding.max(1),
            )
        };

        let avail_cash = self.ledger.available_cash(entity_id)?;
        if avail_cash <= working_floor {
            return Ok(());
        }

        let report = FinancialStatementReport::generate(&self.ledger, entity_id, tick)?;
        let regular_div = if report.net_income.is_positive() && payout_bps > 0 {
            let (d, _) = report
                .net_income
                .mul_div_rem_u64(u64::from(payout_bps), BPS_DENOMINATOR)?;
            d
        } else {
            Money::ZERO
        };

        // Mandatory excess-cash debt amortization & dividend sweep:
        // If available cash exceeds 180 days of operating expenses, first pay down up to 50% of excess toward
        // any outstanding bank debt, then distribute 50% of remaining excess above the 180-day ceiling to shareholders.
        let excess_hoard_sweep = if avail_cash > idle_cash_ceiling {
            let excess = avail_cash.checked_sub(idle_cash_ceiling)?;
            let debt_paydown = Money::from_cents(excess.cents() / 2);
            let repaid = if debt_paydown.is_positive() {
                self.bank
                    .repay_loan_principal(&mut self.ledger, tick, entity_id, debt_paydown)?
            } else {
                Money::ZERO
            };
            let remaining_excess = excess.checked_sub(repaid)?;
            Money::from_cents(remaining_excess.cents() / 2)
        } else {
            Money::ZERO
        };

        let fresh_avail = self.ledger.available_cash(entity_id)?;
        if fresh_avail <= working_floor {
            return Ok(());
        }
        let fresh_distributable = fresh_avail.checked_sub(working_floor)?;

        let desired_div = regular_div.max(excess_hoard_sweep);
        let total_dividend = desired_div.min(fresh_distributable);
        if let Some(c) = self.corporations.get_mut(corp_id) {
            c.last_monthly_dividend_paid = total_dividend;
        }
        if !total_dividend.is_positive() {
            return Ok(());
        }

        // Collect all shareholders of `corp_id` (note: reserved_shares still belong to `owned_shares` until fill!)
        let shareholder_weights: Vec<(EntityId, u64)> = self
            .equity_positions
            .iter()
            .filter_map(|(&(cid, holder_id), pos)| {
                if cid == corp_id && pos.owned_shares > 0 && holder_id != entity_id {
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

        if total_dividend >= Money::from_dollars(10_000) {
            self.push_news(
                &ticker,
                NewsCategory::DividendAndBuyback,
                format!(
                    "{ticker} distributes {total_dividend} monthly cash dividend to shareholders"
                ),
            );
        }

        // Guard #2: Collect Payment-in-Lieu-of-Dividend (PIL) from open `ShortPositionRecord`s on `corp_id`
        let open_shorts: Vec<ShortLoanId> = self
            .short_positions
            .iter()
            .filter_map(|(&lid, s)| (s.corp_id == corp_id).then_some(lid))
            .collect();

        for loan_id in open_shorts {
            let Some((borrower_id, lender_id, borrowed_shares, collateral)) =
                self.short_positions.get(&loan_id).map(|s| {
                    (
                        s.borrower_entity_id,
                        s.lender_entity_id,
                        s.borrowed_shares,
                        s.collateral_reserved_cash,
                    )
                })
            else {
                continue;
            };
            let (pil_due, _) =
                total_dividend.mul_div_rem_u64(borrowed_shares, u128::from(total_shares))?;
            if !pil_due.is_positive() {
                continue;
            }
            let avail_unreserved = self.ledger.available_cash(borrower_id)?;
            let pay = if avail_unreserved >= pil_due {
                pil_due
            } else {
                let shortfall = pil_due.checked_sub(avail_unreserved)?;
                let from_collat = shortfall.min(collateral);
                if from_collat.is_positive() {
                    self.ledger.release_reserved_cash(borrower_id, from_collat)?;
                    if let Some(s) = self.short_positions.get_mut(&loan_id) {
                        s.collateral_reserved_cash =
                            s.collateral_reserved_cash.checked_sub(from_collat)?;
                    }
                }
                avail_unreserved.checked_add(from_collat)?
            };
            if pay.is_positive() {
                let b_int = self
                    .ledger
                    .entity_account_id(borrower_id, StandardAccountCode::InterestExpense)?;
                let b_cash = self
                    .ledger
                    .entity_account_id(borrower_id, StandardAccountCode::CashAndDeposits)?;
                let l_cash = self
                    .ledger
                    .entity_account_id(lender_id, StandardAccountCode::CashAndDeposits)?;
                let l_div = self
                    .ledger
                    .entity_account_id(lender_id, StandardAccountCode::DividendIncome)?;
                let pil_entry =
                    JournalEntry::new(tick, "Short-Sale Payment-in-Lieu-of-Dividend")
                        .debit(b_int, pay)
                        .credit(b_cash, pay)
                        .debit(l_cash, pay)
                        .credit(l_div, pay);
                self.ledger.post_entry(&pil_entry)?;
            }
        }

        Ok(())
    }

    fn update_monthly_credit_ratings(&mut self, tick: u64) -> Result<(), CoreError> {
        let corp_ids: Vec<CorpId> = self.corporations.keys().collect();
        for corp_id in corp_ids {
            let (entity_id, ticker, old_rating, debt, floor_20d) = {
                let Some(c) = self.corporations.get(corp_id) else {
                    continue;
                };
                let debt = self
                    .bank
                    .loans
                    .get(&c.entity_id)
                    .map_or(Money::ZERO, |l| l.principal_outstanding);
                let eff_rate = self
                    .bank
                    .default_loan_rate_bps
                    .saturating_add(c.credit_rating.spread_bps());
                let floor = c.working_capital_floor(20, debt, eff_rate)?;
                (
                    c.entity_id,
                    c.ticker.clone(),
                    c.credit_rating,
                    debt,
                    floor,
                )
            };

            let avail_cash = self.ledger.available_cash(entity_id)?;
            let report = FinancialStatementReport::generate(&self.ledger, entity_id, tick)?;
            let new_rating = CreditRating::evaluate(
                avail_cash,
                floor_20d,
                debt,
                report.total_equity_including_net_income,
            );

            if let Some(c) = self.corporations.get_mut(corp_id) {
                c.credit_rating = new_rating;
            }
            let new_rate = self
                .bank
                .default_loan_rate_bps
                .saturating_add(new_rating.spread_bps());
            if let Some(loan) = self.bank.loans.get_mut(&entity_id) {
                loan.annual_rate_bps = new_rate;
            }

            if new_rating != old_rating {
                let verb = if new_rating < old_rating {
                    "UPGRADED"
                } else {
                    "DOWNGRADED"
                };
                self.push_news(
                    &ticker,
                    NewsCategory::CreditAndDebt,
                    format!(
                        "Credit Rating {verb}: {ticker} {} -> {} (Borrow Rate {}.{:02}%)",
                        old_rating.badge_label(),
                        new_rating.badge_label(),
                        new_rate / 100,
                        new_rate % 100
                    ),
                );
            }
        }
        Ok(())
    }

    fn execute_authority_mint_service_credit(
        &mut self,
        recipient_entity_id: EntityId,
        credits: u64,
        webhook_ref: String,
    ) -> Result<(), CoreError> {
        if credits == 0 || webhook_ref.trim().is_empty() {
            return Err(CoreError::InvalidOrder {
                reason: "ServiceCredit mint requires credits > 0 and non-empty webhook_ref"
                    .to_owned(),
            });
        }
        // Guard #5: Idempotent webhook reference check
        if !self.processed_webhook_refs.insert(webhook_ref) {
            return Ok(());
        }
        // Verify entity exists in ledger
        let _ = self.ledger.cash_balance(recipient_entity_id)?;
        let pos = self
            .service_credit_positions
            .entry(recipient_entity_id)
            .or_default();
        pos.owned_credits = pos.owned_credits.saturating_add(credits);
        self.total_service_credits_supply =
            self.total_service_credits_supply.saturating_add(credits);
        Ok(())
    }

    fn execute_ingest_oracle_price_tick(
        &mut self,
        instrument_id: InstrumentId,
        new_oracle_price: Money,
    ) -> Result<(), CoreError> {
        if !new_oracle_price.is_positive() {
            return Err(CoreError::InvalidOrder {
                reason: "oracle price must be strictly positive".to_owned(),
            });
        }
        let is_dr = {
            let book = self
                .order_books
                .get_mut(instrument_id)
                .ok_or(CoreError::UnknownInstrument(instrument_id))?;
            if !matches!(book.kind, InstrumentKind::DepositaryReceipt { .. }) {
                return Err(CoreError::InvalidOrder {
                    reason: "IngestOraclePriceTick only applies to DepositaryReceipt instruments"
                        .to_owned(),
                });
            }
            // Guard #3: Immediately re-center LULD reference price to new_oracle_price
            book.reanchor_luld_reference(new_oracle_price);
            true
        };
        if is_dr {
            self.run_subphase_3_depositary_and_sc_market_making()?;
        }
        Ok(())
    }

    pub fn execute_found_private_sme(
        &mut self,
        founder_entity_id: EntityId,
        ticker: &str,
        name: &str,
        role: CorporationRole,
        seed_capital: Money,
    ) -> Result<CorpId, CoreError> {
        let min_seed = Money::from_dollars(50_000);
        if seed_capital < min_seed {
            return Err(CoreError::InvalidOrder {
                reason: format!("SME seed_capital must be at least {min_seed}, got {seed_capital}"),
            });
        }
        let clean_ticker = ticker.trim().to_uppercase();
        if clean_ticker.is_empty()
            || self
                .corporations
                .values()
                .any(|c| c.ticker.eq_ignore_ascii_case(&clean_ticker))
        {
            return Err(CoreError::InvalidOrder {
                reason: format!("invalid or duplicate SME ticker '{clean_ticker}'"),
            });
        }

        let avail = self.ledger.available_cash(founder_entity_id)?;
        if avail < seed_capital {
            return Err(CoreError::InvalidOrder {
                reason: format!(
                    "founder insufficient available cash: available {avail}, requested {seed_capital}"
                ),
            });
        }

        let tick = self.current_tick.0;
        let sme_entity = self.ledger.register_entity(name, false);

        // Transfer seed capital from founder to SME (founder capitalizes CommonStock investment; SME records CommonStock equity)
        let f_eq = self
            .ledger
            .entity_account_id(founder_entity_id, StandardAccountCode::CommonStock)?;
        let f_cash = self
            .ledger
            .entity_account_id(founder_entity_id, StandardAccountCode::CashAndDeposits)?;
        let s_cash = self
            .ledger
            .entity_account_id(sme_entity, StandardAccountCode::CashAndDeposits)?;
        let s_eq = self
            .ledger
            .entity_account_id(sme_entity, StandardAccountCode::CommonStock)?;

        let entry = JournalEntry::new(tick, "Private SME Incorporation Seed Capital")
            .debit(f_eq, seed_capital)
            .credit(f_cash, seed_capital)
            .debit(s_cash, seed_capital)
            .credit(s_eq, seed_capital);
        self.ledger.post_entry(&entry)?;

        let default_price = match role {
            CorporationRole::ResourceExtractor { output_sku }
            | CorporationRole::Retailer { retail_sku: output_sku } => self
                .skus
                .get(output_sku)
                .map_or(Money::from_dollars(200), |s| s.base_reference_price),
            CorporationRole::Manufacturer { output_sku, .. } => {
                let r = self
                    .skus
                    .get(output_sku)
                    .map_or(Money::from_dollars(200), |s| s.base_reference_price);
                Money::from_cents((r.cents() * 60) / 100)
            }
            CorporationRole::LogisticsTrucking { .. } => Money::from_dollars(15),
            CorporationRole::PrivateSecurity { .. } => Money::from_dollars(2_500),
        };
        let max_bid = Money::from_cents((default_price.cents() * 80) / 100);

        let initial_shares = 100_000u64;
        let corp_id = self.corporations.insert_with_key(|cid| Corporation {
            corp_id: cid,
            entity_id: sme_entity,
            equity_instrument_id: None,
            ticker: clean_ticker,
            name: name.to_owned(),
            role,
            persona_kind: ExecutivePersonaKind::BalancedExecutive,
            delegation: AiDelegationConfig::player_default(),
            daily_capacity: 60,
            daily_wage_bill: Money::from_dollars(1_200),
            daily_manager_bill: Money::from_dollars(600),
            posted_selling_price: default_price,
            max_input_bid_price: max_bid,
            quality_score_bps: 10_000,
            brand_score_bps: 10_000,
            target_dividend_payout_bps: 2_500,
            total_shares_outstanding: initial_shares,
            last_monthly_dividend_paid: Money::ZERO,
            founded_tick: tick,
            founder_entity_id: Some(founder_entity_id),
            founder_lockup_until_tick: 0,
            founder_locked_shares: 0,
            cloud_tier: CloudTier::Tier1PosWire,
            subscribed_cloud_tier: CloudTier::Tier1PosWire,
            monthly_units_output: 0,
            monthly_units_sold: 0,
            monthly_genuine_b2c_revenue: Money::ZERO,
            monthly_laundered_cash: Money::ZERO,
            ransomware_degraded_until_tick: 0,
            security_shield_until_tick: 0,
            logistics_shield_until_tick: 0,
            facility_condition_bps: 10_000,
            credit_rating: CreditRating::AAA,
            inventory: BTreeMap::new(),
        });

        self.equity_positions.insert(
            (corp_id, founder_entity_id),
            EquityPosition {
                owned_shares: initial_shares,
                reserved_shares: 0,
            },
        );

        Ok(corp_id)
    }

    pub fn execute_launch_ipo_auction(
        &mut self,
        corp_id: CorpId,
        primary_shares_to_issue: u64,
        offer_price: Money,
    ) -> Result<InstrumentId, CoreError> {
        if primary_shares_to_issue == 0 || !offer_price.is_positive() {
            return Err(CoreError::IpoGateViolation {
                reason: "primary_shares_to_issue and offer_price must be strictly positive"
                    .to_owned(),
            });
        }
        let tick = self.current_tick.0;
        let (entity_id, ticker, founded_tick, div_bps, founder_id, already_listed) = {
            let corp = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            (
                corp.entity_id,
                corp.ticker.clone(),
                corp.founded_tick,
                corp.target_dividend_payout_bps,
                corp.founder_entity_id,
                corp.equity_instrument_id.is_some(),
            )
        };

        if already_listed {
            return Err(CoreError::IpoGateViolation {
                reason: format!("{ticker} is already publicly listed on the Equity Board"),
            });
        }
        let operating_age = tick.saturating_sub(founded_tick);
        if operating_age < 90 {
            return Err(CoreError::IpoGateViolation {
                reason: format!(
                    "{ticker} operating history ({operating_age} ticks) < 90-tick IPO minimum"
                ),
            });
        }
        if div_bps < 2_000 {
            return Err(CoreError::IpoGateViolation {
                reason: format!(
                    "{ticker} dividend policy ({div_bps} bps) < 2,000 bps (20%) IPO minimum"
                ),
            });
        }

        let report = FinancialStatementReport::generate(&self.ledger, entity_id, tick)?;
        let retained_earnings = self
            .ledger
            .entity_balance(entity_id, StandardAccountCode::RetainedEarnings)?;
        let cumulative_operating_profit = report.net_income.checked_add(retained_earnings)?;
        if !report.net_income.is_positive() && !cumulative_operating_profit.is_positive() {
            return Err(CoreError::IpoGateViolation {
                reason: format!(
                    "{ticker} trailing/cumulative operating profit ({cumulative_operating_profit}) must be strictly positive for IPO"
                ),
            });
        }
        let min_equity = Money::from_dollars(250_000);
        if report.total_equity_including_net_income < min_equity {
            return Err(CoreError::IpoGateViolation {
                reason: format!(
                    "{ticker} GAAP book equity ({}) < {min_equity} IPO gate",
                    report.total_equity_including_net_income
                ),
            });
        }

        // Lock 50% of founder's shares for 180 ticks
        let locked_shares = if let Some(fid) = founder_id {
            self.equity_positions
                .get(&(corp_id, fid))
                .map_or(0, |p| p.owned_shares / 2)
        } else {
            0
        };

        let eq_ticker = format!("{ticker} EQ");
        let inst_id = self.order_books.insert_with_key(|iid| {
            CentralLimitOrderBook::new(iid, eq_ticker, InstrumentKind::Equity { corp_id }, offer_price)
        });

        if let Some(corp) = self.corporations.get_mut(corp_id) {
            corp.equity_instrument_id = Some(inst_id);
            corp.founder_lockup_until_tick = tick.saturating_add(180);
            corp.founder_locked_shares = locked_shares;
            corp.total_shares_outstanding = corp
                .total_shares_outstanding
                .saturating_add(primary_shares_to_issue);
        }

        // Allocate newly issued primary shares into treasury first, then underwrite against CapitalistsExecutives
        // (up to 25% of Capitalists' available cash) and post any remainder on the new CLOB book.
        let mut underwritten_shares = 0u64;
        if let Some((_, _, cap_entity)) = self.all_cohort_entities() {
            let cap_avail = self.ledger.available_cash(cap_entity)?;
            let max_underwrite_budget = Money::from_cents(cap_avail.cents() / 4);
            let affordable = u64::try_from(
                max_underwrite_budget
                    .checked_div_euclid(offer_price.cents())?
                    .cents(),
            )
            .unwrap_or(0);
            underwritten_shares = primary_shares_to_issue.min(affordable);

            if underwritten_shares > 0 {
                let gross_proceeds = offer_price.checked_mul_u64(underwritten_shares)?;
                let (underwriting_fee, _) =
                    gross_proceeds.mul_div_rem_u64(300, BPS_DENOMINATOR)?;
                let net_proceeds = gross_proceeds.checked_sub(underwriting_fee)?;

                let cap_eq = self
                    .ledger
                    .entity_account_id(cap_entity, StandardAccountCode::CommonStock)?;
                let cap_cash = self
                    .ledger
                    .entity_account_id(cap_entity, StandardAccountCode::CashAndDeposits)?;
                let c_cash = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
                let c_eq = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::CommonStock)?;
                let c_tax = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::TaxAndFineExpense)?;
                let sov_cash = self.ledger.entity_account_id(
                    self.sovereign_entity_id,
                    StandardAccountCode::CashAndDeposits,
                )?;
                let sov_rev = self.ledger.entity_account_id(
                    self.sovereign_entity_id,
                    StandardAccountCode::TaxAndFeeRevenue,
                )?;

                let mut entry =
                    JournalEntry::new(tick, "IPO Primary Offering Underwriting Settlement")
                        .debit(cap_eq, gross_proceeds)
                        .credit(cap_cash, gross_proceeds)
                        .debit(c_cash, net_proceeds)
                        .credit(c_eq, gross_proceeds);
                if underwriting_fee.is_positive() {
                    entry = entry
                        .debit(c_tax, underwriting_fee)
                        .debit(sov_cash, underwriting_fee)
                        .credit(sov_rev, underwriting_fee);
                }
                self.ledger.post_entry(&entry)?;

                let cap_pos = self
                    .equity_positions
                    .entry((corp_id, cap_entity))
                    .or_default();
                cap_pos.owned_shares = cap_pos.owned_shares.saturating_add(underwritten_shares);
            }
        }

        let remainder = primary_shares_to_issue.saturating_sub(underwritten_shares);
        if remainder > 0 {
            let pos = self
                .equity_positions
                .entry((corp_id, entity_id))
                .or_default();
            pos.owned_shares = pos.owned_shares.saturating_add(remainder);
            self.submit_clob_limit_order(entity_id, inst_id, OrderSide::Ask, offer_price, remainder)?;
        }

        Ok(inst_id)
    }

    pub fn execute_borrow_shares_to_short(
        &mut self,
        borrower_entity_id: EntityId,
        corp_id: CorpId,
        shares: u64,
        sell_limit_price: Money,
    ) -> Result<ShortLoanId, CoreError> {
        if shares == 0 || !sell_limit_price.is_positive() {
            return Err(CoreError::ShortMarginViolation {
                reason: "short borrow shares and sell_limit_price must be strictly positive"
                    .to_owned(),
            });
        }
        let (corp_entity, eq_inst_id) = {
            let corp = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            let Some(eq_id) = corp.equity_instrument_id else {
                return Err(CoreError::ShortMarginViolation {
                    reason: "cannot short an unlisted private SME".to_owned(),
                });
            };
            (corp.entity_id, eq_id)
        };
        if borrower_entity_id == corp_entity {
            return Err(CoreError::ShortMarginViolation {
                reason: "issuer corporation cannot short its own shares".to_owned(),
            });
        }

        let Some((_, _, lender_entity_id)) = self.all_cohort_entities() else {
            return Err(CoreError::ShortMarginViolation {
                reason: "no institutional share lender cohort available".to_owned(),
            });
        };
        if borrower_entity_id == lender_entity_id {
            return Err(CoreError::ShortMarginViolation {
                reason: "lender cohort cannot borrow from itself".to_owned(),
            });
        }

        // Cancel resting orders for lender on this book so unreserved shares are maximized
        self.cancel_all_orders_for_entity_on_book(eq_inst_id, lender_entity_id)?;
        let lender_avail = self
            .equity_positions
            .get(&(corp_id, lender_entity_id))
            .map_or(0, |p| p.available_shares());
        if shares > lender_avail {
            return Err(CoreError::ShortMarginViolation {
                reason: format!(
                    "Short-Locate Vault insufficient shares: available {lender_avail}, requested {shares}"
                ),
            });
        }

        let ref_price = self
            .order_books
            .get(eq_inst_id)
            .map_or(sell_limit_price, |b| b.last_price.max(sell_limit_price));
        let notional = ref_price.checked_mul_u64(shares)?;
        let (initial_margin, _) = notional.mul_div_rem_u64(15_000, BPS_DENOMINATOR)?;

        // Lock 150% cash collateral on borrower
        self.ledger.reserve_cash(borrower_entity_id, initial_margin)?;

        // Physically transfer borrowed shares from lender -> borrower (`sum(owned_shares)` stays invariant!)
        if let Some(l_pos) = self.equity_positions.get_mut(&(corp_id, lender_entity_id)) {
            l_pos.owned_shares -= shares;
        }
        let b_pos = self
            .equity_positions
            .entry((corp_id, borrower_entity_id))
            .or_default();
        b_pos.owned_shares = b_pos.owned_shares.saturating_add(shares);

        let safe_sell_px = self
            .order_books
            .get(eq_inst_id)
            .map_or(sell_limit_price, |b| b.clamp_luld_price(sell_limit_price));
        self.submit_clob_limit_order(
            borrower_entity_id,
            eq_inst_id,
            OrderSide::Ask,
            safe_sell_px,
            shares,
        )?;

        let loan_id = ShortLoanId(self.next_short_loan_seq);
        self.next_short_loan_seq = self.next_short_loan_seq.saturating_add(1);
        self.short_positions.insert(
            loan_id,
            ShortPositionRecord {
                short_loan_id: loan_id,
                borrower_entity_id,
                lender_entity_id,
                corp_id,
                borrowed_shares: shares,
                collateral_reserved_cash: initial_margin,
                annual_borrow_fee_bps: 300,
                fee_reserve: RoundingReserve::new(),
            },
        );

        Ok(loan_id)
    }

    pub fn execute_cover_short_position(
        &mut self,
        short_loan_id: ShortLoanId,
        max_buy_price: Money,
    ) -> Result<(), CoreError> {
        let record = self
            .short_positions
            .get(&short_loan_id)
            .cloned()
            .ok_or(CoreError::UnknownShortLoan(short_loan_id))?;

        let eq_inst_id = self
            .corporations
            .get(record.corp_id)
            .and_then(|c| c.equity_instrument_id)
            .ok_or(CoreError::UnknownShortLoan(short_loan_id))?;

        // 1. Cancel any open orders by borrower on this equity book (releasing any unsold borrowed shares)
        self.cancel_all_orders_for_entity_on_book(eq_inst_id, record.borrower_entity_id)?;

        // 2. Release locked short margin cash so borrower can use it to buy back shares
        if record.collateral_reserved_cash.is_positive() {
            self.ledger
                .release_reserved_cash(record.borrower_entity_id, record.collateral_reserved_cash)?;
        }

        // 3. Check how many shares borrower currently holds vs needs to buy in
        let currently_held = self
            .equity_positions
            .get(&(record.corp_id, record.borrower_entity_id))
            .map_or(0, |p| p.available_shares());

        if currently_held < record.borrowed_shares {
            let deficit = record.borrowed_shares - currently_held;
            let clamp_buy = self
                .order_books
                .get(eq_inst_id)
                .map_or(max_buy_price, |b| b.clamp_luld_price(max_buy_price));
            let avail_cash = self.ledger.available_cash(record.borrower_entity_id)?;
            let affordable = u64::try_from(
                avail_cash
                    .checked_div_euclid(clamp_buy.cents().max(1))?
                    .cents(),
            )
            .unwrap_or(0);
            let buy_qty = deficit.min(affordable);
            if buy_qty > 0 {
                let (ord_id, _) = self.submit_clob_limit_order(
                    record.borrower_entity_id,
                    eq_inst_id,
                    OrderSide::Bid,
                    clamp_buy,
                    buy_qty,
                )?;
                let _ = self.cancel_clob_limit_order(eq_inst_id, ord_id);
            }
        }

        // 4. If still short (e.g., thin book or forced buy-in), settle remaining deficit directly against lender's shares at last_price
        let held_after_clob = self
            .equity_positions
            .get(&(record.corp_id, record.borrower_entity_id))
            .map_or(0, |p| p.available_shares());
        let returnable = held_after_clob.min(record.borrowed_shares);

        if returnable > 0 {
            if let Some(b_pos) = self
                .equity_positions
                .get_mut(&(record.corp_id, record.borrower_entity_id))
            {
                b_pos.owned_shares -= returnable;
            }
            let l_pos = self
                .equity_positions
                .entry((record.corp_id, record.lender_entity_id))
                .or_default();
            l_pos.owned_shares = l_pos.owned_shares.saturating_add(returnable);
        }

        let remaining_borrowed = record.borrowed_shares.saturating_sub(returnable);
        if remaining_borrowed == 0 {
            self.short_positions.remove(&short_loan_id);
        } else {
            // Re-lock whatever cash remains up to 150% of remaining liability
            let mark_px = self
                .order_books
                .get(eq_inst_id)
                .map_or(Money::from_dollars(25), |b| b.last_price);
            let rem_notional = mark_px.checked_mul_u64(remaining_borrowed)?;
            let (desired_margin, _) = rem_notional.mul_div_rem_u64(15_000, BPS_DENOMINATOR)?;
            let avail_left = self.ledger.available_cash(record.borrower_entity_id)?;
            let new_collat = desired_margin.min(avail_left);
            if new_collat.is_positive() {
                self.ledger
                    .reserve_cash(record.borrower_entity_id, new_collat)?;
            }
            if let Some(s) = self.short_positions.get_mut(&short_loan_id) {
                s.borrowed_shares = remaining_borrowed;
                s.collateral_reserved_cash = new_collat;
            }
        }

        Ok(())
    }

    fn accrue_daily_short_borrow_fees(&mut self, tick: u64) -> Result<(), CoreError> {
        let loan_ids: Vec<ShortLoanId> = self.short_positions.keys().copied().collect();
        for loan_id in loan_ids {
            let Some((corp_id, borrower_id, lender_id, shares, fee_bps, collat)) =
                self.short_positions.get(&loan_id).map(|s| {
                    (
                        s.corp_id,
                        s.borrower_entity_id,
                        s.lender_entity_id,
                        s.borrowed_shares,
                        s.annual_borrow_fee_bps,
                        s.collateral_reserved_cash,
                    )
                })
            else {
                continue;
            };
            let mark_px = self
                .corporations
                .get(corp_id)
                .and_then(|c| c.equity_instrument_id)
                .and_then(|id| self.order_books.get(id))
                .map_or(Money::from_dollars(25), |b| b.last_price);
            let notional = mark_px.checked_mul_u64(shares)?;
            let fee_due = match self.short_positions.get_mut(&loan_id) {
                Some(s) => s.fee_reserve.accrue_daily_360_bps(notional, fee_bps)?,
                None => Money::ZERO,
            };
            if !fee_due.is_positive() {
                continue;
            }

            let avail = self.ledger.available_cash(borrower_id)?;
            let pay = if avail >= fee_due {
                fee_due
            } else {
                let short = fee_due.checked_sub(avail)?;
                let from_collat = short.min(collat);
                if from_collat.is_positive() {
                    self.ledger.release_reserved_cash(borrower_id, from_collat)?;
                    if let Some(s) = self.short_positions.get_mut(&loan_id) {
                        s.collateral_reserved_cash =
                            s.collateral_reserved_cash.checked_sub(from_collat)?;
                    }
                }
                avail.checked_add(from_collat)?
            };

            if pay.is_positive() {
                let b_int = self
                    .ledger
                    .entity_account_id(borrower_id, StandardAccountCode::InterestExpense)?;
                let b_cash = self
                    .ledger
                    .entity_account_id(borrower_id, StandardAccountCode::CashAndDeposits)?;
                let l_cash = self
                    .ledger
                    .entity_account_id(lender_id, StandardAccountCode::CashAndDeposits)?;
                let l_int = self
                    .ledger
                    .entity_account_id(lender_id, StandardAccountCode::InterestIncome)?;
                let entry = JournalEntry::new(tick, "Daily Short-Locate Share Borrow Fee")
                    .debit(b_int, pay)
                    .credit(b_cash, pay)
                    .debit(l_cash, pay)
                    .credit(l_int, pay);
                self.ledger.post_entry(&entry)?;
            }
        }
        Ok(())
    }

    fn execute_crime_operation(
        &mut self,
        actor_corp_id: CorpId,
        target_corp_id: CorpId,
        kind: CrimeOperationKind,
        seq_no: u32,
    ) -> Result<(), CoreError> {
        if actor_corp_id == target_corp_id {
            return Err(CoreError::InvalidOrder {
                reason: "corporation cannot execute crime operation against itself".to_owned(),
            });
        }
        let tick = self.current_tick.0;
        let actor_entity = self
            .corporations
            .get(actor_corp_id)
            .ok_or(CoreError::UnknownCorporation(actor_corp_id))?
            .entity_id;
        let (target_entity, target_cloud, sec_shield, log_shield) = {
            let target = self
                .corporations
                .get(target_corp_id)
                .ok_or(CoreError::UnknownCorporation(target_corp_id))?;
            (
                target.entity_id,
                target.cloud_tier,
                target.security_shield_until_tick > tick,
                target.logistics_shield_until_tick > tick,
            )
        };

        let mut rng = self.seeded_command_rng(seq_no);
        let roll_bps: u32 = rng.gen_range(0..10_000);

        match kind {
            CrimeOperationKind::HijackCargo { contract_id } => {
                // Logistics shield reduces success probability from 75% to 15%
                let threshold = if log_shield { 1_500 } else { 7_500 };
                if roll_bps >= threshold {
                    return Ok(());
                }
                let Some(contract) = self
                    .b2b_contracts
                    .iter()
                    .find(|c| c.contract_id == contract_id)
                    .cloned()
                else {
                    return Ok(());
                };
                let supplier_id = contract.supplier_corp_id;
                let sku_id = contract.sku_id;
                let inv_code = self
                    .skus
                    .get(sku_id)
                    .ok_or(CoreError::UnknownSku(sku_id))?
                    .ledger_inventory_account;
                let sup_entity = self
                    .corporations
                    .get(supplier_id)
                    .ok_or(CoreError::UnknownCorporation(supplier_id))?
                    .entity_id;
                let avail_units = self
                    .corporations
                    .get(supplier_id)
                    .map_or(0, |c| c.inventory_lot(sku_id).available_qty());
                let stolen_qty = ((contract.daily_qty.saturating_mul(30)) / 100)
                    .max(1)
                    .min(avail_units);
                if stolen_qty == 0 {
                    return Ok(());
                }

                let stolen_book_value = self
                    .corporations
                    .get_mut(supplier_id)
                    .ok_or(CoreError::UnknownCorporation(supplier_id))?
                    .inventory_lot_mut(sku_id)
                    .draw_unreserved_units(stolen_qty)?;

                self.corporations
                    .get_mut(actor_corp_id)
                    .ok_or(CoreError::UnknownCorporation(actor_corp_id))?
                    .inventory_lot_mut(sku_id)
                    .record_addition(stolen_qty, stolen_book_value)?;

                if stolen_book_value.is_positive() {
                    let s_cogs = self
                        .ledger
                        .entity_account_id(sup_entity, StandardAccountCode::CostOfGoodsSold)?;
                    let s_inv = self.ledger.entity_account_id(sup_entity, inv_code)?;
                    let a_inv = self.ledger.entity_account_id(actor_entity, inv_code)?;
                    let a_rev = self
                        .ledger
                        .entity_account_id(actor_entity, StandardAccountCode::SalesRevenue)?;
                    let entry = JournalEntry::new(tick, "B2B Cargo Hijack WAC Seizure")
                        .debit(s_cogs, stolen_book_value)
                        .credit(s_inv, stolen_book_value)
                        .debit(a_inv, stolen_book_value)
                        .credit(a_rev, stolen_book_value);
                    self.ledger.post_entry(&entry)?;
                }
            }
            CrimeOperationKind::ExtortRetailer { demand_amount } => {
                if !demand_amount.is_positive() {
                    return Ok(());
                }
                // PrivateSecurity shield reduces extortion success from 80% to 10%
                let threshold = if sec_shield { 1_000 } else { 8_000 };
                if roll_bps >= threshold {
                    return Ok(());
                }
                let target_avail = self.ledger.available_cash(target_entity)?;
                let seized = demand_amount.min(Money::from_cents(target_avail.cents() / 2));
                if seized.is_positive() {
                    let t_exp = self.ledger.entity_account_id(
                        target_entity,
                        StandardAccountCode::SecurityAndFreightExpense,
                    )?;
                    let t_cash = self
                        .ledger
                        .entity_account_id(target_entity, StandardAccountCode::CashAndDeposits)?;
                    let a_illicit = self
                        .ledger
                        .entity_account_id(actor_entity, StandardAccountCode::IllicitCash)?;
                    let a_rev = self
                        .ledger
                        .entity_account_id(actor_entity, StandardAccountCode::SalesRevenue)?;
                    let entry = JournalEntry::new(tick, "Syndicate Protection Extortion Transfer")
                        .debit(t_exp, seized)
                        .credit(t_cash, seized)
                        .debit(a_illicit, seized)
                        .credit(a_rev, seized);
                    self.ledger.post_entry(&entry)?;
                }
            }
            CrimeOperationKind::CyberRansomware { ransom_demand } => {
                // Tier 3 Clef-Firewall blocks 90% of ransomware attacks
                let threshold = if target_cloud == CloudTier::Tier3ClefFirewall {
                    1_000
                } else {
                    7_500
                };
                if roll_bps >= threshold {
                    return Ok(());
                }
                let target_avail = self.ledger.available_cash(target_entity)?;
                let pay = ransom_demand.min(Money::from_cents(target_avail.cents() / 4));
                if pay.is_positive() {
                    let t_exp = self.ledger.entity_account_id(
                        target_entity,
                        StandardAccountCode::SecurityAndFreightExpense,
                    )?;
                    let t_cash = self
                        .ledger
                        .entity_account_id(target_entity, StandardAccountCode::CashAndDeposits)?;
                    let a_illicit = self
                        .ledger
                        .entity_account_id(actor_entity, StandardAccountCode::IllicitCash)?;
                    let a_rev = self
                        .ledger
                        .entity_account_id(actor_entity, StandardAccountCode::SalesRevenue)?;
                    let entry = JournalEntry::new(tick, "Cyber Ransomware Settlement")
                        .debit(t_exp, pay)
                        .credit(t_cash, pay)
                        .debit(a_illicit, pay)
                        .credit(a_rev, pay);
                    self.ledger.post_entry(&entry)?;
                }
                if let Some(target) = self.corporations.get_mut(target_corp_id) {
                    target.ransomware_degraded_until_tick = tick.saturating_add(15);
                }
            }
            CrimeOperationKind::CorporateWiretap { duration_ticks } => {
                let threshold = if target_cloud == CloudTier::Tier3ClefFirewall {
                    2_000
                } else {
                    8_500
                };
                if roll_bps < threshold {
                    self.active_wiretaps.insert(
                        (actor_corp_id, target_corp_id),
                        tick.saturating_add(duration_ticks.max(30)),
                    );
                }
            }
        }
        Ok(())
    }

    fn execute_launder_illicit_cash(
        &mut self,
        corp_id: CorpId,
        amount: Money,
        seq_no: u32,
    ) -> Result<(), CoreError> {
        if !amount.is_positive() {
            return Ok(());
        }
        let tick = self.current_tick.0;
        let (entity_id, prev_laundered, genuine_b2c, is_retailer) = {
            let corp = self
                .corporations
                .get(corp_id)
                .ok_or(CoreError::UnknownCorporation(corp_id))?;
            (
                corp.entity_id,
                corp.monthly_laundered_cash,
                corp.monthly_genuine_b2c_revenue,
                matches!(corp.role, CorporationRole::Retailer { .. }),
            )
        };

        let illicit_bal = self.ledger.illicit_cash_balance(entity_id)?;
        let launder_amt = amount.min(illicit_bal);
        if !launder_amt.is_positive() {
            return Ok(());
        }

        // 1. Standard 500 bps (5%) placement tax to SovereignTreasury
        let (placement_tax, _) = launder_amt.mul_div_rem_u64(500, BPS_DENOMINATOR)?;
        let clean_credited = launder_amt.checked_sub(placement_tax)?;

        let c_cash = self
            .ledger
            .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
        let c_illicit = self
            .ledger
            .entity_account_id(entity_id, StandardAccountCode::IllicitCash)?;
        let c_tax_exp = self
            .ledger
            .entity_account_id(entity_id, StandardAccountCode::TaxAndFineExpense)?;
        let sov_cash = self.ledger.entity_account_id(
            self.sovereign_entity_id,
            StandardAccountCode::CashAndDeposits,
        )?;
        let sov_rev = self.ledger.entity_account_id(
            self.sovereign_entity_id,
            StandardAccountCode::TaxAndFeeRevenue,
        )?;

        let mut entry = JournalEntry::new(tick, "Retail Cash Placement Laundering")
            .debit(c_cash, clean_credited)
            .credit(c_illicit, launder_amt);
        if placement_tax.is_positive() {
            entry = entry
                .debit(c_tax_exp, placement_tax)
                .debit(sov_cash, placement_tax)
                .credit(sov_rev, placement_tax);
        }
        self.ledger.post_entry(&entry)?;

        let new_laundered = prev_laundered.checked_add(launder_amt)?;
        if let Some(corp) = self.corporations.get_mut(corp_id) {
            corp.monthly_laundered_cash = new_laundered;
        }

        // 2. Automated Statistical Forensic Audit & Guard #4 (Non-Negative Balance Protection on 150% Fine)
        // Retailers have a $5,000 baseline + 35% of genuine B2C revenue safe harbor; non-retailers have $1,000 + 15%
        let (safe_ratio_bps, baseline) = if is_retailer {
            (3_500u64, Money::from_dollars(5_000))
        } else {
            (1_500u64, Money::from_dollars(1_000))
        };
        let (safe_from_rev, _) = genuine_b2c.mul_div_rem_u64(safe_ratio_bps, BPS_DENOMINATOR)?;
        let safe_ceiling = safe_from_rev.checked_add(baseline)?;

        if new_laundered > safe_ceiling {
            let excess = new_laundered.checked_sub(safe_ceiling)?;
            let mut rng = self.seeded_command_rng(seq_no);
            let roll: u32 = rng.gen_range(0..10_000);
            // If excess > safe_ceiling, 100% deterministic audit trigger; otherwise 75% trigger
            if excess >= safe_ceiling || roll < 7_500 {
                let (total_fine, _) = launder_amt.mul_div_rem_u64(15_000, BPS_DENOMINATOR)?;
                let rem_illicit = self.ledger.illicit_cash_balance(entity_id)?;
                let seize_illicit = total_fine.min(rem_illicit);
                let rem_after_illicit = total_fine.checked_sub(seize_illicit)?;

                let rem_clean = self.ledger.available_cash(entity_id)?;
                let seize_clean = rem_after_illicit.min(rem_clean);
                let unpaid_lien = rem_after_illicit.checked_sub(seize_clean)?;
                let total_cash_seized = seize_illicit.checked_add(seize_clean)?;

                let mut fine_entry =
                    JournalEntry::new(tick, "Forensic Anti-Money-Laundering 150% Penalty")
                        .debit(c_tax_exp, total_fine)
                        .credit(sov_rev, total_fine);

                if seize_illicit.is_positive() {
                    fine_entry = fine_entry.credit(c_illicit, seize_illicit);
                }
                if seize_clean.is_positive() {
                    fine_entry = fine_entry.credit(c_cash, seize_clean);
                }
                if total_cash_seized.is_positive() {
                    fine_entry = fine_entry.debit(sov_cash, total_cash_seized);
                }
                if unpaid_lien.is_positive() {
                    let c_tax_pay = self
                        .ledger
                        .entity_account_id(entity_id, StandardAccountCode::TaxesPayable)?;
                    let sov_ar = self.ledger.entity_account_id(
                        self.sovereign_entity_id,
                        StandardAccountCode::AccountsReceivable,
                    )?;
                    fine_entry = fine_entry
                        .credit(c_tax_pay, unpaid_lien)
                        .debit(sov_ar, unpaid_lien);
                    let lien_entry = self.tax_liens.entry(entity_id).or_insert(Money::ZERO);
                    *lien_entry = lien_entry.checked_add(unpaid_lien)?;
                }

                self.ledger.post_entry(&fine_entry)?;
            }
        }

        Ok(())
    }

    fn garnish_tax_lien_if_any(
        &mut self,
        tick: u64,
        entity_id: EntityId,
        survival_floor: Money,
    ) -> Result<(), CoreError> {
        let Some(&lien_due) = self.tax_liens.get(&entity_id) else {
            return Ok(());
        };
        if !lien_due.is_positive() {
            self.tax_liens.remove(&entity_id);
            return Ok(());
        }
        let avail = self.ledger.available_cash(entity_id)?;
        if avail <= survival_floor {
            return Ok(());
        }
        let pay = lien_due.min(avail.checked_sub(survival_floor)?);
        if !pay.is_positive() {
            return Ok(());
        }

        let c_tax_pay = self
            .ledger
            .entity_account_id(entity_id, StandardAccountCode::TaxesPayable)?;
        let c_cash = self
            .ledger
            .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
        let sov_cash = self.ledger.entity_account_id(
            self.sovereign_entity_id,
            StandardAccountCode::CashAndDeposits,
        )?;
        let sov_ar = self.ledger.entity_account_id(
            self.sovereign_entity_id,
            StandardAccountCode::AccountsReceivable,
        )?;

        let entry = JournalEntry::new(tick, "Senior Forensic Tax Lien Garnishment")
            .debit(c_tax_pay, pay)
            .credit(c_cash, pay)
            .debit(sov_cash, pay)
            .credit(sov_ar, pay);
        self.ledger.post_entry(&entry)?;

        let remaining = lien_due.checked_sub(pay)?;
        if remaining.is_positive() {
            self.tax_liens.insert(entity_id, remaining);
        } else {
            self.tax_liens.remove(&entity_id);
        }
        Ok(())
    }

    fn bill_monthly_sovereign_cloud(&mut self, tick: u64) -> Result<(), CoreError> {
        let corp_ids: Vec<CorpId> = self.corporations.keys().collect();
        for corp_id in corp_ids {
            let Some((entity_id, desired_tier)) = self
                .corporations
                .get(corp_id)
                .map(|c| (c.entity_id, c.subscribed_cloud_tier))
            else {
                continue;
            };
            if desired_tier == CloudTier::Offline {
                if let Some(c) = self.corporations.get_mut(corp_id) {
                    c.cloud_tier = CloudTier::Offline;
                }
                continue;
            }

            let sc_cost = desired_tier.monthly_service_credits();
            let avail_sc = self
                .service_credit_positions
                .get(&entity_id)
                .map_or(0, |p| p.available_credits());

            if sc_cost > 0 && avail_sc >= sc_cost {
                // Burn ServiceCredits directly (zero M1 impact)
                if let Some(pos) = self.service_credit_positions.get_mut(&entity_id) {
                    pos.owned_credits -= sc_cost;
                }
                self.total_service_credits_supply =
                    self.total_service_credits_supply.saturating_sub(sc_cost);
                if let Some(c) = self.corporations.get_mut(corp_id) {
                    c.cloud_tier = desired_tier;
                }
                continue;
            }

            // Fallback: Pay monthly VC fee to SovereignTreasury (respecting 5-day survival floor)
            let vc_fee = desired_tier.monthly_vc_cost();
            let survival_floor = self.corporations.get(corp_id).map_or(Money::ZERO, |c| {
                c.daily_wage_bill
                    .checked_add(c.daily_manager_bill)
                    .unwrap_or(Money::ZERO)
                    .checked_mul_u64(5)
                    .unwrap_or(Money::ZERO)
            });
            let avail_cash = self.ledger.available_cash(entity_id)?;
            if avail_cash >= vc_fee.checked_add(survival_floor)? {
                let c_cloud = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::CloudServiceExpense)?;
                let c_cash = self
                    .ledger
                    .entity_account_id(entity_id, StandardAccountCode::CashAndDeposits)?;
                let sov_cash = self.ledger.entity_account_id(
                    self.sovereign_entity_id,
                    StandardAccountCode::CashAndDeposits,
                )?;
                let sov_rev = self.ledger.entity_account_id(
                    self.sovereign_entity_id,
                    StandardAccountCode::TaxAndFeeRevenue,
                )?;

                let entry = JournalEntry::new(tick, "Monthly SovereignCloud Enterprise Subscription")
                    .debit(c_cloud, vc_fee)
                    .credit(c_cash, vc_fee)
                    .debit(sov_cash, vc_fee)
                    .credit(sov_rev, vc_fee);
                self.ledger.post_entry(&entry)?;
                if let Some(c) = self.corporations.get_mut(corp_id) {
                    c.cloud_tier = desired_tier;
                }
            } else if let Some(c) = self.corporations.get_mut(corp_id) {
                // Graceful downgrade when unable to pay
                c.cloud_tier = CloudTier::Offline;
            }
        }
        Ok(())
    }

    /// Guard #1: Month-End Sovereign Fiscal Recycling (`80%` of `SovereignTreasury` cash above
    /// `sovereign_dr_reserve_buffer` is recycled back into civil-service payroll and B2B procurement).
    fn recycle_monthly_sovereign_surplus(&mut self, tick: u64) -> Result<(), CoreError> {
        let sov_id = self.sovereign_entity_id;
        let avail_cash = self.ledger.available_cash(sov_id)?;
        if avail_cash <= self.sovereign_dr_reserve_buffer {
            return Ok(());
        }
        let surplus = avail_cash.checked_sub(self.sovereign_dr_reserve_buffer)?;
        let (recycle_pool, _) = surplus.mul_div_rem_u64(8_000, BPS_DENOMINATOR)?;
        if !recycle_pool.is_positive() {
            return Ok(());
        }

        let Some((wage_entity, mgr_entity, cap_entity)) = self.all_cohort_entities() else {
            return Ok(());
        };

        let (wage_grant, _) = recycle_pool.mul_div_rem_u64(5_000, BPS_DENOMINATOR)?;
        let (mgr_grant, _) = recycle_pool.mul_div_rem_u64(3_000, BPS_DENOMINATOR)?;
        let cap_grant = recycle_pool
            .checked_sub(wage_grant)?
            .checked_sub(mgr_grant)?;

        let sov_wage = self
            .ledger
            .entity_account_id(sov_id, StandardAccountCode::WageExpense)?;
        let sov_cash = self
            .ledger
            .entity_account_id(sov_id, StandardAccountCode::CashAndDeposits)?;
        let w_cash = self
            .ledger
            .entity_account_id(wage_entity, StandardAccountCode::CashAndDeposits)?;
        let w_rev = self
            .ledger
            .entity_account_id(wage_entity, StandardAccountCode::SalesRevenue)?;
        let m_cash = self
            .ledger
            .entity_account_id(mgr_entity, StandardAccountCode::CashAndDeposits)?;
        let m_rev = self
            .ledger
            .entity_account_id(mgr_entity, StandardAccountCode::SalesRevenue)?;
        let c_cash = self
            .ledger
            .entity_account_id(cap_entity, StandardAccountCode::CashAndDeposits)?;
        let c_rev = self
            .ledger
            .entity_account_id(cap_entity, StandardAccountCode::SalesRevenue)?;

        let mut entry = JournalEntry::new(tick, "Monthly Sovereign Fiscal Recycling Sweep")
            .debit(sov_wage, recycle_pool)
            .credit(sov_cash, recycle_pool);
        if wage_grant.is_positive() {
            entry = entry.debit(w_cash, wage_grant).credit(w_rev, wage_grant);
        }
        if mgr_grant.is_positive() {
            entry = entry.debit(m_cash, mgr_grant).credit(m_rev, mgr_grant);
        }
        if cap_grant.is_positive() {
            entry = entry.debit(c_cash, cap_grant).credit(c_rev, cap_grant);
        }

        self.ledger.post_entry(&entry)?;
        Ok(())
    }

    fn distribute_quarterly_depositary_dividends(&mut self, tick: u64) -> Result<(), CoreError> {
        let sov_id = self.sovereign_entity_id;
        let dr_books: Vec<(InstrumentId, u32, Money)> = self
            .order_books
            .iter()
            .filter_map(|(id, b)| match b.kind {
                InstrumentKind::DepositaryReceipt {
                    quarterly_dividend_bps,
                    ..
                } if quarterly_dividend_bps > 0 => {
                    Some((id, quarterly_dividend_bps, b.last_price))
                }
                _ => None,
            })
            .collect();

        for (inst_id, div_bps, last_px) in dr_books {
            let holders: Vec<(EntityId, u64)> = self
                .depositary_positions
                .iter()
                .filter_map(|(&(iid, holder_id), pos)| {
                    if iid == inst_id && holder_id != sov_id && pos.owned_shares > 0 {
                        Some((holder_id, pos.owned_shares))
                    } else {
                        None
                    }
                })
                .collect();
            let total_held: u64 = holders.iter().map(|(_, q)| *q).sum();
            if total_held == 0 {
                continue;
            }
            let total_notional = last_px.checked_mul_u64(total_held)?;
            let (total_div, _) =
                total_notional.mul_div_rem_u64(u64::from(div_bps), BPS_DENOMINATOR)?;
            let sov_avail = self.ledger.available_cash(sov_id)?;
            let distributable = total_div.min(Money::from_cents(sov_avail.cents() / 4));
            if !distributable.is_positive() {
                continue;
            }

            let allocations = allocate_hamilton(distributable, &holders)?;
            let sov_re = self
                .ledger
                .entity_account_id(sov_id, StandardAccountCode::RetainedEarnings)?;
            let sov_cash = self
                .ledger
                .entity_account_id(sov_id, StandardAccountCode::CashAndDeposits)?;
            let mut entry =
                JournalEntry::new(tick, "Quarterly Global Depositary Synthetic Dividend")
                    .debit(sov_re, distributable)
                    .credit(sov_cash, distributable);
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
        }
        Ok(())
    }

    pub fn execute_send_wire_message(
        &mut self,
        sender_corp_id: CorpId,
        recipient_corp_id: CorpId,
        subject: String,
        body: String,
        attachment: Option<WireAttachment>,
    ) -> Result<WireMessageId, CoreError> {
        if !self.corporations.contains_key(sender_corp_id) {
            return Err(CoreError::UnknownCorporation(sender_corp_id));
        }
        if !self.corporations.contains_key(recipient_corp_id) {
            return Err(CoreError::UnknownCorporation(recipient_corp_id));
        }
        let message_id = WireMessageId(self.next_wire_seq);
        self.next_wire_seq = self.next_wire_seq.saturating_add(1);
        let status = if attachment.is_some() {
            WireAttachmentStatus::Pending
        } else {
            WireAttachmentStatus::NoAttachment
        };
        self.wire_messages.push(WireMessage {
            message_id,
            tick: self.current_tick.0,
            sender_corp_id,
            recipient_corp_id,
            subject,
            body,
            attachment,
            status,
        });
        self.prune_resolved_wire_messages();
        Ok(message_id)
    }

    pub fn execute_respond_wire_attachment(
        &mut self,
        responder_corp_id: CorpId,
        message_id: WireMessageId,
        accept: bool,
    ) -> Result<(), CoreError> {
        let msg_idx = self
            .wire_messages
            .iter()
            .position(|m| m.message_id == message_id)
            .ok_or(CoreError::UnknownWireMessage(message_id))?;

        let msg = self
            .wire_messages
            .get(msg_idx)
            .cloned()
            .ok_or(CoreError::UnknownWireMessage(message_id))?;
        if msg.recipient_corp_id != responder_corp_id {
            return Err(CoreError::InvalidOrder {
                reason: "only the recipient corporation can respond to a wire attachment"
                    .to_owned(),
            });
        }
        if msg.status != WireAttachmentStatus::Pending {
            return Ok(());
        }

        if !accept {
            if let Some(m) = self.wire_messages.get_mut(msg_idx) {
                m.status = WireAttachmentStatus::Rejected;
            }
            self.prune_resolved_wire_messages();
            return Ok(());
        }

        let tick = self.current_tick.0;
        let sender_entity = self
            .corporations
            .get(msg.sender_corp_id)
            .ok_or(CoreError::UnknownCorporation(msg.sender_corp_id))?
            .entity_id;
        let responder_entity = self
            .corporations
            .get(responder_corp_id)
            .ok_or(CoreError::UnknownCorporation(responder_corp_id))?
            .entity_id;

        if let Some(attachment) = msg.attachment {
            match attachment {
                WireAttachment::B2BContractOffer {
                    sku_id,
                    daily_qty,
                    unit_price,
                    duration_days,
                } => {
                    let contract_id = ContractId(self.next_contract_seq);
                    self.next_contract_seq = self.next_contract_seq.saturating_add(1);
                    self.b2b_contracts.push(B2BSupplyContract {
                        contract_id,
                        supplier_corp_id: msg.sender_corp_id,
                        buyer_corp_id: responder_corp_id,
                        sku_id,
                        daily_qty,
                        unit_price,
                        remaining_days: duration_days,
                    });
                }
                WireAttachment::OtcBlockTradeOffer {
                    instrument_id,
                    qty,
                    unit_price,
                } => {
                    // Atomic OTC Delivery-versus-Payment (sender sells `qty` of `instrument_id` to responder at `unit_price`)
                    let kind = self
                        .order_books
                        .get(instrument_id)
                        .ok_or(CoreError::UnknownInstrument(instrument_id))?
                        .kind;
                    let notional = unit_price.checked_mul_u64(qty)?;
                    self.ledger.reserve_cash(responder_entity, notional)?;
                    match kind {
                        InstrumentKind::Equity { corp_id } => {
                            let s_pos = self
                                .equity_positions
                                .entry((corp_id, sender_entity))
                                .or_default();
                            if s_pos.available_shares() < qty {
                                self.ledger.release_reserved_cash(responder_entity, notional)?;
                                return Err(CoreError::InsufficientShares {
                                    corp_id,
                                    available: s_pos.available_shares(),
                                    requested: qty,
                                });
                            }
                            s_pos.reserved_shares = s_pos.reserved_shares.saturating_add(qty);
                        }
                        InstrumentKind::DepositaryReceipt { .. } => {
                            let s_pos = self
                                .depositary_positions
                                .entry((instrument_id, sender_entity))
                                .or_default();
                            if s_pos.available_shares() < qty {
                                self.ledger.release_reserved_cash(responder_entity, notional)?;
                                return Err(CoreError::InvalidOrder {
                                    reason: "insufficient DR units for OTC block trade".to_owned(),
                                });
                            }
                            s_pos.reserved_shares = s_pos.reserved_shares.saturating_add(qty);
                        }
                        InstrumentKind::ServiceCredit => {
                            let s_pos = self
                                .service_credit_positions
                                .entry(sender_entity)
                                .or_default();
                            if s_pos.available_credits() < qty {
                                self.ledger.release_reserved_cash(responder_entity, notional)?;
                                return Err(CoreError::InvalidOrder {
                                    reason: "insufficient ServiceCredits for OTC block trade"
                                        .to_owned(),
                                });
                            }
                            s_pos.reserved_credits = s_pos.reserved_credits.saturating_add(qty);
                        }
                        InstrumentKind::Commodity { sku_id } => {
                            let s_corp = self
                                .corporations
                                .get_mut(msg.sender_corp_id)
                                .ok_or(CoreError::UnknownCorporation(msg.sender_corp_id))?;
                            if let Err(e) = s_corp.inventory_lot_mut(sku_id).reserve_units(qty) {
                                self.ledger.release_reserved_cash(responder_entity, notional)?;
                                return Err(CoreError::Ledger(e));
                            }
                        }
                    }

                    let otc_fill = TradeFill {
                        tick,
                        instrument_id,
                        buyer_entity_id: responder_entity,
                        seller_entity_id: sender_entity,
                        bid_order_id: OrderId(0),
                        ask_order_id: OrderId(0),
                        execution_price: unit_price,
                        buyer_limit_price: unit_price,
                        qty,
                    };
                    self.settle_clob_fill(kind, &otc_fill)?;
                }
                WireAttachment::SyndicateExtortionDemand {
                    demand_amount,
                    immunity_days,
                } => {
                    let avail = self.ledger.available_cash(responder_entity)?;
                    let pay = demand_amount.min(avail);
                    if pay.is_positive() {
                        let r_exp = self.ledger.entity_account_id(
                            responder_entity,
                            StandardAccountCode::SecurityAndFreightExpense,
                        )?;
                        let r_cash = self.ledger.entity_account_id(
                            responder_entity,
                            StandardAccountCode::CashAndDeposits,
                        )?;
                        let s_illicit = self
                            .ledger
                            .entity_account_id(sender_entity, StandardAccountCode::IllicitCash)?;
                        let s_rev = self
                            .ledger
                            .entity_account_id(sender_entity, StandardAccountCode::SalesRevenue)?;
                        let entry = JournalEntry::new(tick, "Wire Protection Tribute Settlement")
                            .debit(r_exp, pay)
                            .credit(r_cash, pay)
                            .debit(s_illicit, pay)
                            .credit(s_rev, pay);
                        self.ledger.post_entry(&entry)?;
                        if let Some(resp) = self.corporations.get_mut(responder_corp_id) {
                            resp.security_shield_until_tick =
                                tick.saturating_add(u64::from(immunity_days));
                        }
                    }
                }
                WireAttachment::ServiceRetainerOffer {
                    is_security,
                    daily_fee,
                    duration_days,
                } => {
                    let total_fee = daily_fee.checked_mul_u64(u64::from(duration_days))?;
                    let avail = self.ledger.available_cash(responder_entity)?;
                    if avail >= total_fee && total_fee.is_positive() {
                        let r_exp = self.ledger.entity_account_id(
                            responder_entity,
                            StandardAccountCode::SecurityAndFreightExpense,
                        )?;
                        let r_cash = self.ledger.entity_account_id(
                            responder_entity,
                            StandardAccountCode::CashAndDeposits,
                        )?;
                        let s_cash = self.ledger.entity_account_id(
                            sender_entity,
                            StandardAccountCode::CashAndDeposits,
                        )?;
                        let s_rev = self
                            .ledger
                            .entity_account_id(sender_entity, StandardAccountCode::SalesRevenue)?;
                        let entry = JournalEntry::new(tick, "B2B Security/Logistics Retainer")
                            .debit(r_exp, total_fee)
                            .credit(r_cash, total_fee)
                            .debit(s_cash, total_fee)
                            .credit(s_rev, total_fee);
                        self.ledger.post_entry(&entry)?;
                        if let Some(resp) = self.corporations.get_mut(responder_corp_id) {
                            let until = tick.saturating_add(u64::from(duration_days));
                            if is_security {
                                resp.security_shield_until_tick = until;
                            } else {
                                resp.logistics_shield_until_tick = until;
                            }
                        }
                    }
                }
            }
        }

        if let Some(m) = self.wire_messages.get_mut(msg_idx) {
            m.status = WireAttachmentStatus::Accepted;
        }
        self.prune_resolved_wire_messages();
        Ok(())
    }

    /// Guard #6: Bounds resolved (`NoAttachment`/`Accepted`/`Rejected`/`Expired`) wire messages
    /// per corporation to `MAX_RESOLVED_WIRE_MESSAGES_PER_CORP` so `postcard` + `blake3` stays fast.
    fn prune_resolved_wire_messages(&mut self) {
        let mut counts_by_corp: BTreeMap<CorpId, usize> = BTreeMap::new();
        let mut keep_flags = vec![true; self.wire_messages.len()];

        for (idx, msg) in self.wire_messages.iter().enumerate().rev() {
            if msg.status == WireAttachmentStatus::Pending {
                continue;
            }
            let sender_count = counts_by_corp.get(&msg.sender_corp_id).copied().unwrap_or(0);
            let recip_count = counts_by_corp
                .get(&msg.recipient_corp_id)
                .copied()
                .unwrap_or(0);
            if sender_count >= MAX_RESOLVED_WIRE_MESSAGES_PER_CORP
                && recip_count >= MAX_RESOLVED_WIRE_MESSAGES_PER_CORP
            {
                if let Some(flag) = keep_flags.get_mut(idx) {
                    *flag = false;
                }
            } else {
                counts_by_corp.insert(msg.sender_corp_id, sender_count.saturating_add(1));
                counts_by_corp.insert(msg.recipient_corp_id, recip_count.saturating_add(1));
            }
        }

        let mut iter = keep_flags.into_iter();
        self.wire_messages.retain(|_| iter.next().unwrap_or(true));
    }

    fn wage_and_manager_entities(&self) -> Option<(EntityId, EntityId)> {
        let (w, m, _) = self.all_cohort_entities()?;
        Some((w, m))
    }

    fn all_cohort_entities(&self) -> Option<(EntityId, EntityId, EntityId)> {
        let mut wage = None;
        let mut mgr = None;
        let mut cap = None;
        for (_, c) in &self.cohorts {
            match c.kind {
                HouseholdCohortKind::WageEarners => wage = Some(c.entity_id),
                HouseholdCohortKind::ManagersSkilled => mgr = Some(c.entity_id),
                HouseholdCohortKind::CapitalistsExecutives => cap = Some(c.entity_id),
            }
        }
        Some((wage?, mgr?, cap?))
    }

    /// Audits:
    /// 1. `self.ledger.audit_trial_balance()` (global double-entry conservation + endogenous M1 conservation).
    /// 2. Every corporation's `WacInventoryLot` book values summed by account code match the corporation's
    ///    ledger `InventoryRaw` / `InventoryWip` / `InventoryFinished` balance to the exact cent.
    /// 3. `CapTable` share conservation: for every corporation, the sum of `owned_shares` across all
    ///    holders in `equity_positions` equals `corp.total_shares_outstanding` and `reserved_shares <= owned_shares`.
    pub fn audit_world_invariants(&self) -> Result<(), CoreError> {
        self.ledger.audit_trial_balance()?;

        for (corp_id, corp) in &self.corporations {
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

            let mut total_cap_table_shares = 0u64;
            for (&(cid, _), pos) in &self.equity_positions {
                if cid != corp_id {
                    continue;
                }
                if pos.reserved_shares > pos.owned_shares {
                    return Err(CoreError::Ledger(LedgerError::TrialBalanceViolation {
                        reason: format!(
                            "corporation {} CapTable violation: reserved_shares ({}) > owned_shares ({})",
                            corp.ticker, pos.reserved_shares, pos.owned_shares
                        ),
                    }));
                }
                total_cap_table_shares = total_cap_table_shares.saturating_add(pos.owned_shares);
            }

            if total_cap_table_shares != corp.total_shares_outstanding {
                return Err(CoreError::Ledger(LedgerError::TrialBalanceViolation {
                    reason: format!(
                        "corporation {} ({}) CapTable share conservation mismatch: sum(owned_shares) ({total_cap_table_shares}) != total_shares_outstanding ({})",
                        corp.ticker, corp.name, corp.total_shares_outstanding
                    ),
                }));
            }
        }

        // 4. Depositary Receipt unit conservation
        for (&inst_id, &expected_units) in &self.depositary_total_units {
            let mut sum_units = 0u64;
            for (&(iid, _), pos) in &self.depositary_positions {
                if iid != inst_id {
                    continue;
                }
                if pos.reserved_shares > pos.owned_shares {
                    return Err(CoreError::Ledger(LedgerError::TrialBalanceViolation {
                        reason: format!(
                            "DR {inst_id:?} reserved_shares ({}) > owned_shares ({})",
                            pos.reserved_shares, pos.owned_shares
                        ),
                    }));
                }
                sum_units = sum_units.saturating_add(pos.owned_shares);
            }
            if sum_units != expected_units {
                return Err(CoreError::Ledger(LedgerError::TrialBalanceViolation {
                    reason: format!(
                        "DR {inst_id:?} unit conservation mismatch: sum(owned_shares) ({sum_units}) != depositary_total_units ({expected_units})"
                    ),
                }));
            }
        }

        // 5. ServiceCredit (`PLEX`) unit conservation
        let mut sum_sc = 0u64;
        for pos in self.service_credit_positions.values() {
            if pos.reserved_credits > pos.owned_credits {
                return Err(CoreError::Ledger(LedgerError::TrialBalanceViolation {
                    reason: format!(
                        "ServiceCredit reserved_credits ({}) > owned_credits ({})",
                        pos.reserved_credits, pos.owned_credits
                    ),
                }));
            }
            sum_sc = sum_sc.saturating_add(pos.owned_credits);
        }
        if sum_sc != self.total_service_credits_supply {
            return Err(CoreError::Ledger(LedgerError::TrialBalanceViolation {
                reason: format!(
                    "ServiceCredit supply mismatch: sum(owned_credits) ({sum_sc}) != total_service_credits_supply ({})",
                    self.total_service_credits_supply
                ),
            }));
        }

        Ok(())
    }

    /// Projects an immutable `WorldView` snapshot for local/admin inspection (`viewer_corp_id = None`).
    pub fn build_world_view(&self) -> Result<WorldView, CoreError> {
        self.build_authorized_world_view(None)
    }

    /// Guard #6: Projects an information-barrier-filtered `WorldView` for a specific authenticated
    /// `viewer_corp_id` (or full admin/local view when `viewer_corp_id` is `None`).
    ///
    /// Masks:
    /// - Rival `illicit_cash_balance` and `monthly_laundered_cash` (unless wiretapped by `viewer_corp_id`).
    /// - Rival private `wire_messages` (unless `viewer_corp_id` is sender, recipient, or holds an active wiretap).
    /// - Individual `ShortPositionRecord`s (exposing only aggregate `short_interest_shares` on `InstrumentSnapshot`,
    ///   plus the viewer's own short positions or wiretapped targets' short positions).
    pub fn build_authorized_world_view(
        &self,
        viewer_corp_id: Option<CorpId>,
    ) -> Result<WorldView, CoreError> {
        let tick = self.current_tick;
        let bank_id = self.bank.bank_id()?;
        let bank_statement = FinancialStatementReport::generate(&self.ledger, bank_id, tick.0)?;
        let total_m1 = self
            .ledger
            .entity_balance(bank_id, StandardAccountCode::CustomerDepositsLiability)?;
        let total_loans = self
            .ledger
            .entity_balance(bank_id, StandardAccountCode::LoansReceivable)?;
        let sovereign_cash_balance = self.ledger.cash_balance(self.sovereign_entity_id)?;

        let can_inspect_corp = |target_cid: CorpId| -> bool {
            let Some(vcid) = viewer_corp_id else {
                return true;
            };
            if vcid == target_cid {
                return true;
            }
            self.active_wiretaps
                .get(&(vcid, target_cid))
                .is_some_and(|&exp| exp > tick.0)
        };

        let mut instruments = Vec::with_capacity(self.order_books.len());
        for (_, book) in &self.order_books {
            let short_interest_shares = match book.kind {
                InstrumentKind::Equity { corp_id } => self
                    .short_positions
                    .values()
                    .filter(|s| s.corp_id == corp_id)
                    .map(|s| s.borrowed_shares)
                    .sum(),
                _ => 0,
            };
            instruments.push(InstrumentSnapshot {
                instrument_id: book.instrument_id,
                ticker: book.ticker.clone(),
                kind: book.kind,
                last_price: book.last_price,
                luld_reference_price: book.luld_reference_price,
                luld_band_bps: book.luld_band_bps,
                short_interest_shares,
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
            let is_authorized = can_inspect_corp(corp.corp_id);
            let cash_balance = self.ledger.cash_balance(corp.entity_id)?;
            let available_cash = self.ledger.available_cash(corp.entity_id)?;
            let illicit_cash_balance = if is_authorized {
                self.ledger.illicit_cash_balance(corp.entity_id)?
            } else {
                Money::ZERO
            };
            let tax_lien_payable = self
                .tax_liens
                .get(&corp.entity_id)
                .copied()
                .unwrap_or(Money::ZERO);
            let service_credits = self
                .service_credit_positions
                .get(&corp.entity_id)
                .map_or(0, |p| p.available_credits());
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
            let effective_loan_rate_bps = self
                .bank
                .default_loan_rate_bps
                .saturating_add(corp.credit_rating.spread_bps());

            let total_shares = corp.total_shares_outstanding.max(1);
            let mut cap_table: Vec<ShareholderRecord> = self
                .equity_positions
                .iter()
                .filter_map(|(&(cid, holder_id), pos)| {
                    if cid == corp.corp_id && pos.owned_shares > 0 {
                        let holder_name = self
                            .ledger
                            .entity(holder_id)
                            .map_or_else(|_| "Shareholder".to_owned(), |e| e.name.clone());
                        let ownership_bps = u32::try_from(
                            (u128::from(pos.owned_shares) * 10_000) / u128::from(total_shares),
                        )
                        .unwrap_or(0);
                        Some(ShareholderRecord {
                            holder_entity_id: holder_id,
                            holder_name,
                            owned_shares: pos.owned_shares,
                            reserved_shares: pos.reserved_shares,
                            ownership_bps,
                        })
                    } else {
                        None
                    }
                })
                .collect();
            cap_table.sort_by(|a, b| {
                b.owned_shares
                    .cmp(&a.owned_shares)
                    .then_with(|| a.holder_entity_id.cmp(&b.holder_entity_id))
            });

            let eff_cap = if corp.ransomware_degraded_until_tick > tick.0 {
                ((corp.effective_daily_capacity().saturating_mul(60)) / 100).max(1)
            } else {
                corp.effective_daily_capacity()
            };

            corporations.push(CorporationSnapshot {
                corp_id: corp.corp_id,
                entity_id: corp.entity_id,
                equity_instrument_id: corp.equity_instrument_id,
                ticker: corp.ticker.clone(),
                name: corp.name.clone(),
                role: corp.role,
                persona_kind: corp.persona_kind,
                delegation: corp.delegation,
                cash_balance,
                available_cash,
                illicit_cash_balance,
                tax_lien_payable,
                service_credits,
                cloud_tier: corp.cloud_tier,
                subscribed_cloud_tier: corp.subscribed_cloud_tier,
                credit_rating: corp.credit_rating,
                effective_loan_rate_bps,
                loans_payable,
                daily_capacity: corp.daily_capacity,
                effective_daily_capacity: eff_cap,
                facility_condition_bps: corp.facility_condition_bps,
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
                last_monthly_dividend_paid: corp.last_monthly_dividend_paid,
                founded_tick: corp.founded_tick,
                founder_lockup_until_tick: corp.founder_lockup_until_tick,
                monthly_genuine_b2c_revenue: corp.monthly_genuine_b2c_revenue,
                monthly_laundered_cash: if is_authorized {
                    corp.monthly_laundered_cash
                } else {
                    Money::ZERO
                },
                ransomware_degraded_until_tick: corp.ransomware_degraded_until_tick,
                security_shield_until_tick: corp.security_shield_until_tick,
                logistics_shield_until_tick: corp.logistics_shield_until_tick,
                cap_table,
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

        let wire_messages: Vec<WireMessage> = self
            .wire_messages
            .iter()
            .filter(|m| {
                can_inspect_corp(m.sender_corp_id) || can_inspect_corp(m.recipient_corp_id)
            })
            .cloned()
            .collect();

        let visible_short_positions: Vec<ShortPositionRecord> = self
            .short_positions
            .values()
            .filter(|s| {
                if viewer_corp_id.is_none() {
                    return true;
                }
                self.find_corp_by_entity(s.borrower_entity_id)
                    .is_some_and(can_inspect_corp)
            })
            .cloned()
            .collect();

        let mut sku_codes = BTreeMap::new();
        for (sku_id, sku) in &self.skus {
            sku_codes.insert(sku_id, sku.code.clone());
        }

        let macro_telemetry = self.compute_macro_telemetry(&corporations);

        Ok(WorldView {
            tick,
            date: tick.to_date360(),
            bank_entity_id: bank_id,
            bank_statement,
            bank_loan_rate_bps: self.bank.default_loan_rate_bps,
            bank_deposit_rate_bps: self.bank.deposit_rate_bps,
            sovereign_entity_id: self.sovereign_entity_id,
            sovereign_cash_balance,
            sovereign_dr_reserve_buffer: self.sovereign_dr_reserve_buffer,
            total_m1_money_supply: total_m1,
            total_bank_loans: total_loans,
            total_service_credits_supply: self.total_service_credits_supply,
            macro_telemetry,
            corporations,
            cohorts,
            instruments,
            active_b2b_contracts: self.b2b_contracts.clone(),
            wire_messages,
            visible_short_positions,
            news_feed: self.news_feed.iter().cloned().collect(),
            sku_codes,
        })
    }

    fn compute_macro_telemetry(
        &self,
        corporations: &[CorporationSnapshot],
    ) -> MacroTelemetrySnapshot {
        let mut retail_cpi_sum = 0i128;
        let mut retail_count = 0i128;
        let mut cond_sum = 0u64;
        let mut qual_sum = 0u64;

        for c in corporations {
            cond_sum = cond_sum.saturating_add(u64::from(c.facility_condition_bps));
            qual_sum = qual_sum.saturating_add(u64::from(c.quality_score_bps));
            if let CorporationRole::Retailer { retail_sku } = c.role {
                let ref_px = self
                    .skus
                    .get(retail_sku)
                    .map_or(22_000, |s| s.base_reference_price.cents().max(1));
                retail_cpi_sum += (c.posted_selling_price.cents() * 10_000) / ref_px;
                retail_count += 1;
            }
        }

        let retail_cpi_index_bps = if retail_count > 0 {
            u32::try_from((retail_cpi_sum / retail_count).clamp(1_000, 50_000)).unwrap_or(10_000)
        } else {
            10_000
        };
        let total_corps = u64::try_from(corporations.len().max(1)).unwrap_or(1);
        let avg_facility_condition_bps = u32::try_from(cond_sum / total_corps).unwrap_or(10_000);
        let avg_quality_score_bps = u32::try_from(qual_sum / total_corps).unwrap_or(10_000);

        let extractor_hhi = Self::compute_sector_hhi(corporations, |r| {
            matches!(r, CorporationRole::ResourceExtractor { .. })
        });
        let manufacturer_hhi = Self::compute_sector_hhi(corporations, |r| {
            matches!(r, CorporationRole::Manufacturer { .. })
        });
        let retail_hhi = Self::compute_sector_hhi(corporations, |r| {
            matches!(r, CorporationRole::Retailer { .. })
        });

        let gdp = if self.trailing_30d_gdp.is_positive() {
            self.trailing_30d_gdp
        } else {
            self.current_month_gdp_accumulator
        };

        MacroTelemetrySnapshot {
            retail_cpi_index_bps,
            trailing_30d_gdp: gdp,
            avg_facility_condition_bps,
            avg_quality_score_bps,
            extractor_hhi,
            manufacturer_hhi,
            retail_hhi,
        }
    }

    fn compute_sector_hhi<F>(corporations: &[CorporationSnapshot], filter: F) -> u32
    where
        F: Fn(CorporationRole) -> bool,
    {
        let weights: Vec<u128> = corporations
            .iter()
            .filter(|c| filter(c.role))
            .map(|c| {
                let rev = u128::try_from(c.financial_statement.total_revenue.cents().max(0))
                    .unwrap_or(0);
                let cap = u128::from(c.effective_daily_capacity.max(1)) * 1_000;
                rev.saturating_add(cap)
            })
            .collect();
        let sum_w: u128 = weights.iter().copied().sum();
        if sum_w == 0 {
            return 0;
        }
        let mut hhi = 0u128;
        for w in weights {
            let pct = (w * 100) / sum_w;
            hhi = hhi.saturating_add(pct * pct);
        }
        u32::try_from(hhi.min(10_000)).unwrap_or(10_000)
    }

    /// Builds a lightweight `ClobDeltaEvent` for an `InstrumentId` after an intraday order action.
    pub fn build_clob_delta_event(
        &self,
        instrument_id: InstrumentId,
        seq_no: u32,
    ) -> Result<crate::wire::ClobDeltaEvent, CoreError> {
        let book = self
            .order_books
            .get(instrument_id)
            .ok_or(CoreError::UnknownInstrument(instrument_id))?;
        Ok(crate::wire::ClobDeltaEvent {
            tick: self.current_tick.0,
            seq_no,
            instrument_id,
            ticker: book.ticker.clone(),
            last_price: book.last_price,
            best_bid: book.best_bid(),
            best_ask: book.best_ask(),
            top_bids: book.top_bid_levels(10),
            top_asks: book.top_ask_levels(10),
            recent_fills: book.recent_fills.iter().cloned().collect(),
        })
    }
}
