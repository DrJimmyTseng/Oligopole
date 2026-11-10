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
    assert_eq!(
        world.corporations.len(),
        6,
        "Vertical Slice 0 should have 6 corporations (2 extractors, 2 manufacturers, 2 retailers)"
    );

    // Originate a test bank loan on Day 1 so the bank earns net interest spread and recycles it at month-end
    world
        .execute_command(PlayerCommand::RequestBankLoan {
            corp_id: handles.acme_corp,
            principal: Money::from_dollars(200_000),
        })
        .unwrap();

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

    // Verify closed-loop secondary Equity CLOB market-making produced active fills and L2 depth
    for eq_ticker in ["IRON EQ", "VALE EQ", "ACME EQ", "ZENI EQ", "PLYR EQ", "APEX EQ"] {
        let eq_book = view
            .instruments
            .iter()
            .find(|i| i.ticker == eq_ticker)
            .unwrap();
        assert!(
            !eq_book.recent_fills.is_empty(),
            "expected secondary equity CLOB fills on {eq_ticker}"
        );
        assert!(
            eq_book.best_bid.is_some() && eq_book.best_ask.is_some(),
            "expected two-sided resting L2 depth on {eq_ticker}"
        );
    }

    // Verify all solvent corporations maintained positive working capital above $0.00
    for corp in &view.corporations {
        assert!(
            corp.available_cash.is_positive(),
            "corporation {} should maintain positive working-capital cash buffer",
            corp.ticker
        );
    }
}

