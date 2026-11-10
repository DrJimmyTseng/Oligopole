use crate::candidate::generate_joint_action_candidates;
use crate::error::AiError;
use crate::observation::ActorObservationView;
use crate::persona::{PersonaBiasVector, modulate_and_sample_choice, seeded_actor_rng};
use oligopole_core::{DividendStance, PricingStance, StrategicDirective, WageStance};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Pluggable Slow-Tick decision policy trait (`ClefFlashSidecarPolicy` vs. `System0HeuristicPolicy`).
pub trait DecisionPolicy {
    /// Evaluates an actor's `ActorObservationView` and `PersonaBiasVector`, returning a sampled
    /// `StrategicDirective` and a 32-byte `blake3` hash of the calibrated probability distributions
    /// for deterministic replay verification in `oligopole-store`.
    fn evaluate_actor_slow_tick(
        &mut self,
        world_seed: [u8; 32],
        obs: &ActorObservationView,
        persona: &PersonaBiasVector,
    ) -> Result<(StrategicDirective, [u8; 32]), AiError>;
}

/// Pure-Rust deterministic NPV & inventory-coverage heuristic policy (`System 0`)
/// used for 10,000-tick headless Monte Carlo runs, `proptest` suites, and offline fallback.
#[derive(Debug, Clone, Default)]
pub struct System0HeuristicPolicy;

impl System0HeuristicPolicy {
    pub const fn new() -> Self {
        Self
    }
}

impl DecisionPolicy for System0HeuristicPolicy {
    fn evaluate_actor_slow_tick(
        &mut self,
        world_seed: [u8; 32],
        obs: &ActorObservationView,
        persona: &PersonaBiasVector,
    ) -> Result<(StrategicDirective, [u8; 32]), AiError> {
        let candidates = generate_joint_action_candidates(obs)?;
        if candidates.is_empty() {
            return Err(AiError::EmptyCandidates);
        }

        let mut rng = seeded_actor_rng(world_seed, obs.tick.0, &obs.ticker);

        // 1. Capital Action Tuple priors & persona affinities
        let mut cap_priors = Vec::with_capacity(candidates.len());
        let mut cap_affinities = Vec::with_capacity(candidates.len());
        for cand in &candidates {
            let npv_dollars = (cand.estimated_npv.cents() / 100).max(0);
            let prior = u64::try_from(npv_dollars / 100 + 500).unwrap_or(500).clamp(100, 8_000);
            cap_priors.push(prior);
            let aff = (persona.risk_appetite * cand.risk_weight)
                + (persona.leverage_tolerance * cand.leverage_weight);
            cap_affinities.push(aff);
        }
        let (cap_idx, cap_bps) =
            modulate_and_sample_choice(&cap_priors, &cap_affinities, &mut rng)?;
        let selected_cap_action = candidates
            .get(cap_idx)
            .ok_or(AiError::EmptyCandidates)?
            .action
            .clone();

        // 2. Pricing Stance priors based on inventory coverage & peer price comparison
        let inv_high = obs.primary_inventory_units > obs.daily_capacity.saturating_mul(4);
        let inv_low = obs.primary_inventory_units < obs.daily_capacity;
        let pricing_priors: [u64; 4] = if inv_high {
            [4_500, 3_500, 1_500, 500]
        } else if inv_low {
            [500, 3_000, 4_000, 2_500]
        } else {
            [2_000, 4_500, 2_500, 1_000]
        };
        let pricing_affinities: [Decimal; 4] = [
            persona.price_aggression * dec!(0.6),
            Decimal::ZERO,
            persona.price_aggression * dec!(-0.3),
            persona.price_aggression * dec!(-0.6),
        ];
        let (pricing_idx, pricing_bps) =
            modulate_and_sample_choice(&pricing_priors, &pricing_affinities, &mut rng)?;
        let pricing_stance = match pricing_idx {
            0 => PricingStance::AggressiveUndercut,
            1 => PricingStance::MatchMarket,
            2 => PricingStance::PremiumMargin,
            _ => PricingStance::ScarcitySkimming,
        };

        // 3. Wage Stance priors
        let wage_priors: [u64; 3] = [1_500, 6_500, 2_000];
        let wage_affinities: [Decimal; 3] = [
            persona.risk_appetite * dec!(-0.2),
            Decimal::ZERO,
            persona.risk_appetite * dec!(0.2),
        ];
        let (wage_idx, wage_bps) =
            modulate_and_sample_choice(&wage_priors, &wage_affinities, &mut rng)?;
        let wage_stance = match wage_idx {
            0 => WageStance::Cut5Pct,
            1 => WageStance::HoldSteady,
            _ => WageStance::Raise5Pct,
        };

        // 4. Dividend Stance priors
        let div_priors: [u64; 4] = if obs.net_income.is_positive() {
            [1_500, 3_500, 3_500, 1_500]
        } else {
            [7_000, 2_000, 800, 200]
        };
        let div_affinities: [Decimal; 4] = [
            persona.shareholder_yield_bias * dec!(-0.6),
            persona.shareholder_yield_bias * dec!(-0.1),
            persona.shareholder_yield_bias * dec!(0.4),
            persona.shareholder_yield_bias * dec!(0.8),
        ];
        let (div_idx, div_bps) =
            modulate_and_sample_choice(&div_priors, &div_affinities, &mut rng)?;
        let dividend_stance = match div_idx {
            0 => DividendStance::RetainAll,
            1 => DividendStance::Low20Pct,
            2 => DividendStance::Moderate40Pct,
            _ => DividendStance::High70Pct,
        };

        // Hash all calibrated probability distributions via blake3 for replay verification
        let mut hasher = blake3::Hasher::new();
        for &b in cap_bps
            .iter()
            .chain(pricing_bps.iter())
            .chain(wage_bps.iter())
            .chain(div_bps.iter())
        {
            hasher.update(&b.to_be_bytes());
        }
        let probs_hash = *hasher.finalize().as_bytes();

        Ok((
            StrategicDirective {
                corp_id: obs.corp_id,
                capital_action: selected_cap_action,
                pricing_stance,
                wage_stance,
                dividend_stance,
                confidence_score: 7,
            },
            probs_hash,
        ))
    }
}
