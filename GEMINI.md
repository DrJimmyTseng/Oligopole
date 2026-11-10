# Oligopole — Strategic Domain-Driven Design (DDD) & Architectural Specification

## 1. Vision & Core Architectural Guardrails

**Oligopole** is a deterministic, emergent macroeconomic and corporate simulation engine written in a 100% Rust monorepo (`edition = "2024"`, `rust-version = "1.87"`), paired with a native 120 FPS `egui` / `egui_tiles` Bloomberg-style terminal UI and a local NVIDIA RTX 5090 `Clef-Flash` (9B, BF16) non-autoregressive decision sidecar.

### Non-Negotiable Engineering Invariants
1. **Zero `unsafe`, Zero `panic!`, Zero `unwrap()` / `expect()` in Production Paths:**
   - Enforced across the workspace via `[workspace.lints]`. Panics are permitted exclusively inside `#[cfg(test)]` modules.
2. **Bit-for-Bit Determinism & Banned Randomized Hash Collections:**
   - `std::collections::HashMap` and `std::collections::HashSet` are forbidden via `clippy.toml` (`disallowed-types`). All keyed collections use `std::collections::BTreeMap`, `std::collections::BTreeSet`, or `slotmap::SlotMap` to guarantee canonical iteration order and reproducible `postcard` + `blake3` state hashes.
3. **Closed-Loop `i128` Double-Entry Accounting:**
   - All monetary balances and ledger postings use `Money(i128)` minor units (`1 cent = 1`).
   - Pro-rata allocations (`allocate_hamilton`) and Weighted Average Cost (`WacInventoryLot`) inventory draws use safe widening 256-bit intermediate division (`u256_mul_div_rem`) with `u64` weights/quantities so `proptest` cannot overflow `i128`.
   - Money enters and exits the private sector exclusively through the **Endogenous Commercial & Central Bank Credit Facility** (loans mint deposit liabilities and matching loan assets; defaults execute a liquidation waterfall and write down bank equity against loan assets).
4. **30/360 Institutional Calendar & Dual-Tick Execution:**
   - **Fast Tick (Daily):** `30 days/month`, `90 days/quarter`, `360 days/year`, divided into 5 deterministic daily sub-phases.
   - **Slow Tick (Monthly — Every 30 Fast Ticks):** Rust generates NPV-filtered `JointActionTuple` candidates and orthogonal policy dials from each actor's asymmetric `ActorObservationView`, evaluates probabilities via `DecisionPolicy` (`ClefFlashSidecar` or `System0Heuristic`), applies `PersonaBiasVector` log-odds shifts in `rust_decimal`, samples via `ChaCha8Rng(world_seed, tick, actor_id)`, and enforces cash/debt guardrails via the deterministic Rust CFO solver.
5. **Mandatory TDD, Living Documentation & GitHub Sync Protocol:**
   - Every feature or bugfix begins with unit and/or `proptest` tests.
   - After every successful compilation and test pass (`cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`), update the relevant bounded-context `GEMINI.md` file, commit with a descriptive message, and push immediately to `origin/main` (`https://github.com/DrJimmyTseng/Oligopole.git`).

---

## 2. Bounded Context Map

| Bounded Context | Path | Role | Upstream Dependencies |
| :--- | :--- | :--- | :--- |
| **`oligopole-ledger`** | `crates/oligopole-ledger` | **Shared Kernel / Core Accounting Domain:** `Money(i128)`, widening 256-bit math, Hamilton Largest Remainder, `RoundingReserve`, Double-Entry `LedgerEngine`, `WacInventoryLot`, `CommercialBankFacility`, and Financial Statements. | None (Leaf Domain Crate) |
| **`oligopole-core`** | `crates/oligopole-core` | **Core Simulation Domain:** `Calendar360`, `SlotMap` entity arenas, reservation-escrow `CentralLimitOrderBook` (CLOB), `B2BMarket`, Stone-Geary + scaling-and-squaring Padé `(2,2)` MNL `RetailMarket`, `ProductionEngine`, `CfoSolver`, `WorldState`, and Vertical Slice 0. | `oligopole-ledger` |
| **`oligopole-ai`** | `crates/oligopole-ai` | **Decision & Candidate Subdomain:** Asymmetric `ActorObservationView`, `JointActionTuple` candidate generator, `PersonaBiasVector` log-odds modulator, `System0HeuristicPolicy`, and `127.0.0.1:19509` `TCP_NODELAY` `msgpack` client. | `oligopole-core`, `oligopole-ledger` |
| **`oligopole-store`** | `crates/oligopole-store` | **Persistence Infrastructure:** Synchronous bundled `rusqlite` WAL `.oligopole` container, 5-table schema, sortable 16-byte big-endian `i128` BLOBs, `zstd(postcard)` snapshots, and `blake3` replay verification. | `oligopole-core`, `oligopole-ledger`, `oligopole-ai` |
| **`oligopole-ui`** | `crates/oligopole-ui` | **Presentation Layer:** 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal (`<TICKER> <MARKET> <MNEMONIC>` command bar, linked Channels A/B/C, OHLCV charts, CLOB depth, Financial Statements, Operations DAG, Executive Ticket). | `oligopole-core`, `oligopole-ledger` |
| **`oligopole-cli`** | `crates/oligopole-cli` | **Application Composition Root:** CLI entry point (`sim-bench`, `verify-replay`, `terminal`). | All workspace crates |
| **`clef-flash-worker`** | `workers/clef-flash-worker` | **AI Inference Sidecar:** Python/Torch local RTX 5090 BF16 `Clef-Flash` 9B single-pass prefill worker (`msgspec` over `TCP_NODELAY`). | Wire protocol defined in `oligopole-ai` |

