# Bounded Context: `oligopole-cli` — Headless Monte Carlo Runner, Replay Verifier & Terminal Launcher

## 1. Domain Purpose & Scope
`oligopole-cli` is the application composition root wiring `oligopole-ledger`, `oligopole-core`, `oligopole-ai`, `oligopole-store`, and `oligopole-ui`.

## 2. Subcommands
- `sim-bench --ticks <N> --seed <S> [--save <PATH>]`: Executes $N$ Fast Ticks headlessly (default 10,000 ticks = 27.7 simulated 30/360 years) with `System0HeuristicPolicy`, auditing double-entry trial balance on every tick and reporting throughput and macroeconomic telemetry.
- `verify-replay --save <PATH>`: Loads a `.oligopole` file, replays from `world_seed` + `decision_outbox_journal`, and verifies bit-for-bit `blake3_state_hash` identity against recorded snapshots.
- `terminal`: Launches the 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal connected to the background simulation thread.

## 3. Benchmark & Replay Verification Status
- `cargo run --release -p oligopole-cli -- sim-bench --ticks 10000 --seed 42`:
  - Simulates 10,000 Fast Ticks (28 simulated 30/360 years) in ~`203ms` (~`49,000+ Fast Ticks/sec`).
  - Verifies closed-loop `LedgerEngine::audit_trial_balance()` and WAC inventory conservation on every tick.
  - Verifies 29/29 annual `zstd(postcard)` snapshots bit-for-bit via `verify_deterministic_replay`.
