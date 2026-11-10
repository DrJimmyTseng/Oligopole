use crate::candidate::generate_joint_action_candidates;
use crate::client::ClefFlashTcpClient;
use crate::error::AiError;
use crate::observation::ActorObservationView;
use crate::persona::{PersonaBiasVector, modulate_and_sample_choice, seeded_actor_rng};
use crate::protocol::{ClefQuestionType, ClefTypedQuestion};
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

/// Active AI backend status exposed in `oligopole-cli` and the `oligopole-ui` top status ribbon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiBackendStatus {
    /// Pure-Rust deterministic NPV & persona heuristic (`System 0`).
    System0Heuristic { reason: Option<String> },
    /// Live TCP connection to the local RTX 5090 `Clef-Flash` 9B sidecar worker.
    ClefFlashConnected {
        addr: String,
        device_name: String,
        dtype: String,
    },
}

impl AiBackendStatus {
    /// Short badge label for the Bloomberg Terminal top status bar.
    pub fn badge_label(&self) -> String {
        match self {
            Self::System0Heuristic { .. } => "AI: SYSTEM-0 HEURISTIC (CPU)".to_owned(),
            Self::ClefFlashConnected { device_name, .. } => {
                if device_name.contains("5090") {
                    "AI: CLEF-FLASH RTX 5090 (BF16)".to_owned()
                } else {
                    format!("AI: CLEF-FLASH ({device_name})")
                }
            }
        }
    }

    pub const fn is_clef_flash(&self) -> bool {
        matches!(self, Self::ClefFlashConnected { .. })
    }
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

        // 1. Capital Action Tuple priors
        let mut cap_priors = Vec::with_capacity(candidates.len());
        for cand in &candidates {
            let npv_dollars = (cand.estimated_npv.cents() / 100).max(0);
            let prior = u64::try_from(npv_dollars / 100 + 500)
                .unwrap_or(500)
                .clamp(100, 8_000);
            cap_priors.push(prior);
        }

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

        // 3. Wage Stance priors
        let wage_priors: [u64; 3] = [1_500, 6_500, 2_000];

        // 4. Dividend Stance priors (including 180-day excess-cash hoard sweep bias)
        let daily_payroll = obs.daily_wage_bill.checked_add(obs.daily_manager_bill)?;
        let (daily_interest, _) = obs
            .outstanding_debt
            .mul_div_rem_u64(u64::from(obs.bank_loan_rate_bps), 360 * 10_000)?;
        let daily_burn = daily_payroll.checked_add(daily_interest)?;
        let excess_cash_ceiling = daily_burn.checked_mul_u64(180)?;
        let working_floor = daily_burn.checked_mul_u64(20)?;

        let div_priors: [u64; 4] = if obs.available_cash > excess_cash_ceiling {
            [200, 800, 3_000, 6_000]
        } else if obs.available_cash <= working_floor {
            [8_500, 1_000, 400, 100]
        } else if obs.net_income.is_positive() {
            [1_500, 3_500, 3_500, 1_500]
        } else {
            [7_000, 2_000, 800, 200]
        };

        sample_directive_from_priors(
            world_seed,
            obs,
            persona,
            &candidates,
            &cap_priors,
            &pricing_priors,
            &wage_priors,
            &div_priors,
        )
    }
}

/// Adaptive Slow-Tick policy that connects to the local `Clef-Flash` 9B sidecar worker over
/// `TCP_NODELAY` `msgpack` when configured, and automatically falls back to `System0HeuristicPolicy`
/// if the sidecar is unreachable or disconnects mid-run.
#[derive(Debug)]
pub struct AdaptiveDecisionPolicy {
    client: Option<ClefFlashTcpClient>,
    status: AiBackendStatus,
    fallback: System0HeuristicPolicy,
}

impl Default for AdaptiveDecisionPolicy {
    fn default() -> Self {
        Self::new_system0()
    }
}

impl AdaptiveDecisionPolicy {
    pub const fn new_system0() -> Self {
        Self {
            client: None,
            status: AiBackendStatus::System0Heuristic { reason: None },
            fallback: System0HeuristicPolicy::new(),
        }
    }

