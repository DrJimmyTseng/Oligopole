use crate::error::CoreError;
use crate::ids::{CohortId, CorpId};
use oligopole_ledger::{BPS_DENOMINATOR, EntityId, Money, allocate_hamilton};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

/// Monotonic scaling-and-squaring Padé `(2,2)` approximation for $e^x$ on $x \le 0$ in pure `Decimal`.
///
/// Algorithm:
/// 1. Clamps $x$ to $[-20, 0]$ so extreme negative utilities cannot underflow or invert the rational curve.
/// 2. Scales by $2^{-5} = 1/32$: $s = x / 32 \in [-0.625, 0]$, keeping $s$ strictly inside the high-accuracy
///    monotonic domain of Padé `(2,2)` (avoiding the numerator parabola minimum at $s = -3$).
/// 3. Evaluates $R_{2,2}(s) = \frac{12 + 6s + s^2}{12 - 6s + s^2}$.
/// 4. Squares $R_{2,2}(s)$ 5 times ($y \leftarrow y^2$) to recover $e^x \in (0, 1]$.
pub fn pade_exp_neg(x: Decimal) -> Result<Decimal, CoreError> {
    let clamped = x.clamp(dec!(-20), Decimal::ZERO);
    if clamped.is_zero() {
        return Ok(Decimal::ONE);
    }

    let s = clamped / dec!(32);
    let s_sq = s.checked_mul(s).ok_or(CoreError::DecimalOverflow)?;
    let six_s = dec!(6).checked_mul(s).ok_or(CoreError::DecimalOverflow)?;

    let numerator = dec!(12)
        .checked_add(six_s)
        .and_then(|v| v.checked_add(s_sq))
        .ok_or(CoreError::DecimalOverflow)?;
    let denominator = dec!(12)
        .checked_sub(six_s)
        .and_then(|v| v.checked_add(s_sq))
        .ok_or(CoreError::DecimalOverflow)?;

    if denominator <= Decimal::ZERO {
        return Err(CoreError::DecimalOverflow);
    }

    let mut y = numerator
        .checked_div(denominator)
        .ok_or(CoreError::DecimalOverflow)?;

    for _ in 0..5 {
        y = y.checked_mul(y).ok_or(CoreError::DecimalOverflow)?;
        // Round to 12 decimal places at each squaring step so the 96-bit Decimal mantissa never overflows
        y = y.round_dp(12);
    }

    Ok(y.clamp(dec!(0.000_000_001), Decimal::ONE))
}

/// Deterministic 16-byte binary `serde` adapter for `rust_decimal::Decimal` compatible with `postcard`.
pub mod decimal_bytes_serde {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(val: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
        val.serialize().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
        let bytes = <[u8; 16]>::deserialize(deserializer)?;
        Ok(Decimal::deserialize(bytes))
    }
}

/// Stratified macroeconomic household income cohorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HouseholdCohortKind {
    WageEarners,
    ManagersSkilled,
    CapitalistsExecutives,
}

/// Retail shelf offer evaluated by household cohorts via Multinomial Logit (MNL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetailStoreOffer {
    pub seller_corp_id: CorpId,
    pub unit_price: Money,
    pub quality_score_bps: u32,
    pub brand_score_bps: u32,
    pub available_units: u64,
}

/// Household cohort aggregate storing Stone-Geary subsistence parameters and MNL utility elasticities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HouseholdCohort {
    pub cohort_id: CohortId,
    pub entity_id: EntityId,
    pub kind: HouseholdCohortKind,
    pub name: String,
    pub population_count: u64,
    /// Minimum cash savings buffer (`Money`) preserved before discretionary retail spending (Stone-Geary floor).
    pub savings_subsistence_floor: Money,
    /// Daily fraction of excess disposable cash spent on retail consumption (`bps` out of `10_000`).
    pub daily_mpc_bps: u32,
    #[serde(with = "decimal_bytes_serde")]
    pub price_sensitivity: Decimal,
    #[serde(with = "decimal_bytes_serde")]
    pub quality_affinity: Decimal,
    #[serde(with = "decimal_bytes_serde")]
    pub brand_affinity: Decimal,
}

impl HouseholdCohort {
    /// Computes the cohort's today retail expenditure budget from `available_cash` using its
    /// Stone-Geary savings floor and daily Marginal Propensity to Consume (`daily_mpc_bps`).
    pub fn compute_daily_retail_budget(&self, available_cash: Money) -> Result<Money, CoreError> {
        if available_cash <= self.savings_subsistence_floor {
            return Ok(Money::ZERO);
        }
        let discretionary = available_cash.checked_sub(self.savings_subsistence_floor)?;
        let (budget, _) =
            discretionary.mul_div_rem_u64(u64::from(self.daily_mpc_bps), BPS_DENOMINATOR)?;
        Ok(budget)
    }

