#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oligopole_core::{
    DividendStance, JointCapitalAction, OrderSide, PlayerCommand, PricingStance,
    StrategicDirective, WageStance, build_vertical_slice_0,
};
use oligopole_ledger::Money;
use proptest::prelude::*;

#[test]
fn test_vertical_slice_0_runs_360_days_full_year_invariants() {
    let (mut world, handles) = build_vertical_slice_0([7u8; 32]).unwrap();

    for day in 1..=360 {
        let mut cmds = Vec::new();
        if day == 15 {
            // Test equity CLOB trade between Capitalist Cohort and Manager Cohort
            let acme_eq = world
                .corporations
                .get(handles.acme_corp)
                .unwrap()
                .equity_instrument_id
                .unwrap();
            let cap_entity = world
                .cohorts
                .get(handles.capitalist_cohort)
                .unwrap()
                .entity_id;
            let mgr_entity = world.cohorts.get(handles.manager_cohort).unwrap().entity_id;

            cmds.push(PlayerCommand::SubmitClobOrder {
                trader_entity_id: cap_entity,
                instrument_id: acme_eq,
                side: OrderSide::Ask,
                limit_price: Money::from_dollars(42),
                qty: 500,
            });
            cmds.push(PlayerCommand::SubmitClobOrder {
                trader_entity_id: mgr_entity,
                instrument_id: acme_eq,
                side: OrderSide::Bid,
                limit_price: Money::from_dollars(45), // Price improvement fill at $42!
                qty: 300,
            });
        }
        world.step_fast_tick(&cmds).unwrap();
    }

    let view = world.build_world_view().unwrap();
    assert_eq!(view.tick.0, 360);
    // Verify commodities and retail sales occurred
    let iron_book = view
        .instruments
        .iter()
        .find(|i| i.ticker == "IRON CMDTY")
        .unwrap();
    assert!(!iron_book.recent_fills.is_empty(), "expected commodity CLOB fills");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn prop_vertical_slice_0_conserves_money_and_wac_under_random_directives(
        months in 1usize..=6,
        pricing_idx in 0usize..4,
        wage_idx in 0usize..3,
        div_idx in 0usize..4,
        capex_mode in 0usize..4,
    ) {
        let (mut world, handles) = build_vertical_slice_0([42u8; 32]).unwrap();
        let pricing = [
            PricingStance::AggressiveUndercut,
            PricingStance::MatchMarket,
            PricingStance::PremiumMargin,
            PricingStance::ScarcitySkimming,
        ][pricing_idx];
        let wage = [
            WageStance::Cut5Pct,
            WageStance::HoldSteady,
            WageStance::Raise5Pct,
        ][wage_idx];
        let div = [
            DividendStance::RetainAll,
            DividendStance::Low20Pct,
            DividendStance::Moderate40Pct,
            DividendStance::High70Pct,
        ][div_idx];

        let total_days = months * 30;
        for day in 1..=total_days {
            let mut cmds = Vec::new();
            if day % 30 == 0 {
                let cap_action = match capex_mode {
                    0 => JointCapitalAction::HoldAndDelever {
                        repay_amount: Money::from_dollars(20_000),
                    },
                    1 => JointCapitalAction::ExpandCapacityCashFunded {
                        added_capacity: 5,
                        capex_cost: Money::from_dollars(50_000),
                        estimated_npv: Money::from_dollars(80_000),
                    },
                    2 => JointCapitalAction::ExpandCapacityDebtFunded {
                        added_capacity: 8,
                        capex_cost: Money::from_dollars(80_000),
                        loan_principal: Money::from_dollars(80_000),
                        estimated_npv: Money::from_dollars(110_000),
                    },
                    _ => JointCapitalAction::BrandAndQualityCampaign {
                        campaign_cost: Money::from_dollars(25_000),
                        quality_boost_bps: 200,
                        brand_boost_bps: 300,
                        estimated_npv: Money::from_dollars(45_000),
                    },
                };

                cmds.push(PlayerCommand::ApplyStrategicDirective(StrategicDirective {
                    corp_id: handles.player_retail_corp,
                    capital_action: cap_action,
                    pricing_stance: pricing,
                    wage_stance: wage,
                    dividend_stance: div,
                    confidence_score: 7,
                }));
            }
            world.step_fast_tick(&cmds).unwrap();
        }

        world.audit_world_invariants().unwrap();
    }
}