    /// Attempts to connect and handshake with `clef-flash-worker` at `addr` (e.g. `127.0.0.1:19509`).
    /// If the TCP connection or handshake fails, gracefully falls back to `System0HeuristicPolicy`.
    pub fn connect_or_fallback(clef_addr: Option<&str>) -> Self {
        let Some(addr) = clef_addr else {
            return Self::new_system0();
        };
        match ClefFlashTcpClient::connect_and_handshake(addr) {
            Ok((client, verification)) => Self {
                client: Some(client),
                status: AiBackendStatus::ClefFlashConnected {
                    addr: addr.to_owned(),
                    device_name: verification.cuda_device_name,
                    dtype: verification.dtype,
                },
                fallback: System0HeuristicPolicy::new(),
            },
            Err(err) => Self {
                client: None,
                status: AiBackendStatus::System0Heuristic {
                    reason: Some(format!("Clef-Flash fallback ({addr}): {err}")),
                },
                fallback: System0HeuristicPolicy::new(),
            },
        }
    }

    pub const fn status(&self) -> &AiBackendStatus {
        &self.status
    }

    fn try_evaluate_via_clef(
        client: &mut ClefFlashTcpClient,
        world_seed: [u8; 32],
        obs: &ActorObservationView,
        persona: &PersonaBiasVector,
    ) -> Result<(StrategicDirective, [u8; 32]), AiError> {
        let candidates = generate_joint_action_candidates(obs)?;
        if candidates.is_empty() {
            return Err(AiError::EmptyCandidates);
        }

        let dossier = obs.to_clef_dossier();
        let cap_options: Vec<String> = candidates.iter().map(|c| c.label.clone()).collect();
        let questions = vec![
            ClefTypedQuestion {
                question_id: format!("{}_CAPEX", obs.ticker),
                context_dossier: dossier.clone(),
                prompt: "Select joint capital allocation & financing action".to_owned(),
                question_type: ClefQuestionType::Choice {
                    options: cap_options,
                },
            },
            ClefTypedQuestion {
                question_id: format!("{}_PRICE", obs.ticker),
                context_dossier: dossier.clone(),
                prompt: "Select monthly product pricing stance".to_owned(),
                question_type: ClefQuestionType::Choice {
                    options: vec![
                        "AGGRESSIVE_UNDERCUT".to_owned(),
                        "MATCH_MARKET".to_owned(),
                        "PREMIUM_MARGIN".to_owned(),
                        "SCARCITY_SKIMMING".to_owned(),
                    ],
                },
            },
            ClefTypedQuestion {
                question_id: format!("{}_WAGE", obs.ticker),
                context_dossier: dossier.clone(),
                prompt: "Select monthly labor wage stance".to_owned(),
                question_type: ClefQuestionType::Choice {
                    options: vec![
                        "CUT_5PCT".to_owned(),
                        "HOLD_STEADY".to_owned(),
                        "RAISE_5PCT".to_owned(),
                    ],
                },
            },
            ClefTypedQuestion {
                question_id: format!("{}_DIV", obs.ticker),
                context_dossier: dossier,
                prompt: "Select monthly shareholder dividend stance".to_owned(),
                question_type: ClefQuestionType::Choice {
                    options: vec![
                        "RETAIN_ALL".to_owned(),
                        "LOW_20PCT".to_owned(),
                        "MODERATE_40PCT".to_owned(),
                        "HIGH_70PCT".to_owned(),
                    ],
                },
            },
        ];

        let dists = client.evaluate_batch(obs.tick.0, questions)?;
        let [d_cap, d_price, d_wage, d_div] = dists.as_slice() else {
            return Err(AiError::ProtocolError(
                "expected 4 distributions from Clef-Flash Slow-Tick batch".to_owned(),
            ));
        };

        if d_cap.probs_bps.len() != candidates.len()
            || d_price.probs_bps.len() != 4
            || d_wage.probs_bps.len() != 3
            || d_div.probs_bps.len() != 4
        {
            return Err(AiError::ProtocolError(
                "mismatched option count in Clef-Flash distribution response".to_owned(),
            ));
        }

        sample_directive_from_priors(
            world_seed,
            obs,
            persona,
            &candidates,
            &d_cap.probs_bps,
            &d_price.probs_bps,
            &d_wage.probs_bps,
            &d_div.probs_bps,
        )
    }
}

