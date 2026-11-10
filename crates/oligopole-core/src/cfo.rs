use crate::command::{
    DividendStance, JointCapitalAction, PricingStance, StrategicDirective, WageStance,
};
use crate::error::CoreError;
use crate::production::Corporation;
use oligopole_ledger::Money;

/// Deterministic corporate finance constraint solver ("Rust CFO Guardrail").
///
/// Reconciles a sampled `StrategicDirective` (where `JointCapitalAction` and policy dials
/// were evaluated in parallel by `Clef-Flash`) against the corporation's hard working-capital
/// and solvency constraints:
/// 1. Reserves a 15-day working-capital payroll floor (`15 * (daily_wage_bill + daily_manager_bill)`).
/// 2. If `ExpandCapacityCashFunded` or `BrandAndQualityCampaign` exceeds available cash above the floor,
///    downgrades to `HoldAndDelever { repay_amount: excess_cash / 4 }`.
/// 3. If cash after capital actions is below the working-capital floor, clips `DividendStance` to `RetainAll`
///    and prevents `WageStance::Raise5Pct`.
pub fn reconcile_strategic_directive(
    corp: &Corporation,
    available_cash: Money,
    outstanding_debt: Money,
    mut directive: StrategicDirective,
) -> Result<StrategicDirective, CoreError> {
    let daily_payroll = corp.daily_wage_bill.checked_add(corp.daily_manager_bill)?;
    let working_capital_floor = daily_payroll.checked_mul_u64(15)?;
    let free_cash_above_floor = if available_cash > working_capital_floor {
        available_cash.checked_sub(working_capital_floor)?
    } else {
        Money::ZERO
    };

    let mut cash_after_capex = free_cash_above_floor;

    match &directive.capital_action {
        JointCapitalAction::HoldAndDelever { repay_amount } => {
            let safe_repay = (*repay_amount)
                .min(free_cash_above_floor)
                .min(outstanding_debt);
            cash_after_capex = free_cash_above_floor.checked_sub(safe_repay)?;
            directive.capital_action = JointCapitalAction::HoldAndDelever {
                repay_amount: safe_repay,
            };
        }
        JointCapitalAction::ExpandCapacityCashFunded {
            capex_cost,
            ..
        } => {
            if *capex_cost <= free_cash_above_floor {
                cash_after_capex = free_cash_above_floor.checked_sub(*capex_cost)?;
            } else {
                // Downgrade infeasible cash CapEx to conservative debt paydown / hold
                let safe_repay = Money::from_cents(free_cash_above_floor.cents() / 4)
                    .min(outstanding_debt);
                cash_after_capex = free_cash_above_floor.checked_sub(safe_repay)?;
                directive.capital_action = JointCapitalAction::HoldAndDelever {
                    repay_amount: safe_repay,
                };
            }
        }
        JointCapitalAction::ExpandCapacityDebtFunded {
            capex_cost,
            loan_principal,
            ..
        } => {
            // Enforce maximum debt-to-cash & working-capital leverage covenant
            let max_leverage_ceiling = working_capital_floor.checked_mul_u64(8)?;
            let net_cash_needed = if *capex_cost > *loan_principal {
                capex_cost.checked_sub(*loan_principal)?
            } else {
                Money::ZERO
            };
            if outstanding_debt < max_leverage_ceiling && net_cash_needed <= free_cash_above_floor {
                cash_after_capex = free_cash_above_floor.checked_sub(net_cash_needed)?;
            } else {
                let safe_repay = Money::from_cents(free_cash_above_floor.cents() / 3)
                    .min(outstanding_debt);
                cash_after_capex = free_cash_above_floor.checked_sub(safe_repay)?;
                directive.capital_action = JointCapitalAction::HoldAndDelever {
                    repay_amount: safe_repay,
                };
            }
        }
        JointCapitalAction::BrandAndQualityCampaign { campaign_cost, .. } => {
            if *campaign_cost <= free_cash_above_floor {
                cash_after_capex = free_cash_above_floor.checked_sub(*campaign_cost)?;
            } else {
                directive.capital_action = JointCapitalAction::HoldAndDelever {
                    repay_amount: Money::ZERO,
                };
            }
        }
        JointCapitalAction::SignB2BContract { .. } => {}
    }

    if cash_after_capex.is_zero() {
        directive.dividend_stance = DividendStance::RetainAll;
        if directive.wage_stance == WageStance::Raise5Pct {
            directive.wage_stance = WageStance::HoldSteady;
        }
    }

    Ok(directive)
}

/// Applies a `PricingStance` to a current unit price, clamped to `>= $0.10 (10 cents)`.
pub fn apply_pricing_stance(current_price: Money, stance: PricingStance) -> Result<Money, CoreError> {
    let cents = current_price.cents().max(10);
    let updated_cents = match stance {
        PricingStance::AggressiveUndercut => (cents * 95) / 100,
        PricingStance::MatchMarket => cents,
        PricingStance::PremiumMargin => (cents * 105) / 100,
        PricingStance::ScarcitySkimming => (cents * 110) / 100,
    };
    Ok(Money::from_cents(updated_cents.max(10)))
}

/// Applies a `WageStance` to a daily wage bill, clamped to `>= $1.00 (100 cents)`.
pub fn apply_wage_stance(current_wage: Money, stance: WageStance) -> Result<Money, CoreError> {
    let cents = current_wage.cents().max(100);
    let updated_cents = match stance {
        WageStance::Cut5Pct => (cents * 95) / 100,
        WageStance::HoldSteady => cents,
        WageStance::Raise5Pct => (cents * 105) / 100,
    };
    Ok(Money::from_cents(updated_cents.max(100)))
}
