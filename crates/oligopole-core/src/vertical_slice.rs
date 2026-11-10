use crate::clob::{CentralLimitOrderBook, InstrumentKind};
use crate::error::CoreError;
use crate::household::{HouseholdCohort, HouseholdCohortKind};
use crate::ids::{CohortId, CorpId, InstrumentId, SkuId};
use crate::production::{
    AiDelegationConfig, CloudTier, Corporation, CorporationRole, CreditRating,
    ExecutivePersonaKind, SkuDefinition,
};
use crate::wire::ServiceCreditPosition;
use crate::world::{EquityPosition, WorldState};
use oligopole_ledger::{EntityId, Money, StandardAccountCode};
use rust_decimal_macros::dec;
use std::collections::BTreeMap;

/// Handles to the canonical entities, SKUs, and instruments created in Vertical Slice 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerticalSliceHandles {
    pub iron_ore_sku: SkuId,
    pub appliance_sku: SkuId,
    pub iron_cmdty_book: InstrumentId,
    pub plex_sc_book: InstrumentId,
    pub googl_dr_book: InstrumentId,
    pub meta_dr_book: InstrumentId,
    pub nvda_dr_book: InstrumentId,
    pub iron_corp: CorpId,
    pub vale_corp: CorpId,
    pub acme_corp: CorpId,
    pub zenith_corp: CorpId,
    pub player_retail_corp: CorpId,
    pub apex_retail_corp: CorpId,
    pub wage_cohort: CohortId,
    pub manager_cohort: CohortId,
    pub capitalist_cohort: CohortId,
}

