use crate::error::AiError;
use crate::observation::ActorObservationView;
use oligopole_core::{JointCapitalAction, decimal_bytes_serde};
use oligopole_ledger::Money;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

/// A pre-bundled strategic `JointCapitalAction` candidate paired with its estimated NPV
/// and persona affinity sensitivities (`risk_weight`, `leverage_weight`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateOption {
    pub label: String,
    pub action: JointCapitalAction,
    pub estimated_npv: Money,
    #[serde(with = "decimal_bytes_serde")]
    pub risk_weight: Decimal,
    #[serde(with = "decimal_bytes_serde")]
    pub leverage_weight: Decimal,
}

/// Generates NPV-evaluated `CandidateOption` tuples for an actor at a Monthly Slow Tick.
pub fn generate_joint_action_candidates(
    obs: &ActorObservationView,
) -> Result<Vec<CandidateOption>, AiError> {
    let daily_payroll = obs.daily_wage_bill.checked_add(obs.daily_manager_bill)?;
    let (daily_interest, _) = obs
        .outstanding_debt
        .mul_div_rem_u64(u64::from(obs.bank_loan_rate_bps), 360 * 10_000)?;
    let daily_burn = daily_payroll.checked_add(daily_interest)?;
    let working_floor = daily_burn.checked_mul_u64(20)?;
    let free_cash = if obs.available_cash > working_floor {
        obs.available_cash.checked_sub(working_floor)?
    } else {
        Money::ZERO
    };

    // Estimate unit gross margin
    let unit_margin = if obs.posted_selling_price > obs.input_market_price {
        obs.posted_selling_price
            .checked_sub(obs.input_market_price)?
    } else {
        Money::from_dollars(10)
    };

    let expansion_units = (obs.daily_capacity / 10).max(5);
    let capex_cost = Money::from_dollars(45_000);
    // 180-day (half-year) discounted gross profit minus CapEx cost
    let gross_180d = unit_margin
        .checked_mul_u64(expansion_units)?
        .checked_mul_u64(180)?;
    let npv_cash_expand = gross_180d.checked_sub(capex_cost)?;

    let interest_180d = Money::from_cents(
        (capex_cost.cents() * i128::from(obs.bank_loan_rate_bps)) / (10_000 * 2),
    );
    let npv_debt_expand = npv_cash_expand.checked_sub(interest_180d)?;

    let delever_amount = Money::from_cents(free_cash.cents() / 3).min(obs.outstanding_debt);
    let npv_delever = Money::from_cents(
        (delever_amount.cents() * i128::from(obs.bank_loan_rate_bps)) / 10_000,
    );

    let campaign_cost = Money::from_dollars(20_000);
    let npv_campaign = Money::from_cents(gross_180d.cents() / 3).checked_sub(campaign_cost)?;

    let excess_90d = daily_burn.checked_mul_u64(90)?;
    let surplus_above_90d = if obs.available_cash > excess_90d {
        obs.available_cash.checked_sub(excess_90d)?
    } else {
        Money::ZERO
    };
    let buyback_budget =
        Money::from_cents(surplus_above_90d.cents() / 4).min(Money::from_dollars(25_000));
    let buyback_max_px = Money::from_cents(((obs.share_price.cents() * 103) / 100).max(200));
    let buyback_npv = if buyback_budget.is_positive() && obs.book_value_per_share >= obs.share_price
    {
        Money::from_cents(buyback_budget.cents() / 3)
    } else if buyback_budget.is_positive() {
        Money::from_cents(buyback_budget.cents() / 8)
    } else {
        Money::from_dollars(-50_000)
    };

    let overhaul_cost = Money::from_dollars(18_000);
    let cond_deficit_bps = 10_000u32.saturating_sub(obs.facility_condition_bps);
    let restored_units = ((obs.daily_capacity * u64::from(cond_deficit_bps.min(1_500))) / 10_000)
        .max(u64::from(cond_deficit_bps >= 500));
    let overhaul_gross_180d = unit_margin
        .checked_mul_u64(restored_units)?
        .checked_mul_u64(180)?;
    let npv_overhaul = if cond_deficit_bps >= 400 && free_cash >= overhaul_cost {
        overhaul_gross_180d
            .checked_add(Money::from_dollars(8_000))?
            .checked_sub(overhaul_cost)?
    } else {
        Money::from_dollars(-25_000)
    };

    let candidates = vec![
        CandidateOption {
            label: "HOLD_AND_DELEVER".to_owned(),
            action: JointCapitalAction::HoldAndDelever {
                repay_amount: delever_amount,
            },
            estimated_npv: npv_delever,
            risk_weight: dec!(-0.6),
            leverage_weight: dec!(-0.8),
        },
        CandidateOption {
            label: "EXPAND_CAPACITY_CASH".to_owned(),
            action: JointCapitalAction::ExpandCapacityCashFunded {
                added_capacity: expansion_units,
                capex_cost,
                estimated_npv: npv_cash_expand,
            },
            estimated_npv: npv_cash_expand,
            risk_weight: dec!(0.5),
            leverage_weight: dec!(-0.2),
        },
        CandidateOption {
            label: "EXPAND_CAPACITY_DEBT".to_owned(),
            action: JointCapitalAction::ExpandCapacityDebtFunded {
                added_capacity: expansion_units,
                capex_cost,
                loan_principal: capex_cost,
                estimated_npv: npv_debt_expand,
            },
            estimated_npv: npv_debt_expand,
            risk_weight: dec!(0.8),
            leverage_weight: dec!(0.9),
        },
        CandidateOption {
            label: "BRAND_QUALITY_CAMPAIGN".to_owned(),
            action: JointCapitalAction::BrandAndQualityCampaign {
                campaign_cost,
                quality_boost_bps: 250,
                brand_boost_bps: 350,
                estimated_npv: npv_campaign,
            },
            estimated_npv: npv_campaign,
            risk_weight: dec!(0.3),
            leverage_weight: dec!(0.0),
        },
        CandidateOption {
            label: "REPURCHASE_SHARES".to_owned(),
            action: JointCapitalAction::RepurchaseShares {
                buyback_budget,
                max_limit_price: buyback_max_px,
            },
            estimated_npv: buyback_npv,
            risk_weight: dec!(-0.2),
            leverage_weight: dec!(-0.4),
        },
        CandidateOption {
            label: "MAINTENANCE_RD_OVERHAUL".to_owned(),
            action: JointCapitalAction::MaintenanceAndRdOverhaul {
                overhaul_cost,
                condition_restore_bps: 1_500,
                quality_boost_bps: 200,
                estimated_npv: npv_overhaul,
            },
            estimated_npv: npv_overhaul,
            risk_weight: dec!(0.1),
            leverage_weight: dec!(-0.3),
        },
    ];

    Ok(candidates)
}
