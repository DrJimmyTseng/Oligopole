# Bounded Context: `oligopole-cli` — Headless Monte Carlo Runner, Replay Verifier & Terminal Launcher

## 1. Domain Purpose & Scope
`oligopole-cli` is the application composition root wiring `oligopole-ledger`, `oligopole-core`, `oligopole-ai`, `oligopole-store`, and `oligopole-ui`.

## 2. Subcommands
- `sim-bench --ticks <N> --seed <S> [--save <PATH>]`: Executes $N$ Fast Ticks headlessly (default 10,000 ticks = 27.7 simulated 30/360 years) with `System0HeuristicPolicy`, auditing double-entry trial balance on every tick and reporting throughput and macroeconomic telemetry.
- `verify-replay --save <PATH>`: Loads a `.oligopole` file, replays from `world_seed` + `decision_outbox_journal`, and verifies bit-for-bit `blake3_state_hash` identity against recorded snapshots.
- `terminal`: Launches the 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal connected to the background simulation thread.