/// Constructs **Vertical Slice 0 ("Iron-to-Consumer + Equity CLOB")** inside a new `WorldState`.
pub fn build_vertical_slice_0(
    world_seed: [u8; 32],
) -> Result<(WorldState, VerticalSliceHandles), CoreError> {
    let mut world = WorldState::new(world_seed, 600, 180);

    // 1. Capitalize Central & Commercial Bank reserves & Sovereign Treasury initial reserve
    world
        .bank
        .seed_bank_equity_reserves(&mut world.ledger, 0, Money::from_dollars(25_000_000))?;
    world.bank.seed_initial_deposit_endowment(
        &mut world.ledger,
        0,
        world.sovereign_entity_id,
        Money::from_dollars(1_000_000),
    )?;

    // 2. Register SKUs
    let iron_ore_sku = world.skus.insert_with_key(|id| SkuDefinition {
        sku_id: id,
        code: "IRON".to_owned(),
        name: "Iron Ore (Metric Ton)".to_owned(),
        ledger_inventory_account: StandardAccountCode::InventoryRaw,
        base_reference_price: Money::from_dollars(42),
    });

    let appliance_sku = world.skus.insert_with_key(|id| SkuDefinition {
        sku_id: id,
        code: "APPL".to_owned(),
        name: "Smart Home Appliance".to_owned(),
        ledger_inventory_account: StandardAccountCode::InventoryFinished,
        base_reference_price: Money::from_dollars(220),
    });

    // 3. Register Commodity CLOB for IRON_ORE, ServiceCredit (`PLEX SC`), and Global Depositary Receipts (`GOOGL-DR`, `META-DR`, `NVDA-DR`)
    let iron_cmdty_book = world.order_books.insert_with_key(|id| {
        CentralLimitOrderBook::new(
            id,
            "IRON CMDTY",
            InstrumentKind::Commodity {
                sku_id: iron_ore_sku,
            },
            Money::from_dollars(42),
        )
    });

    let plex_sc_book = world.order_books.insert_with_key(|id| {
        CentralLimitOrderBook::new(
            id,
            "PLEX SC",
            InstrumentKind::ServiceCredit,
            Money::from_dollars(1_000),
        )
    });

    let googl_dr_book = register_depositary_receipt_book(
        &mut world,
        "GOOGL-DR EQ",
        *b"GOOGL   ",
        Money::from_dollars(281),
        25,
        10_000,
    );
    let meta_dr_book = register_depositary_receipt_book(
        &mut world,
        "META-DR EQ",
        *b"META    ",
        Money::from_dollars(590),
        20,
        10_000,
    );
    let nvda_dr_book = register_depositary_receipt_book(
        &mut world,
        "NVDA-DR EQ",
        *b"NVDA    ",
        Money::from_dollars(1_380),
        10,
        10_000,
    );

    // 4. Register the 3 Household Income Cohorts
    let wage_entity = world.ledger.register_entity("Wage Earners Cohort", false);
    world.bank.seed_initial_deposit_endowment(
        &mut world.ledger,
        0,
        wage_entity,
        Money::from_dollars(1_500_000),
    )?;
    let wage_cohort = world.cohorts.insert_with_key(|cid| HouseholdCohort {
        cohort_id: cid,
        entity_id: wage_entity,
        kind: HouseholdCohortKind::WageEarners,
        name: "Wage Earners Cohort".to_owned(),
        population_count: 50_000,
        savings_subsistence_floor: Money::from_dollars(250_000),
        daily_mpc_bps: 400,
        price_sensitivity: dec!(2.4),
        quality_affinity: dec!(1.0),
        brand_affinity: dec!(0.9),
    });

    let mgr_entity = world
        .ledger
        .register_entity("Managers & Skilled Cohort", false);
    world.bank.seed_initial_deposit_endowment(
        &mut world.ledger,
        0,
        mgr_entity,
        Money::from_dollars(1_200_000),
    )?;
    let manager_cohort = world.cohorts.insert_with_key(|cid| HouseholdCohort {
        cohort_id: cid,
        entity_id: mgr_entity,
        kind: HouseholdCohortKind::ManagersSkilled,
        name: "Managers & Skilled Cohort".to_owned(),
        population_count: 8_000,
        savings_subsistence_floor: Money::from_dollars(300_000),
        daily_mpc_bps: 320,
        price_sensitivity: dec!(1.4),
        quality_affinity: dec!(1.8),
        brand_affinity: dec!(1.5),
    });

    let cap_entity = world
        .ledger
        .register_entity("Capitalists & Executives Cohort", false);
    world.bank.seed_initial_deposit_endowment(
        &mut world.ledger,
        0,
        cap_entity,
        Money::from_dollars(3_000_000),
    )?;
    let capitalist_cohort = world.cohorts.insert_with_key(|cid| HouseholdCohort {
        cohort_id: cid,
        entity_id: cap_entity,
        kind: HouseholdCohortKind::CapitalistsExecutives,
        name: "Capitalists & Executives Cohort".to_owned(),
        population_count: 1_000,
        savings_subsistence_floor: Money::from_dollars(800_000),
        daily_mpc_bps: 220,
        price_sensitivity: dec!(0.6),
        quality_affinity: dec!(2.5),
        brand_affinity: dec!(2.2),
    });

    // 5. Register the 6 Corporations (2 Extractors, 2 Manufacturers, 2 Retailers) + their Equity CLOBs
    let iron_corp = register_slice_corporation(
        &mut world,
        "IRON",
        "IronWorks Mining Corp",
        CorporationRole::ResourceExtractor {
            output_sku: iron_ore_sku,
        },
        ExecutivePersonaKind::DividendAristocrat,
        AiDelegationConfig::autonomous_rival(),
        Money::from_dollars(480_000),
        65,
        Money::from_dollars(1_550),
        Money::from_dollars(450),
        Money::from_dollars(43),
        Money::from_dollars(10),
        10_200,
        10_000,
        3_500,
        Money::from_dollars(26),
        (wage_entity, mgr_entity, cap_entity),
    )?;

    let vale_corp = register_slice_corporation(
        &mut world,
        "VALE",
        "Vale Basin Resources Corp",
        CorporationRole::ResourceExtractor {
            output_sku: iron_ore_sku,
        },
        ExecutivePersonaKind::AggressiveEmpireBuilder,
        AiDelegationConfig::autonomous_rival(),
        Money::from_dollars(460_000),
        60,
        Money::from_dollars(1_450),
        Money::from_dollars(420),
        Money::from_dollars(41),
        Money::from_dollars(10),
        9_900,
        9_800,
        3_500,
        Money::from_dollars(24),
        (wage_entity, mgr_entity, cap_entity),
    )?;

    let acme_corp = register_slice_corporation(
        &mut world,
        "ACME",
        "Acme Appliances Corp",
        CorporationRole::Manufacturer {
            input_sku: iron_ore_sku,
            input_units_per_batch: 1,
            output_sku: appliance_sku,
            output_units_per_batch: 1,
        },
        ExecutivePersonaKind::QualityMoatCompounder,
        AiDelegationConfig::autonomous_rival(),
        Money::from_dollars(650_000),
        55,
        Money::from_dollars(2_100),
        Money::from_dollars(850),
        Money::from_dollars(134),
        Money::from_dollars(52),
        10_800,
        10_500,
        3_000,
        Money::from_dollars(40),
        (wage_entity, mgr_entity, cap_entity),
    )?;

    let zenith_corp = register_slice_corporation(
        &mut world,
        "ZENI",
        "Zenith Manufacturing Corp",
        CorporationRole::Manufacturer {
            input_sku: iron_ore_sku,
            input_units_per_batch: 1,
            output_sku: appliance_sku,
            output_units_per_batch: 1,
        },
        ExecutivePersonaKind::CostUndercutChallenger,
        AiDelegationConfig::autonomous_rival(),
        Money::from_dollars(600_000),
        50,
        Money::from_dollars(1_950),
        Money::from_dollars(800),
        Money::from_dollars(130),
        Money::from_dollars(50),
        10_200,
        10_000,
        3_000,
        Money::from_dollars(36),
        (wage_entity, mgr_entity, cap_entity),
    )?;

    let player_retail_corp = register_slice_corporation(
        &mut world,
        "PLYR",
        "Oligopole Retail Group (Player)",
        CorporationRole::Retailer {
            retail_sku: appliance_sku,
        },
        ExecutivePersonaKind::BalancedExecutive,
        AiDelegationConfig {
            is_player_controlled: true,
            delegate_pricing_to_ai: true,
            delegate_capex_and_financing_to_ai: true,
            delegate_procurement_to_ai: true,
        },
        Money::from_dollars(550_000),
        55,
        Money::from_dollars(1_200),
        Money::from_dollars(600),
        Money::from_dollars(224),
        Money::from_dollars(165),
        10_600,
        10_800,
        3_500,
        Money::from_dollars(50),
        (wage_entity, mgr_entity, cap_entity),
    )?;

    let apex_retail_corp = register_slice_corporation(
        &mut world,
        "APEX",
        "Apex MegaMart Corp (Rival)",
        CorporationRole::Retailer {
            retail_sku: appliance_sku,
        },
        ExecutivePersonaKind::OpportunisticCapitalAllocator,
        AiDelegationConfig::autonomous_rival(),
        Money::from_dollars(550_000),
        55,
        Money::from_dollars(1_150),
        Money::from_dollars(600),
        Money::from_dollars(218),
        Money::from_dollars(165),
        10_100,
        10_300,
        3_000,
        Money::from_dollars(48),
        (wage_entity, mgr_entity, cap_entity),
    )?;

    // Seed initial ServiceCredits (PLEX) to CapitalistsExecutives (500 SC) and Player Corp (10 SC)
    let player_entity = world
        .corporations
        .get(player_retail_corp)
        .map(|c| c.entity_id)
        .ok_or(CoreError::UnknownCorporation(player_retail_corp))?;
    world.service_credit_positions.insert(
        cap_entity,
        ServiceCreditPosition {
            owned_credits: 500,
            reserved_credits: 0,
        },
    );
    world.service_credit_positions.insert(
        player_entity,
        ServiceCreditPosition {
            owned_credits: 10,
            reserved_credits: 0,
        },
    );
    world.total_service_credits_supply = 510;

    // Seed initial Depositary Receipt holdings (8,000 held by Sovereign Market-Maker, 2,000 held by CapitalistsExecutives)
    for dr_book in [googl_dr_book, meta_dr_book, nvda_dr_book] {
        world.depositary_positions.insert(
            (dr_book, world.sovereign_entity_id),
            EquityPosition {
                owned_shares: 8_000,
                reserved_shares: 0,
            },
        );
        world.depositary_positions.insert(
            (dr_book, cap_entity),
            EquityPosition {
                owned_shares: 2_000,
                reserved_shares: 0,
            },
        );
    }

    world.audit_world_invariants()?;

    Ok((
        world,
        VerticalSliceHandles {
            iron_ore_sku,
            appliance_sku,
            iron_cmdty_book,
            plex_sc_book,
            googl_dr_book,
            meta_dr_book,
            nvda_dr_book,
            iron_corp,
            vale_corp,
            acme_corp,
            zenith_corp,
            player_retail_corp,
            apex_retail_corp,
            wage_cohort,
            manager_cohort,
            capitalist_cohort,
        },
    ))
}

