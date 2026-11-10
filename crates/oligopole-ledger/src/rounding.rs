use crate::error::LedgerError;
use crate::money::Money;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

/// Basis-point denominator (`10_000 bps = 100%`).
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Daily 30/360 basis-point denominator (`10_000 bps * 360 days = 3_600_000`).
pub const DAILY_360_BPS_DENOMINATOR: u128 = 3_600_000;

/// Allocates a non-negative `total: Money` across recipients according to `(Id, weight: u64)`
/// using the deterministic Largest Remainder (Hamilton) method.
///
/// Tie-breaking on identical fractional remainders is strictly ordered by `Id: Ord` ascending.
/// Duplicate `Id` entries in `weights` are preserved in their input slots (callers should pass
/// unique `Id`s or distinct tuple keys `(EntityId, SlotIdx)`).
///
/// Guarantees:
/// - Every allocated share is `>= Money::ZERO`.
/// - The sum of all allocated shares equals `total` with zero cent leakage.
/// - Intermediate math uses widening 256-bit arithmetic (`mul_div_rem_u64`), so no `i128` overflow
///   can occur for any valid `total >= Money::ZERO` and `u64` weights.
pub fn allocate_hamilton<Id: Ord + Copy>(
    total: Money,
    weights: &[(Id, u64)],
) -> Result<Vec<(Id, Money)>, LedgerError> {
    if total.is_negative() {
        return Err(LedgerError::NegativeAmountNotPermitted(total));
    }
    if weights.is_empty() {
        return Err(LedgerError::EmptyOrZeroWeights);
    }

    let mut weight_sum: u128 = 0;
    for &(_, w) in weights {
        weight_sum = weight_sum
            .checked_add(u128::from(w))
            .ok_or(LedgerError::ArithmeticOverflow)?;
    }
    if weight_sum == 0 {
        return Err(LedgerError::EmptyOrZeroWeights);
    }

    if total.is_zero() {
        return Ok(weights.iter().map(|&(id, _)| (id, Money::ZERO)).collect());
    }

    let mut allocations: Vec<(Id, Money, u128, usize)> = Vec::with_capacity(weights.len());
    let mut floor_sum = Money::ZERO;

    for (idx, &(id, weight)) in weights.iter().enumerate() {
        if weight == 0 {
            allocations.push((id, Money::ZERO, 0, idx));
            continue;
        }
        let (q, rem) = total.mul_div_rem_u64(weight, weight_sum)?;
        floor_sum = floor_sum.checked_add(q)?;
        allocations.push((id, q, rem, idx));
    }

    let remainder_cents = total.checked_sub(floor_sum)?.cents();
    if remainder_cents < 0 {
        return Err(LedgerError::ArithmeticOverflow);
    }
    let leftover_units = usize::try_from(remainder_cents)
        .map_err(|_| LedgerError::ArithmeticOverflow)?;

    // Sort by (Remainder DESC, Id ASC, OriginalIndex ASC) to award 1-cent remainders deterministically
    allocations.sort_by_key(|&(id, _, rem, idx)| (Reverse(rem), id, idx));

    for item in allocations.iter_mut().take(leftover_units) {
        item.1 = item.1.checked_add(Money::ONE_CENT)?;
    }

    // Restore original input order
    allocations.sort_by_key(|&(_, _, _, idx)| idx);

    Ok(allocations
        .into_iter()
        .map(|(id, share, _, _)| (id, share))
        .collect())
}

/// Allocates a discrete `total_units: u64` quantity across recipients according to `(Id, weight: u64)`
/// using the deterministic Largest Remainder (Hamilton) method.
pub fn allocate_units_hamilton<Id: Ord + Copy>(
    total_units: u64,
    weights: &[(Id, u64)],
) -> Result<Vec<(Id, u64)>, LedgerError> {
    let money_total = Money::from_cents(i128::from(total_units));
    let shares = allocate_hamilton(money_total, weights)?;
    let mut out = Vec::with_capacity(shares.len());
    for (id, share) in shares {
        let units = u64::try_from(share.cents()).map_err(|_| LedgerError::ArithmeticOverflow)?;
        out.push((id, units));
    }
    Ok(out)
}

