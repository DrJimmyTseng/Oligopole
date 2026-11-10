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
/// 1. Reserves a 20-day working-capital payroll + interest floor (`20 * (daily_wage_bill + daily_manager_bill + daily_loan_interest)`).
/// 2. If `ExpandCapacityCashFunded` or `BrandAndQualityCampaign` exceeds available cash above the floor,
///    downgrades to `HoldAndDelever { repay_amount: excess_cash / 4 }`.
/// 3. Debt-funded capacity expansion (`ExpandCapacityDebtFunded`) is permitted only when the corporation has
///    at least its 20-day working-capital floor in unreserved cash and total debt remains below `3 * working_capital_floor`.
/// 4. If cash after capital actions is below the working-capital floor, clips `DividendStance` to `RetainAll`
///    and prevents `WageStance::Raise5Pct`.
pub fn reconcile_strategic_directive(
    corp: &Corporation,
    available_cash: Money,
    outstanding_debt: Money,
    loan_rate_bps: u32,
    mut directive: StrategicDirective,
) -> Result<StrategicDirective, CoreError> {
    let working_capital_floor =
        corp.working_capital_floor(20, outstanding_debt, loan_rate_bps)?;
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
                let safe_repay = Money::from_cents(free_cash_above_floor.cents() / 3)
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
            // Enforce solvency & interest-coverage covenant:
            // Must hold at least the 20-day working-capital floor in cash, and post-loan debt <= 3x working-capital floor
            let max_leverage_ceiling = working_capital_floor.checked_mul_u64(3)?;
            let projected_debt = outstanding_debt.checked_add(*loan_principal)?;
            let net_cash_needed = if *capex_cost > *loan_principal {
                capex_cost.checked_sub(*loan_principal)?
            } else {
                Money::ZERO
            };
            if available_cash >= working_capital_floor
                && projected_debt <= max_leverage_ceiling
                && net_cash_needed <= free_cash_above_floor
            {
                cash_after_capex = free_cash_above_floor.checked_sub(net_cash_needed)?;
            } else {
                let safe_repay = Money::from_cents(free_cash_above_floor.cents() / 2)
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
        JointCapitalAction::RepurchaseShares {
            buyback_budget,
            max_limit_price,
        } => {
            let safe_budget = (*buyback_budget).min(free_cash_above_floor);
            if safe_budget.is_positive() && max_limit_price.is_positive() {
                cash_after_capex = free_cash_above_floor.checked_sub(safe_budget)?;
                directive.capital_action = JointCapitalAction::RepurchaseShares {
                    buyback_budget: safe_budget,
                    max_limit_price: *max_limit_price,
                };
            } else {
                directive.capital_action = JointCapitalAction::HoldAndDelever {
                    repay_amount: Money::ZERO,
                };
            }
        }
        JointCapitalAction::IssueEquityShares {
            shares_to_issue,
            min_ask_price,
        } => {
            if *shares_to_issue == 0 || !min_ask_price.is_positive() {
                directive.capital_action = JointCapitalAction::HoldAndDelever {
                    repay_amount: Money::ZERO,
                };
            }
        }
        JointCapitalAction::MaintenanceAndRdOverhaul { overhaul_cost, .. } => {
            if *overhaul_cost <= free_cash_above_floor && overhaul_cost.is_positive() {
                cash_after_capex = free_cash_above_floor.checked_sub(*overhaul_cost)?;
            } else {
                directive.capital_action = JointCapitalAction::HoldAndDelever {
                    repay_amount: Money::ZERO,
                };
            }
        }
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
