# Bounded Context: `oligopole-store` — SQLite WAL `.oligopole` Persistence & Deterministic Replay

## 1. Domain Purpose & Scope
`oligopole-store` manages the `.oligopole` self-contained SQLite WAL save-file container using synchronous bundled `rusqlite`. It stores `zstd`-compressed `postcard` Arena snapshots, `blake3` state hashes, the append-only Transactional Outbox (`decision_outbox_journal`) for deterministic replay verification, and sortable 16-byte big-endian `i128` historical rollups (`market_ohlcv_history`, `financial_statement_rollups`).

## 2. Core Modules
- **`encoding.rs`:** `zstd(postcard)` compression/decompression, `blake3` canonical state hashing, and sortable 16-byte big-endian `Money(i128)` BLOB helpers.
- **`schema.rs`:** 5-table `STRICT` DDL schema (`world_meta`, `world_snapshots`, `decision_outbox_journal`, `market_ohlcv_history`, `financial_statement_rollups`) and WAL pragmas.
- **`outbox.rs`:** Append-only journal recording `PlayerCommand` (`DecisionSourceKind::PlayerManual`) and Slow-Tick `StrategicDirective` (`ClefFlashSlowTick` or `System0Heuristic`) events with probability hashes for bit-for-bit replay verification.
- **`repository.rs`:** `OligopoleStore` supporting file-backed `.oligopole` saves (with startup session resumption and live UI `Ctrl+S` / annual checkpointing in `oligopole-ui`), alongside `:memory:` databases for headless `proptest` / Monte Carlo runs.

## 3. Invariants Verified by Tests
1. `zstd(postcard)` world snapshots roundtrip with exact `blake3` hash verification.
2. SQLite `ORDER BY net_income_be16 DESC` on 16-byte sign-flipped big-endian `Money(i128)` BLOBs matches `Money::Ord`.
3. `verify_deterministic_replay` replays from Genesis `WorldState` + `decision_outbox_journal` and confirms bit-for-bit `blake3_state_hash` identity at every checkpoint tick.

## 4. Status
- **Status:** Complete & Verified (`1/1` end-to-end replay & sortable BLOB test passing, zero Clippy warnings).
