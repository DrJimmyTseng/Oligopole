use crate::error::AiError;
use oligopole_core::{ExecutivePersonaKind, decimal_bytes_serde, pade_exp_neg};
use oligopole_ledger::{Money, allocate_hamilton};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

/// Executive persona bias parameters (`[-1.0, +1.0]`) modulating `Clef-Flash` probability distributions
/// in log-odds space before deterministic `ChaCha8Rng` sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaBiasVector {
    #[serde(with = "decimal_bytes_serde")]
    pub risk_appetite: Decimal,
    #[serde(with = "decimal_bytes_serde")]
    pub leverage_tolerance: Decimal,
    #[serde(with = "decimal_bytes_serde")]
    pub price_aggression: Decimal,
    #[serde(with = "decimal_bytes_serde")]
    pub shareholder_yield_bias: Decimal,
}

impl Default for PersonaBiasVector {
    fn default() -> Self {
        Self::balanced_executive()
    }
}

impl PersonaBiasVector {
    pub const fn from_archetype(kind: ExecutivePersonaKind) -> Self {
        match kind {
            ExecutivePersonaKind::BalancedExecutive => Self::balanced_executive(),
            ExecutivePersonaKind::DividendAristocrat => Self::conservative_dividend_steward(),
            ExecutivePersonaKind::AggressiveEmpireBuilder => Self::aggressive_growth_ceo(),
            ExecutivePersonaKind::QualityMoatCompounder => Self::quality_moat_compounder(),
            ExecutivePersonaKind::CostUndercutChallenger => Self::cost_undercut_challenger(),
            ExecutivePersonaKind::OpportunisticCapitalAllocator => {
                Self::opportunistic_capital_allocator()
            }
        }
    }

    pub const fn balanced_executive() -> Self {
        Self {
            risk_appetite: dec!(0.1),
            leverage_tolerance: dec!(0.0),
            price_aggression: dec!(0.0),
            shareholder_yield_bias: dec!(0.2),
        }
    }

    pub const fn aggressive_growth_ceo() -> Self {
        Self {
            risk_appetite: dec!(0.7),
            leverage_tolerance: dec!(0.6),
            price_aggression: dec!(0.5),
            shareholder_yield_bias: dec!(-0.3),
        }
    }

    pub const fn conservative_dividend_steward() -> Self {
        Self {
            risk_appetite: dec!(-0.5),
            leverage_tolerance: dec!(-0.7),
            price_aggression: dec!(-0.3),
            shareholder_yield_bias: dec!(0.8),
        }
    }

    pub const fn quality_moat_compounder() -> Self {
        Self {
            risk_appetite: dec!(0.3),
            leverage_tolerance: dec!(-0.2),
            price_aggression: dec!(-0.6),
            shareholder_yield_bias: dec!(0.3),
        }
    }

    pub const fn cost_undercut_challenger() -> Self {
        Self {
            risk_appetite: dec!(0.4),
            leverage_tolerance: dec!(0.3),
            price_aggression: dec!(0.8),
            shareholder_yield_bias: dec!(-0.2),
        }
    }

    pub const fn opportunistic_capital_allocator() -> Self {
        Self {
            risk_appetite: dec!(-0.2),
            leverage_tolerance: dec!(-0.5),
            price_aggression: dec!(0.2),
            shareholder_yield_bias: dec!(0.6),
        }
    }
}

/// Modulates prior option weights `prior_weights_bps` (`u64`) by `persona_affinities` (`Decimal`)
/// using our monotonic scaling-and-squaring `pade_exp_neg` kernel (`w_i = prior_i * exp(affinity_i - max_affinity)`),
/// normalizes via `allocate_hamilton` to exact basis points summing to `10_000`, and samples
/// an index deterministically using `rng`.
///
/// Returns `(selected_index, normalized_bps_weights)`.
pub fn modulate_and_sample_choice(
    prior_weights_bps: &[u64],
    persona_affinities: &[Decimal],
    rng: &mut ChaCha8Rng,
) -> Result<(usize, Vec<u64>), AiError> {
    if prior_weights_bps.is_empty() || prior_weights_bps.len() != persona_affinities.len() {
        return Err(AiError::EmptyCandidates);
    }

    let mut max_aff = dec!(-1_000_000);
    for &aff in persona_affinities {
        if aff > max_aff {
            max_aff = aff;
        }
    }

    let mut combined_weights: Vec<(usize, u64)> = Vec::with_capacity(prior_weights_bps.len());
    for (idx, (&prior, &aff)) in prior_weights_bps
        .iter()
        .zip(persona_affinities.iter())
        .enumerate()
    {
        let shifted = aff.checked_sub(max_aff).unwrap_or(Decimal::ZERO);
        let mult = pade_exp_neg(shifted)?;
        let prior_dec = Decimal::from(prior.max(1));
        let prod = (prior_dec * mult * dec!(1_000)).round();
        let w = u64::try_from(prod.mantissa() / 10i128.pow(prod.scale()))
            .unwrap_or(1)
            .max(1);
        combined_weights.push((idx, w));
    }

    let normalized = allocate_hamilton(Money::from_cents(10_000), &combined_weights)?;
    let mut bps_out = Vec::with_capacity(normalized.len());
    for (_, share) in &normalized {
        let b = u64::try_from(share.cents()).unwrap_or(0);
        bps_out.push(b);
    }

    let roll: u64 = rng.gen_range(0..10_000);
    let mut cumulative = 0u64;
    let mut chosen = 0usize;
    for (idx, &weight) in bps_out.iter().enumerate() {
        cumulative = cumulative.saturating_add(weight);
        if roll < cumulative {
            chosen = idx;
            break;
        }
    }

    Ok((chosen, bps_out))
}

/// Derives a deterministic `ChaCha8Rng` for `(world_seed, tick, actor_discriminator)`.
pub fn seeded_actor_rng(world_seed: [u8; 32], tick: u64, actor_tag: &str) -> ChaCha8Rng {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&world_seed);
    hasher.update(&tick.to_be_bytes());
    hasher.update(actor_tag.as_bytes());
    let digest = hasher.finalize();
    ChaCha8Rng::from_seed(*digest.as_bytes())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_persona_modulate_and_sample_exact_10000_bps_and_deterministic(
            priors in prop::collection::vec(1u64..=10_000u64, 2..=8),
            seed_byte in any::<u8>(),
            tick in 1u64..=10_000u64,
        ) {
            let affinities: Vec<Decimal> = (0..priors.len())
                .map(|i| Decimal::new(i64::try_from(i).unwrap() * 250 - 500, 3))
                .collect();

            let mut rng1 = seeded_actor_rng([seed_byte; 32], tick, "ACME");
            let mut rng2 = seeded_actor_rng([seed_byte; 32], tick, "ACME");

            let (idx1, bps1) = modulate_and_sample_choice(&priors, &affinities, &mut rng1).unwrap();
            let (idx2, bps2) = modulate_and_sample_choice(&priors, &affinities, &mut rng2).unwrap();

            prop_assert_eq!(idx1, idx2);
            prop_assert_eq!(&bps1, &bps2);
            let sum: u64 = bps1.iter().sum();
            prop_assert_eq!(sum, 10_000);
            prop_assert!(idx1 < priors.len());
        }
    }
}