    /// Evaluates Multinomial Logit (MNL) utility across active `offers` (with `available_units > 0`)
    /// using `pade_exp_neg` in `rust_decimal` and splits `total_budget` via Hamilton Largest Remainder.
    pub fn allocate_retail_expenditure(
        &self,
        total_budget: Money,
        offers: &[RetailStoreOffer],
        reference_price: Money,
    ) -> Result<Vec<(CorpId, Money)>, CoreError> {
        let active: Vec<&RetailStoreOffer> = offers
            .iter()
            .filter(|o| o.available_units > 0 && o.unit_price.is_positive())
            .collect();

        if active.is_empty() || !total_budget.is_positive() {
            return Ok(Vec::new());
        }

        let ref_cents = reference_price.cents().max(100);
        let ref_dec = Decimal::from_i128_with_scale(ref_cents, 0);

        let mut utilities: Vec<(CorpId, Decimal)> = Vec::with_capacity(active.len());
        let mut max_u = dec!(-1_000_000);

        for offer in &active {
            let price_dec = Decimal::from_i128_with_scale(offer.unit_price.cents(), 0);
            let rel_price = price_dec
                .checked_div(ref_dec)
                .ok_or(CoreError::DecimalOverflow)?;
            let qual_norm = Decimal::from(offer.quality_score_bps) / dec!(10_000);
            let brand_norm = Decimal::from(offer.brand_score_bps) / dec!(10_000);

            let u_qual = self
                .quality_affinity
                .checked_mul(qual_norm)
                .ok_or(CoreError::DecimalOverflow)?;
            let u_brand = self
                .brand_affinity
                .checked_mul(brand_norm)
                .ok_or(CoreError::DecimalOverflow)?;
            let u_price = self
                .price_sensitivity
                .checked_mul(rel_price)
                .ok_or(CoreError::DecimalOverflow)?;

            let u = u_qual
                .checked_add(u_brand)
                .and_then(|v| v.checked_sub(u_price))
                .ok_or(CoreError::DecimalOverflow)?;

            if u > max_u {
                max_u = u;
            }
            utilities.push((offer.seller_corp_id, u));
        }

        let mut weights: Vec<(CorpId, u64)> = Vec::with_capacity(utilities.len());
        for (corp_id, u) in utilities {
            let shifted = u.checked_sub(max_u).ok_or(CoreError::DecimalOverflow)?;
            let exp_val = pade_exp_neg(shifted)?;
            // Scale (0, 1] weight to integer weight in [1, 1_000_000_000]
            let scaled = (exp_val * dec!(1_000_000_000)).round();
            let weight_u64 = u64::try_from(scaled.mantissa() / 10i128.pow(scaled.scale()))
                .unwrap_or(1)
                .max(1);
            weights.push((corp_id, weight_u64));
        }

        Ok(allocate_hamilton(total_budget, &weights)?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_pade_exp_neg_monotonic_across_minus_3_trap() {
        // Without scaling-and-squaring, Padé (2,2) rebounds upward for x < -3!
        // Verify strict monotonicity across [0, -1, -2, -3, -4, -6, -10, -20].
        let points = [
            Decimal::ZERO,
            dec!(-0.5),
            dec!(-1.0),
            dec!(-2.0),
            dec!(-3.0),
            dec!(-4.0),
            dec!(-6.0),
            dec!(-10.0),
            dec!(-15.0),
            dec!(-20.0),
        ];
        for pair in points.windows(2) {
            let [hi_x, lo_x] = pair else { continue };
            let v_hi = pade_exp_neg(*hi_x).unwrap();
            let v_lo = pade_exp_neg(*lo_x).unwrap();
            assert!(
                v_hi > v_lo,
                "expected pade_exp_neg({hi_x}) = {v_hi} > pade_exp_neg({lo_x}) = {v_lo}"
            );
            assert!(v_lo > Decimal::ZERO);
        }
    }

    proptest! {
        #[test]
        fn prop_pade_exp_neg_strictly_monotonic(
            a_milli in -20_000i64..=0i64,
            b_milli in -20_000i64..=0i64,
        ) {
            let xa = Decimal::new(a_milli, 3);
            let xb = Decimal::new(b_milli, 3);
            let ya = pade_exp_neg(xa).unwrap();
            let yb = pade_exp_neg(xb).unwrap();
            prop_assert!(ya > Decimal::ZERO && ya <= Decimal::ONE);
            prop_assert!(yb > Decimal::ZERO && yb <= Decimal::ONE);
            match xa.cmp(&xb) {
                std::cmp::Ordering::Less => prop_assert!(ya <= yb),
                std::cmp::Ordering::Greater => prop_assert!(ya >= yb),
                std::cmp::Ordering::Equal => prop_assert_eq!(ya, yb),
            }
        }
    }
}