---

## 3. Ubiquitous Language Glossary

- **`Money(i128)`:** Signed 128-bit integer count of minor currency units (cents). Never converted to IEEE-754 floats.
- **`HamiltonAllocation`:** Largest Remainder pro-rata distribution sorting fractional remainders by `(Remainder DESC, Id ASC)` so $\sum \text{shares}_i == \text{total}$ with zero cent leakage.
- **`RoundingReserve`:** Per-entity integer numerator accumulator carrying forward sub-cent remainders across daily 30/360 interest or tax rate accruals.
- **`WacInventoryLot`:** $O(1)$ Weighted Average Cost SKU tracker `(total_qty: u64, total_book_value: Money, reserved_qty: u64)` that sweeps exact remaining book value when `qty_drawn == total_qty`.
- **`EndogenousCreditFacility`:** Commercial bank balance-sheet operation that originates loans by simultaneously debiting `LoanReceivable` (Asset) and crediting `CustomerDepositsLiability` (Liability).
- **`ReservationEscrow`:** CLOB order collateral lock using `reserved_cash`, `reserved_qty`, and `reserved_shares` counters so legal ownership, WAC cost basis, and dividend eligibility remain intact until trade fill.
- **`JointActionTuple`:** Composite strategic candidate pairing a capital allocation action (e.g., factory expansion) with its exact financing method (e.g., cash vs. senior bank loan) so parallel `Clef-Flash` `choice` queries never produce mutually incompatible capital structures.
- **`ActorObservationView`:** Strict information-barrier projection exposing only an actor's private internal books, lagged public quarterly filings of rivals, and noisy market estimates.

---

## 4. Build & Milestone Log

- **Milestone 1 (Completed):** Workspace scaffolding, `.cargo/config.toml` Proton Drive build isolation, `clippy.toml` deterministic collection enforcement, hierarchical DDD `GEMINI.md` specifications, and full `oligopole-ledger` implementation (`Money(i128)` sortable BE16 encoding, widening 256-bit `u256_mul_div_rem`, Hamilton pro-rata splitter, `RoundingReserve`, `WacInventoryLot` with reservation escrow, `LedgerEngine`, `CommercialBankFacility`, `FinancialStatementReport`, and `proptest` closed-loop monetary conservation suite).
- **Milestone 2 (Completed):** `oligopole-core` deterministic 30/360 Fast-Tick engine (5 daily sub-phases), reservation-escrow `CentralLimitOrderBook` (CLOB) with price-improvement escrow release, Stone-Geary + scaling-and-squaring Padé `(2,2)` MNL `RetailMarket`, `B2BMarket`, `ProductionEngine`, `CfoSolver`, `WorldView` snapshot projection, and Vertical Slice 0 ("Iron-to-Consumer + Equity CLOB") verified via `proptest`.
- **Milestone 3 (Completed):** `oligopole-ai` Candidate Generator (`ActorObservationView`, `JointActionTuple`, `PersonaBiasVector` log-odds modulator, `System0HeuristicPolicy`, and `127.0.0.1:19509` `msgpack` TCP client) + `workers/clef-flash-worker` Python `msgspec` sidecar + `oligopole-store` (`rusqlite` WAL `.oligopole` container, 5-table `STRICT` schema, sortable BE16 `i128` columns, and bit-for-bit `blake3` replay verifier).
- **Milestone 4 (Completed):** `oligopole-ui` 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal (`<TICKER> <MARKET> <MNEMONIC>` command bar, Channel A/B/C linking, Market Monitor, OHLCV Chart `GP`, L2 Order Book `OB`, Financial Statements `FA`, Supply-Chain WAC DAG `OPS`, Executive Ticket `EX`) & `oligopole-cli` (`sim-bench` running 10,000 ticks at ~49,000+ TPS in 203ms with 29/29 bit-for-bit `blake3` replay checkpoints verified).
