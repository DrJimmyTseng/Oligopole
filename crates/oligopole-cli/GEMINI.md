# Bounded Context: `oligopole-cli` — Headless Monte Carlo Runner, Replay Verifier, Multiplayer Server & Terminal Launcher

## 1. Domain Purpose & Scope
`oligopole-cli` is the application composition root wiring `oligopole-ledger`, `oligopole-core`, `oligopole-ai`, `oligopole-store`, `oligopole-server`, and `oligopole-ui`.

## 2. Subcommands
- `sim-bench --ticks <N> --seed <S> [--save <PATH>] [--clef-flash [ADDR]]`: Executes $N$ Fast Ticks headlessly via `AdaptiveDecisionPolicy`, auditing double-entry trial balance, Dual-Cash M1 conservation, WAC inventory conservation, `CapTable` share conservation, DR unit conservation, and `ServiceCredit` conservation on every tick.
- `verify-replay --save <PATH>`: Loads a `.oligopole` file, replays from `world_seed` + `decision_outbox_journal` using `(tick, seq_no)` ordering, and verifies bit-for-bit `blake3_state_hash` identity against recorded snapshots.
- `serve [--bind 127.0.0.1:19510] [--save <PATH>] [--tick-ms 15000] [--webhook-secret <SECRET>] [--clef-flash [ADDR]]`: Launches the authoritative **Black Tape** Dual-Clock multiplayer Axum server.
- `terminal [--save <PATH>] [--clef-flash [ADDR]] [--connect ws://127.0.0.1:19510/ws --token PLYR-TOKEN]`: Launches the 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal in either local single-player mode or remote multiplayer WebSocket mode.