/// Per-entity sub-cent numerator accumulator for single-party rate math (30/360 daily interest
/// or basis-point tax accruals) ensuring sub-cent remainders carry forward across ticks without loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RoundingReserve {
    /// Accumulated numerator units (`cents * bps_or_rate_numerator`) awaiting division by denominator.
    accrued_numerator: u128,
}

impl RoundingReserve {
    pub const fn new() -> Self {
        Self {
            accrued_numerator: 0,
        }
    }

    pub const fn accrued_numerator(self) -> u128 {
        self.accrued_numerator
    }

    /// Accrues one day of 30/360 annual interest on `principal >= Money::ZERO` at `annual_rate_bps: u32`.
    /// Returns the whole `Money` cents payable today and retains the sub-cent numerator remainder.
    pub fn accrue_daily_360_bps(
        &mut self,
        principal: Money,
        annual_rate_bps: u32,
    ) -> Result<Money, LedgerError> {
        if principal.is_negative() {
            return Err(LedgerError::NegativeAmountNotPermitted(principal));
        }
        if principal.is_zero() || annual_rate_bps == 0 {
            return Ok(Money::ZERO);
        }

        let (whole_from_principal, rem_from_principal) =
            principal.mul_div_rem_u64(u64::from(annual_rate_bps), DAILY_360_BPS_DENOMINATOR)?;

        let combined_rem = self
            .accrued_numerator
            .checked_add(rem_from_principal)
            .ok_or(LedgerError::ArithmeticOverflow)?;

        let extra_cents_u128 = combined_rem / DAILY_360_BPS_DENOMINATOR;
        self.accrued_numerator = combined_rem % DAILY_360_BPS_DENOMINATOR;

        let extra_money = Money::from_cents(
            i128::try_from(extra_cents_u128).map_err(|_| LedgerError::ArithmeticOverflow)?,
        );
        whole_from_principal.checked_add(extra_money)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_hamilton_exact_three_way_split() {
        let total = Money::from_cents(100);
        let weights = [(10u32, 1u64), (20u32, 1u64), (30u32, 1u64)];
        let result = allocate_hamilton(total, &weights).unwrap();
        // Tie-broken by ascending Id: id 10 gets 34 cents, 20 and 30 get 33 cents
        assert_eq!(
            result,
            vec![
                (10, Money::from_cents(34)),
                (20, Money::from_cents(33)),
                (30, Money::from_cents(33)),
            ]
        );
    }

    #[test]
    fn test_rounding_reserve_360_year_exact() {
        // $100.00 (10,000 cents) at 5.00% (500 bps) over 360 days should yield exactly $5.00 (500 cents)
        let principal = Money::from_dollars(100);
        let mut reserve = RoundingReserve::new();
        let mut total_interest = Money::ZERO;
        for _ in 0..360 {
            let daily = reserve.accrue_daily_360_bps(principal, 500).unwrap();
            total_interest = total_interest.checked_add(daily).unwrap();
        }
        assert_eq!(total_interest, Money::from_dollars(5));
        assert_eq!(reserve.accrued_numerator(), 0);
    }

    proptest! {
        #[test]
        fn prop_allocate_hamilton_conserves_exact_total(
            total_cents in 0i128..=i128::MAX,
            raw_weights in prop::collection::vec(0u64..=u64::MAX, 1..=32),
        ) {
            let has_positive = raw_weights.iter().any(|&w| w > 0);
            prop_assume!(has_positive);

            let weights: Vec<(u32, u64)> = raw_weights
                .into_iter()
                .enumerate()
                .map(|(i, w)| (u32::try_from(i).unwrap(), w))
                .collect();

            let total = Money::from_cents(total_cents);
            let allocated = allocate_hamilton(total, &weights).unwrap();
            prop_assert_eq!(allocated.len(), weights.len());

            let mut sum = Money::ZERO;
            for &(_, share) in &allocated {
                prop_assert!(!share.is_negative());
                sum = sum.checked_add(share).unwrap();
            }
            prop_assert_eq!(sum, total);
        }
    }
}
