//! `oligopole-cli`: Composition root for Oligopole (`sim-bench`, `verify-replay`, `terminal`).

use oligopole_ai::{
    ActorObservationView, AdaptiveDecisionPolicy, DEFAULT_CLEF_WORKER_ADDR, DecisionPolicy,
    PersonaBiasVector,
};
use oligopole_core::{PlayerCommand, build_vertical_slice_0};
use oligopole_store::{DecisionOutboxEntry, DecisionSourceKind, OligopoleStore, to_hex};
use oligopole_ui::launch_native_terminal;
use std::env;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    match run_cli() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("oligopole-cli error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_cli() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let subcommand = args.first().map_or("sim-bench", String::as_str);

    match subcommand {
        "sim-bench" => {
            let mut ticks: u64 = 10_000;
            let mut seed_byte: u8 = 42;
            let mut save_path: Option<String> = None;
            let mut clef_addr: Option<String> = None;

            let mut idx = 1usize;
            while idx < args.len() {
                match args.get(idx).map(String::as_str) {
                    Some("--ticks") => {
                        if let Some(val) = args.get(idx + 1) {
                            ticks = val.parse::<u64>().map_err(|e| e.to_string())?;
                            idx += 2;
                        } else {
                            return Err("--ticks requires a number".to_owned());
                        }
                    }
                    Some("--seed") => {
                        if let Some(val) = args.get(idx + 1) {
                            seed_byte = val.parse::<u8>().map_err(|e| e.to_string())?;
                            idx += 2;
                        } else {
                            return Err("--seed requires a byte (0..=255)".to_owned());
                        }
                    }
                    Some("--save") => {
                        if let Some(val) = args.get(idx + 1) {
                            save_path = Some(val.clone());
                            idx += 2;
                        } else {
                            return Err("--save requires a file path".to_owned());
                        }
                    }
                    Some("--clef-flash") => {
                        let next_is_addr = args
                            .get(idx + 1)
                            .is_some_and(|v| !v.starts_with("--"));
                        if next_is_addr {
                            clef_addr = args.get(idx + 1).cloned();
                            idx += 2;
                        } else {
                            clef_addr = Some(DEFAULT_CLEF_WORKER_ADDR.to_owned());
                            idx += 1;
                        }
                    }
                    other => {
                        return Err(format!("unknown argument: {other:?}"));
                    }
                }
            }

            run_sim_bench(ticks, seed_byte, save_path.as_deref(), clef_addr.as_deref())
        }
        "verify-replay" => {
            let mut save_path: Option<String> = None;
            let mut idx = 1usize;
            while idx < args.len() {
                match args.get(idx).map(String::as_str) {
                    Some("--save") => {
                        if let Some(val) = args.get(idx + 1) {
                            save_path = Some(val.clone());
                            idx += 2;
                        } else {
                            return Err("--save requires a file path".to_owned());
                        }
                    }
                    other => return Err(format!("unknown argument: {other:?}")),
                }
            }
            let path = save_path.ok_or_else(|| "verify-replay requires --save <path>".to_owned())?;
            run_verify_replay(&path)
        }
        "terminal" => {
            let mut clef_addr: Option<String> = None;
            let mut idx = 1usize;
            while idx < args.len() {
                match args.get(idx).map(String::as_str) {
                    Some("--clef-flash") => {
                        let next_is_addr = args
                            .get(idx + 1)
                            .is_some_and(|v| !v.starts_with("--"));
                        if next_is_addr {
                            clef_addr = args.get(idx + 1).cloned();
                            idx += 2;
                        } else {
                            clef_addr = Some(DEFAULT_CLEF_WORKER_ADDR.to_owned());
                            idx += 1;
                        }
                    }
                    other => return Err(format!("unknown argument: {other:?}")),
                }
            }
            run_terminal(clef_addr.as_deref())
        }
        other => Err(format!(
            "unknown subcommand '{other}'. Expected 'sim-bench', 'verify-replay', or 'terminal'."
        )),
    }
}