impl DecisionPolicy for AdaptiveDecisionPolicy {
    fn evaluate_actor_slow_tick(
        &mut self,
        world_seed: [u8; 32],
        obs: &ActorObservationView,
        persona: &PersonaBiasVector,
    ) -> Result<(StrategicDirective, [u8; 32]), AiError> {
        if let Some(client) = &mut self.client {
            match Self::try_evaluate_via_clef(client, world_seed, obs, persona) {
                Ok(res) => return Ok(res),
                Err(err) => {
                    self.client = None;
                    self.status = AiBackendStatus::System0Heuristic {
                        reason: Some(format!("Clef-Flash disconnected, fell back to System0: {err}")),
                    };
                }
            }
        }
        self.fallback
            .evaluate_actor_slow_tick(world_seed, obs, persona)
    }
}

#[allow(clippy::too_many_arguments)]
fn sample_directive_from_priors(
    world_seed: [u8; 32],
    obs: &ActorObservationView,
    persona: &PersonaBiasVector,
    candidates: &[crate::candidate::CandidateOption],
    cap_priors: &[u64],
    pricing_priors: &[u64],
    wage_priors: &[u64],
    div_priors: &[u64],
) -> Result<(StrategicDirective, [u8; 32]), AiError> {
    let mut rng = seeded_actor_rng(world_seed, obs.tick.0, &obs.ticker);

    let mut cap_affinities = Vec::with_capacity(candidates.len());
    for cand in candidates {
        let aff = (persona.risk_appetite * cand.risk_weight)
            + (persona.leverage_tolerance * cand.leverage_weight);
        cap_affinities.push(aff);
    }
    let (cap_idx, cap_bps) = modulate_and_sample_choice(cap_priors, &cap_affinities, &mut rng)?;
    let selected_cap_action = candidates
        .get(cap_idx)
        .ok_or(AiError::EmptyCandidates)?
        .action
        .clone();

    let pricing_affinities: [Decimal; 4] = [
        persona.price_aggression * dec!(0.6),
        Decimal::ZERO,
        persona.price_aggression * dec!(-0.3),
        persona.price_aggression * dec!(-0.6),
    ];
    let (pricing_idx, pricing_bps) =
        modulate_and_sample_choice(pricing_priors, &pricing_affinities, &mut rng)?;
    let pricing_stance = match pricing_idx {
        0 => PricingStance::AggressiveUndercut,
        1 => PricingStance::MatchMarket,
        2 => PricingStance::PremiumMargin,
        _ => PricingStance::ScarcitySkimming,
    };

    let wage_affinities: [Decimal; 3] = [
        persona.risk_appetite * dec!(-0.2),
        Decimal::ZERO,
        persona.risk_appetite * dec!(0.2),
    ];
    let (wage_idx, wage_bps) = modulate_and_sample_choice(wage_priors, &wage_affinities, &mut rng)?;
    let wage_stance = match wage_idx {
        0 => WageStance::Cut5Pct,
        1 => WageStance::HoldSteady,
        _ => WageStance::Raise5Pct,
    };

    let div_affinities: [Decimal; 4] = [
        persona.shareholder_yield_bias * dec!(-0.6),
        persona.shareholder_yield_bias * dec!(-0.1),
        persona.shareholder_yield_bias * dec!(0.4),
        persona.shareholder_yield_bias * dec!(0.8),
    ];
    let (div_idx, div_bps) = modulate_and_sample_choice(div_priors, &div_affinities, &mut rng)?;
    let dividend_stance = match div_idx {
        0 => DividendStance::RetainAll,
        1 => DividendStance::Low20Pct,
        2 => DividendStance::Moderate40Pct,
        _ => DividendStance::High70Pct,
    };

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::protocol::{ClefQuestionDistribution, ClefWorkerRequest, ClefWorkerResponse};
    use oligopole_core::{ExecutivePersonaKind, build_vertical_slice_0};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn test_adaptive_policy_falls_back_gracefully_when_clef_unreachable() {
        let mut policy = AdaptiveDecisionPolicy::connect_or_fallback(Some("127.0.0.1:1"));
        assert!(!policy.status().is_clef_flash());
        assert_eq!(
            policy.status().badge_label(),
            "AI: SYSTEM-0 HEURISTIC (CPU)"
        );

        let (world, handles) = build_vertical_slice_0([9u8; 32]).unwrap();
        let obs = ActorObservationView::build(&world, handles.iron_corp).unwrap();
        let persona = PersonaBiasVector::from_archetype(ExecutivePersonaKind::DividendAristocrat);
        let (dir, hash) = policy
            .evaluate_actor_slow_tick(world.world_seed, &obs, &persona)
            .unwrap();
        assert_eq!(dir.corp_id, handles.iron_corp);
        assert_ne!(hash, [0u8; 32]);
    }

    #[test]
    fn test_adaptive_policy_live_clef_flash_and_distinct_personas() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            for _ in 0..2 {
                let mut len_buf = [0u8; 4];
                conn.read_exact(&mut len_buf).unwrap();
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut buf = vec![0u8; len];
                conn.read_exact(&mut buf).unwrap();
                let req: ClefWorkerRequest = rmp_serde::from_slice(&buf).unwrap();

                let resp = match req {
                    ClefWorkerRequest::PingHandshake { .. } => ClefWorkerResponse::PongHandshake {
                        model_id: "cloudflare/clef-flash-9b".to_owned(),
                        dtype: "bfloat16".to_owned(),
                        cuda_device_name: "NVIDIA GeForce RTX 5090".to_owned(),
                        vram_resident: true,
                    },
                    ClefWorkerRequest::EvaluateSlowTickBatch { tick, questions } => {
                        let distributions = questions
                            .into_iter()
                            .map(|q| {
                                let count = match q.question_type {
                                    ClefQuestionType::Choice { options }
                                    | ClefQuestionType::Noul { options } => options.len(),
                                    ClefQuestionType::Score { .. } => 10,
                                };
                                let each = 10_000 / u64::try_from(count).unwrap();
                                let mut probs = vec![each; count];
                                let rem = 10_000 - each * u64::try_from(count).unwrap();
                                if let Some(first) = probs.first_mut() {
                                    *first += rem;
                                }
                                ClefQuestionDistribution {
                                    question_id: q.question_id,
                                    probs_bps: probs,
                                }
                            })
                            .collect();
                        ClefWorkerResponse::BatchDistributions {
                            tick,
                            inference_micros: 12_000,
                            distributions,
                        }
                    }
                };

                let out = rmp_serde::to_vec_named(&resp).unwrap();
                let out_len = u32::try_from(out.len()).unwrap();
                conn.write_all(&out_len.to_be_bytes()).unwrap();
                conn.write_all(&out).unwrap();
                conn.flush().unwrap();
            }
        });

        let mut policy = AdaptiveDecisionPolicy::connect_or_fallback(Some(&addr));
        assert!(policy.status().is_clef_flash());
        assert_eq!(
            policy.status().badge_label(),
            "AI: CLEF-FLASH RTX 5090 (BF16)"
        );

        let (world, handles) = build_vertical_slice_0([21u8; 32]).unwrap();
        let obs = ActorObservationView::build(&world, handles.vale_corp).unwrap();
        let persona =
            PersonaBiasVector::from_archetype(ExecutivePersonaKind::AggressiveEmpireBuilder);
        let (directive, _) = policy
            .evaluate_actor_slow_tick(world.world_seed, &obs, &persona)
            .unwrap();
        assert_eq!(directive.corp_id, handles.vale_corp);
        server.join().unwrap();
    }
}