#[test]
fn test_player_manual_vs_ai_autopilot_delegation_enforcement() {
    let (mut world, handles) = build_vertical_slice_0([11u8; 32]).unwrap();
    let plyr_id = handles.player_retail_corp;

    // Switch PLYR to full Manual Executive Control
    world
        .execute_command(PlayerCommand::UpdateAiDelegation {
            corp_id: plyr_id,
            delegate_pricing: false,
            delegate_capex: false,
            delegate_procurement: false,
        })
        .unwrap();

    let manual_price = Money::from_dollars(231);
    world
        .execute_command(PlayerCommand::SetCorporationPrice {
            corp_id: plyr_id,
            new_price: manual_price,
        })
        .unwrap();

    assert!(
        !world
            .corporations
            .get(plyr_id)
            .unwrap()
            .delegation
            .accepts_slow_tick_ai()
    );

    // Even if a StrategicDirective is sent to PLYR while pricing & capex delegation are false,
    // `apply_reconciled_directive` must preserve the player's manual selling price and wage bill.
    let initial_wage = world.corporations.get(plyr_id).unwrap().daily_wage_bill;
    world
        .execute_command(PlayerCommand::ApplyStrategicDirective(StrategicDirective {
            corp_id: plyr_id,
            capital_action: JointCapitalAction::HoldAndDelever {
                repay_amount: Money::ZERO,
            },
            pricing_stance: PricingStance::AggressiveUndercut,
            wage_stance: WageStance::Raise5Pct,
            dividend_stance: DividendStance::High70Pct,
            confidence_score: 9,
        }))
        .unwrap();

    let plyr_after = world.corporations.get(plyr_id).unwrap();
    assert_eq!(plyr_after.posted_selling_price, manual_price);
    assert_eq!(plyr_after.daily_wage_bill, initial_wage);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn prop_vertical_slice_0_conserves_money_wac_and_captable_under_buybacks_and_issuance(
        months in 1usize..=6,
        pricing_idx in 0usize..4,
        wage_idx in 0usize..3,
        div_idx in 0usize..4,
        capex_mode in 0usize..6,
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
                    3 => JointCapitalAction::BrandAndQualityCampaign {
                        campaign_cost: Money::from_dollars(25_000),
                        quality_boost_bps: 200,
                        brand_boost_bps: 300,
                        estimated_npv: Money::from_dollars(45_000),
                    },
                    4 => JointCapitalAction::RepurchaseShares {
                        buyback_budget: Money::from_dollars(35_000),
                        max_limit_price: Money::from_dollars(65),
                    },
                    _ => JointCapitalAction::IssueEquityShares {
                        shares_to_issue: 2_500,
                        min_ask_price: Money::from_dollars(30),
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

#[test]
fn test_all_six_black_tape_invariant_guards_and_sme_ipo_pipeline() {
    use oligopole_core::{
        CorporationRole, CrimeOperationKind, MAX_RESOLVED_WIRE_MESSAGES_PER_CORP, WireAttachment,
        WireAttachmentStatus,
    };
    use rand::RngCore;

    let (mut world, handles) = build_vertical_slice_0([99u8; 32]).unwrap();
    let plyr_id = handles.player_retail_corp;
    let apex_id = handles.apex_retail_corp;
    let acme_id = handles.acme_corp;
    let plyr_entity = world.corporations.get(plyr_id).unwrap().entity_id;

    // --- Guard #5: Webhook Idempotency & Dual-Clock (tick, seq_no) RNG ---
    let initial_sc = world.total_service_credits_supply;
    world
        .execute_command_sequenced(
            PlayerCommand::AuthorityMintServiceCredit {
                recipient_entity_id: plyr_entity,
                credits: 25,
                webhook_ref: "wh_stripe_001".to_owned(),
            },
            1,
        )
        .unwrap();
    assert_eq!(world.total_service_credits_supply, initial_sc + 25);

    // Duplicate webhook_ref must be a no-op
    world
        .execute_command_sequenced(
            PlayerCommand::AuthorityMintServiceCredit {
                recipient_entity_id: plyr_entity,
                credits: 25,
                webhook_ref: "wh_stripe_001".to_owned(),
            },
            2,
        )
        .unwrap();
    assert_eq!(world.total_service_credits_supply, initial_sc + 25);

    // Distinct seq_no produces distinct ChaCha8Rng streams
    let rng_seq_1 = world.seeded_command_rng(1).next_u64();
    let rng_seq_2 = world.seeded_command_rng(2).next_u64();
    assert_ne!(rng_seq_1, rng_seq_2);

    // --- Guard #3: Oracle LULD Re-Anchoring on >10% Real-World Gap ---
    let googl_id = handles.googl_dr_book;
    // Jump GOOGL-DR from $281.00 to $350.00 (+24.5% gap)
    let new_oracle_px = Money::from_dollars(350);
    world
        .execute_command(PlayerCommand::IngestOraclePriceTick {
            instrument_id: googl_id,
            new_oracle_price: new_oracle_px,
        })
        .unwrap();
    let googl_book = world.order_books.get(googl_id).unwrap();
    assert_eq!(googl_book.luld_reference_price, new_oracle_px);
    // Submitting a limit order near $350.00 succeeds without LuldBandViolation
    world
        .execute_command(PlayerCommand::SubmitClobOrder {
            trader_entity_id: plyr_entity,
            instrument_id: googl_id,
            side: OrderSide::Bid,
            limit_price: Money::from_dollars(354),
            qty: 10,
        })
        .unwrap();

    // --- Guard #2: Physical Share-Borrow Short-Locate, Dividend-in-Lieu & 115% Maintenance Margin ---
    let acme_eq_id = world
        .corporations
        .get(acme_id)
        .unwrap()
        .equity_instrument_id
        .unwrap();
    let acme_px = world.order_books.get(acme_eq_id).unwrap().last_price;
    world
        .execute_command(PlayerCommand::BorrowSharesToShort {
            borrower_entity_id: plyr_entity,
            corp_id: acme_id,
            shares: 200,
            sell_limit_price: acme_px,
        })
        .unwrap();
    assert_eq!(world.short_positions.len(), 1);
    world.audit_world_invariants().unwrap();

    // --- Crime, Laundering & Guard #4: Non-Negative Balance Protection on 150% Forensic Fines ---
    // Extort cash into PLYR's IllicitCash (try multiple seq_nos to guarantee a successful roll)
    for seq in 10..25 {
        world
            .execute_command_sequenced(
                PlayerCommand::ExecuteCrimeOperation {
                    actor_corp_id: plyr_id,
                    target_corp_id: apex_id,
                    kind: CrimeOperationKind::ExtortRetailer {
                        demand_amount: Money::from_dollars(80_000),
                    },
                },
                seq,
            )
            .unwrap();
        if world
            .ledger
            .illicit_cash_balance(plyr_entity)
            .unwrap()
            .is_positive()
        {
            break;
        }
    }
    let illicit_before = world.ledger.illicit_cash_balance(plyr_entity).unwrap();
    assert!(illicit_before.is_positive(), "expected extortion to yield IllicitCash");

    // Lock most of PLYR's clean cash in a resting Bid so a 150% fine exceeds unreserved cash and triggers a TaxLien
    let avail_clean = world.ledger.available_cash(plyr_entity).unwrap();
    let lock_budget = Money::from_cents((avail_clean.cents() * 95) / 100);
    let iron_eq_id = world
        .corporations
        .get(handles.iron_corp)
        .unwrap()
        .equity_instrument_id
        .unwrap();
    let iron_bid_px = Money::from_dollars(25);
    let lock_qty = u64::try_from(lock_budget.cents() / iron_bid_px.cents()).unwrap();
    let (locked_order_id, _) = world
        .submit_clob_limit_order(plyr_entity, iron_eq_id, OrderSide::Bid, iron_bid_px, lock_qty)
        .unwrap();

    // Launder all IllicitCash at once (> 2x safe ceiling -> 100% deterministic 150% fine trigger)
    world
        .execute_command_sequenced(
            PlayerCommand::LaunderIllicitCash {
                corp_id: plyr_id,
                amount: illicit_before,
            },
            99,
        )
        .unwrap();
    world.audit_world_invariants().unwrap();
    assert!(
        world
            .tax_liens
            .get(&plyr_entity)
            .copied()
            .unwrap_or(Money::ZERO)
            .is_positive(),
        "expected excess 150% forensic fine to book a senior TaxLienPayable without negative cash"
    );

    let _ = world.cancel_clob_limit_order(iron_eq_id, locked_order_id);

    // --- Domestic Private SME -> IPO Gate Pipeline ---
    let sme_id = world
        .execute_found_private_sme(
            plyr_entity,
            "BOLT",
            "Bolt FastFreight Logistics SME",
            CorporationRole::LogisticsTrucking {
                freight_sku: handles.appliance_sku,
            },
            Money::from_dollars(260_000),
        )
        .unwrap();
    // Launching IPO before 90 ticks must fail
    assert!(
        world
            .execute_launch_ipo_auction(sme_id, 20_000, Money::from_dollars(12))
            .is_err()
    );

    // --- Guard #6: Information-Barrier Masking & Bounded Wire Messages ---
    for i in 0..80 {
        world
            .execute_send_wire_message(
                plyr_id,
                apex_id,
                format!("Wire #{i}"),
                "Routine status ping".to_owned(),
                None,
            )
            .unwrap();
    }
    assert!(
        world.wire_messages.len() <= MAX_RESOLVED_WIRE_MESSAGES_PER_CORP,
        "resolved wire messages must be bounded to MAX_RESOLVED_WIRE_MESSAGES_PER_CORP"
    );

    // Send an actionable B2B Smart Attachment and accept it
    let msg_id = world
        .execute_send_wire_message(
            acme_id,
            plyr_id,
            "Wholesale Appliance Contract".to_owned(),
            "10 units/day at $132".to_owned(),
            Some(WireAttachment::B2BContractOffer {
                sku_id: handles.appliance_sku,
                daily_qty: 10,
                unit_price: Money::from_dollars(132),
                duration_days: 30,
            }),
        )
        .unwrap();

    // Rival IRON Corp cannot see ACME <-> PLYR private wire or PLYR's short positions
    let iron_view = world
        .build_authorized_world_view(Some(handles.iron_corp))
        .unwrap();
    assert!(
        iron_view.wire_messages.is_empty(),
        "uninvolved rival should not see private wire messages"
    );
    assert!(
        iron_view.visible_short_positions.is_empty(),
        "uninvolved rival should not see individual short positions"
    );
    let acme_inst = iron_view
        .instruments
        .iter()
        .find(|i| i.instrument_id == acme_eq_id)
        .unwrap();
    assert_eq!(acme_inst.short_interest_shares, 200);

    world
        .execute_respond_wire_attachment(plyr_id, msg_id, true)
        .unwrap();
    let accepted_msg = world
        .wire_messages
        .iter()
        .find(|m| m.message_id == msg_id)
        .unwrap();
    assert_eq!(accepted_msg.status, WireAttachmentStatus::Accepted);

    // --- Guard #1: Sovereign Fiscal Recycling at Day 30 & 90-Tick SME IPO Unlock ---
    // Give BOLT SME a retainer contract so its net_income > 0 after 90 ticks
    let ret_msg = world
        .execute_send_wire_message(
            sme_id,
            apex_id,
            "Freight Shield Retainer".to_owned(),
            "90-day armored freight".to_owned(),
            Some(WireAttachment::ServiceRetainerOffer {
                is_security: false,
                daily_fee: Money::from_dollars(2_200),
                duration_days: 90,
            }),
        )
        .unwrap();
    world
        .execute_respond_wire_attachment(apex_id, ret_msg, true)
        .unwrap();

    for _ in 1..=91 {
        world.step_fast_tick(&[]).unwrap();
    }

    // Give BOLT SME a fresh Q2 retainer so current-quarter net_income is also positive
    let ret_msg_q2 = world
        .execute_send_wire_message(
            sme_id,
            acme_id,
            "Q2 Freight Shield Retainer".to_owned(),
            "30-day armored freight".to_owned(),
            Some(WireAttachment::ServiceRetainerOffer {
                is_security: false,
                daily_fee: Money::from_dollars(2_200),
                duration_days: 30,
            }),
        )
        .unwrap();
    world
        .execute_respond_wire_attachment(acme_id, ret_msg_q2, true)
        .unwrap();

    // Verify SovereignTreasury recycled surplus above $500k reserve at month-ends
    let sov_cash = world.ledger.cash_balance(world.sovereign_entity_id).unwrap();
    assert!(
        sov_cash >= world.sovereign_dr_reserve_buffer,
        "SovereignTreasury should maintain at least its DR reserve buffer"
    );

    // Now BOLT SME has >90 ticks operating history, >$250k equity, and positive net income -> IPO succeeds!
    let bolt_eq_inst = world
        .execute_launch_ipo_auction(sme_id, 25_000, Money::from_dollars(15))
        .unwrap();
    assert!(world.order_books.contains_key(bolt_eq_inst));
    world.audit_world_invariants().unwrap();
}


#[test]
fn test_vertical_slice_0_facility_maintenance_credit_ratings_news_and_cap_table() {
    let (mut world, handles) = build_vertical_slice_0([88u8; 32]).unwrap();
    let plyr_id = handles.player_retail_corp;
    let iron_id = handles.iron_corp;

    // 1. Run 30 Fast Ticks (1 simulated month) so extractors wear down facility condition
    for _ in 0..30 {
        world.step_fast_tick(&[]).unwrap();
    }

    let iron_cond_after_30d = world
        .corporations
        .get(iron_id)
        .unwrap()
        .facility_condition_bps;
    assert!(
        iron_cond_after_30d < 10_000,
        "daily extraction should wear down facility_condition_bps below 10_000 (got {iron_cond_after_30d})"
    );

    // 2. Execute Maintenance & R&D Overhaul on IRON Corp and verify condition + quality boost
    let iron_qual_before = world.corporations.get(iron_id).unwrap().quality_score_bps;
    world
        .execute_command(PlayerCommand::ExecuteFacilityMaintenance { corp_id: iron_id })
        .unwrap();
    let iron_after = world.corporations.get(iron_id).unwrap();
    assert_eq!(iron_after.facility_condition_bps, 10_000);
    assert_eq!(iron_after.quality_score_bps, iron_qual_before + 200);

    // 3. Draw a large bank loan on PLYR right before month-end (Tick 60) to trigger credit rating downgrade
    while world.current_tick.0 < 59 {
        world.step_fast_tick(&[]).unwrap();
    }
    world
        .step_fast_tick(&[PlayerCommand::RequestBankLoan {
            corp_id: plyr_id,
            principal: Money::from_dollars(1_200_000),
        }])
        .unwrap();
    let plyr_rating = world.corporations.get(plyr_id).unwrap().credit_rating;
    assert_ne!(
        plyr_rating,
        oligopole_core::CreditRating::AAA,
        "corporation with $1.2M debt should be downgraded from AAA at month-end"
    );

    // 4. Advance to Quarter-End (Tick 90) and verify WorldView macro telemetry, Cap Table, and News Wire
    while !world.current_tick.is_quarter_end() {
        world.step_fast_tick(&[]).unwrap();
    }
    let view = world.build_world_view().unwrap();
    assert!(view.macro_telemetry.retail_cpi_index_bps > 0);
    assert!(view.macro_telemetry.trailing_30d_gdp.is_positive());
    assert!(view.macro_telemetry.extractor_hhi > 0);
    assert!(
        !view.news_feed.is_empty(),
        "news wire should contain corporate headlines after 90 ticks"
    );

    let plyr_snap = view
        .corporations
        .iter()
        .find(|c| c.corp_id == plyr_id)
        .unwrap();
    let cap_shares_sum: u64 = plyr_snap.cap_table.iter().map(|r| r.owned_shares).sum();
    assert_eq!(cap_shares_sum, plyr_snap.total_shares_outstanding);
    world.audit_world_invariants().unwrap();
}