fn run_sim_bench(
    ticks: u64,
    seed_byte: u8,
    save_path: Option<&str>,
    clef_addr: Option<&str>,
) -> Result<(), String> {
    let seed = [seed_byte; 32];
    let (mut world, _) = build_vertical_slice_0(seed).map_err(|e| e.to_string())?;
    let mut store = match save_path {
        Some(p) => OligopoleStore::open_file(p).map_err(|e| e.to_string())?,
        None => OligopoleStore::open_in_memory().map_err(|e| e.to_string())?,
    };

    let mut policy = AdaptiveDecisionPolicy::connect_or_fallback(clef_addr);

    // Save Genesis checkpoint (Tick 0)
    let genesis_hash = store.save_checkpoint(&world, &[]).map_err(|e| e.to_string())?;

    let start = Instant::now();
    let mut pending_outbox = Vec::new();
    let mut day_cmds = Vec::new();

    for day in 1..=ticks {
        day_cmds.clear();
        if day.is_multiple_of(30) {
            let corp_ids: Vec<_> = world.corporations.keys().collect();
            for (seq, cid) in corp_ids.into_iter().enumerate() {
                let Some((persona_kind, accepts_ai)) = world
                    .corporations
                    .get(cid)
                    .map(|c| (c.persona_kind, c.delegation.accepts_slow_tick_ai()))
                else {
                    continue;
                };
                if !accepts_ai {
                    continue;
                }
                let persona = PersonaBiasVector::from_archetype(persona_kind);
                let obs = ActorObservationView::build(&world, cid).map_err(|e| e.to_string())?;
                let (directive, probs_hash) = policy
                    .evaluate_actor_slow_tick(seed, &obs, &persona)
                    .map_err(|e| e.to_string())?;
                let source_kind = if policy.status().is_clef_flash() {
                    DecisionSourceKind::ClefFlashSlowTick
                } else {
                    DecisionSourceKind::System0Heuristic
                };
                let cmd = PlayerCommand::ApplyStrategicDirective(directive);
                day_cmds.push(cmd.clone());
                pending_outbox.push(DecisionOutboxEntry {
                    tick: day,
                    seq_no: u32::try_from(seq).unwrap_or(0),
                    source_kind,
                    command: cmd,
                    probs_blake3_hash: probs_hash,
                });
            }
        }

        world.step_fast_tick(&day_cmds).map_err(|e| e.to_string())?;

        // Persist checkpoint at the end of each simulated year (every 360 ticks) or final tick
        if day.is_multiple_of(360) || day == ticks {
            store
                .save_checkpoint(&world, &pending_outbox)
                .map_err(|e| e.to_string())?;
            pending_outbox.clear();
        }
    }

    let elapsed = start.elapsed();
    let view = world.build_world_view().map_err(|e| e.to_string())?;
    let (_, final_hash) = store.load_latest_snapshot().map_err(|e| e.to_string())?;

    #[allow(clippy::cast_precision_loss)]
    let tps = (ticks as f64) / elapsed.as_secs_f64().max(0.000_001);

    println!("================================================================================");
    println!(" OLIGOPOLE HEADLESS MONTE CARLO BENCHMARK — VERTICAL SLICE 0");
    println!("================================================================================");
    println!(
        " Simulated Ticks : {} days ({} on 30/360 calendar)",
        view.tick.0, view.date
    );
    println!(
        " Wall Time       : {elapsed:.2?} ({tps:.0} Fast Ticks / sec)"
    );
    println!(" AI Backend      : {}", policy.status().badge_label());
    println!(" Genesis Blake3  : {}", to_hex(&genesis_hash));
    println!(" Final Blake3    : {}", to_hex(&final_hash));
    println!(
        " Endogenous M1   : {} | Total Bank Loans: {}",
        view.total_m1_money_supply, view.total_bank_loans
    );
    println!("--------------------------------------------------------------------------------");
    println!(" TICKER | CORPORATION NAME                  | CASH         | LOANS        | SELL PX | EQ PX");
    println!("--------------------------------------------------------------------------------");
    for corp in &view.corporations {
        println!(
            " {:<6} | {:<33} | {:>12} | {:>12} | {:>7} | {:>7}",
            corp.ticker,
            corp.name,
            corp.cash_balance,
            corp.loans_payable,
            corp.posted_selling_price,
            corp.share_price
        );
    }
    println!("================================================================================");

    // Verify deterministic replay over the recorded checkpoints
    let (fresh_genesis, _) = build_vertical_slice_0(seed).map_err(|e| e.to_string())?;
    let verified = store
        .verify_deterministic_replay(fresh_genesis)
        .map_err(|e| e.to_string())?;
    println!(
        " Deterministic Replay Verification: PASSED ({verified} snapshot checkpoints bit-for-bit identical)"
    );

    Ok(())
}

fn run_verify_replay(path: &str) -> Result<(), String> {
    let store = OligopoleStore::open_file(path).map_err(|e| e.to_string())?;
    let (latest_world, _) = store.load_latest_snapshot().map_err(|e| e.to_string())?;
    let (genesis_world, _) =
        build_vertical_slice_0(latest_world.world_seed).map_err(|e| e.to_string())?;
    let verified = store
        .verify_deterministic_replay(genesis_world)
        .map_err(|e| e.to_string())?;
    println!(
        "Replay verification succeeded for '{path}': {verified} checkpoints matched bit-for-bit."
    );
    Ok(())
}

fn run_terminal(clef_addr: Option<&str>) -> Result<(), String> {
    let (world, _) = build_vertical_slice_0([42u8; 32]).map_err(|e| e.to_string())?;
    launch_native_terminal(world, clef_addr)
}
