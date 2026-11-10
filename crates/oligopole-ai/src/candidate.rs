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
    let working_floor = daily_payroll.checked_mul_u64(15)?;
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
    ];

    Ok(candidates)
}
