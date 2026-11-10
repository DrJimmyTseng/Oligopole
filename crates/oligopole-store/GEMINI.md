# Bounded Context: `oligopole-store` — SQLite WAL `.oligopole` Persistence & Deterministic Replay

## 1. Domain Purpose & Scope
`oligopole-store` manages the `.oligopole` self-contained SQLite WAL save-file container using synchronous bundled `rusqlite`. It stores `zstd`-compressed `postcard` Arena snapshots, `blake3` state hashes, the append-only Transactional Outbox (`decision_outbox_journal`) for deterministic replay verification, and sortable 16-byte big-endian `i128` historical rollups (`market_ohlcv_history`, `financial_statement_rollups`).

## 2. Core Modules
- **`encoding.rs`:** `zstd(postcard)` compression/decompression, `blake3` canonical state hashing, and sortable 16-byte big-endian `Money(i128)` BLOB helpers.
- **`schema.rs`:** 5-table `STRICT` DDL schema (`world_meta`, `world_snapshots`, `decision_outbox_journal`, `market_ohlcv_history`, `financial_statement_rollups`) and WAL pragmas.
- **`outbox.rs`:** Append-only journal recording `PlayerCommand` and Slow-Tick `StrategicDirective` events with probability hashes for bit-for-bit replay verification.
- **`repository.rs`:** `OligopoleStore` supporting both file-backed `.oligopole` saves and `:memory:` databases for headless `proptest` / Monte Carlo runs.