fn register_depositary_receipt_book(
    world: &mut WorldState,
    ticker: &str,
    symbol_code: [u8; 8],
    initial_price: Money,
    quarterly_dividend_bps: u32,
    total_units: u64,
) -> InstrumentId {
    let inst_id = world.order_books.insert_with_key(|id| {
        CentralLimitOrderBook::new(
            id,
            ticker,
            InstrumentKind::DepositaryReceipt {
                symbol_code,
                quarterly_dividend_bps,
            },
            initial_price,
        )
    });
    world.depositary_total_units.insert(inst_id, total_units);
    inst_id
}

#[allow(clippy::too_many_arguments)]
fn register_slice_corporation(
    world: &mut WorldState,
    ticker: &str,
    name: &str,
    role: CorporationRole,
    persona_kind: ExecutivePersonaKind,
    delegation: AiDelegationConfig,
    initial_cash: Money,
    daily_capacity: u64,
    daily_wage_bill: Money,
    daily_manager_bill: Money,
    posted_selling_price: Money,
    max_input_bid_price: Money,
    quality_score_bps: u32,
    brand_score_bps: u32,
    target_dividend_payout_bps: u32,
    initial_share_price: Money,
    cohort_entities: (EntityId, EntityId, EntityId),
) -> Result<CorpId, CoreError> {
    let entity_id: EntityId = world.ledger.register_entity(name, false);
    world
        .bank
        .seed_initial_deposit_endowment(&mut world.ledger, 0, entity_id, initial_cash)?;

    let total_shares = 100_000u64;
    let corp_id = world.corporations.insert_with_key(|cid| Corporation {
        corp_id: cid,
        entity_id,
        equity_instrument_id: None,
        ticker: ticker.to_owned(),
        name: name.to_owned(),
        role,
        persona_kind,
        delegation,
        daily_capacity,
        daily_wage_bill,
        daily_manager_bill,
        posted_selling_price,
        max_input_bid_price,
        quality_score_bps,
        brand_score_bps,
        target_dividend_payout_bps,
        total_shares_outstanding: total_shares,
        inventory: BTreeMap::new(),
        monthly_units_output: 0,
        monthly_units_sold: 0,
        last_monthly_dividend_paid: Money::ZERO,
        founded_tick: 0,
        founder_entity_id: None,
        founder_lockup_until_tick: 0,
        founder_locked_shares: 0,
        cloud_tier: CloudTier::Tier1PosWire,
        subscribed_cloud_tier: CloudTier::Tier1PosWire,
        monthly_genuine_b2c_revenue: Money::ZERO,
        monthly_laundered_cash: Money::ZERO,
        ransomware_degraded_until_tick: 0,
        security_shield_until_tick: 0,
        logistics_shield_until_tick: 0,
        facility_condition_bps: 10_000,
        credit_rating: CreditRating::AAA,
    });

    let eq_ticker = format!("{ticker} EQ");
    let eq_book_id = world.order_books.insert_with_key(|iid| {
        CentralLimitOrderBook::new(
            iid,
            eq_ticker,
            InstrumentKind::Equity { corp_id },
            initial_share_price,
        )
    });

    if let Some(corp) = world.corporations.get_mut(corp_id) {
        corp.equity_instrument_id = Some(eq_book_id);
    }

    let (wage_ent, mgr_ent, cap_ent) = cohort_entities;
    world.equity_positions.insert(
        (corp_id, wage_ent),
        EquityPosition {
            owned_shares: 10_000,
            reserved_shares: 0,
        },
    );
    world.equity_positions.insert(
        (corp_id, mgr_ent),
        EquityPosition {
            owned_shares: 20_000,
            reserved_shares: 0,
        },
    );
    world.equity_positions.insert(
        (corp_id, cap_ent),
        EquityPosition {
            owned_shares: 70_000,
            reserved_shares: 0,
        },
    );

    Ok(corp_id)
}
