# Oligopole

**Oligopole** is a deterministic, emergent macroeconomic and corporate simulation engine written in a 100% Rust monorepo (`edition = "2024"`, `rust-version = "1.87"`), paired with a native 120 FPS `eframe` / `egui_tiles` Bloomberg-style terminal UI and a local NVIDIA RTX 5090 `Clef-Flash` (9B, BF16) non-autoregressive decision sidecar.

![Oligopole 120 FPS 6-Pane Bloomberg Terminal](docs/assets/terminal-hero.png)

---

## Core Architectural Pillars

### 1. Closed-Loop `i128` Double-Entry Accounting (`oligopole-ledger`)
- **Zero IEEE-754 Floats on Balances:** Every balance sheet and income statement account uses `Money(i128)` minor currency units (`1 cent = 1`).
- **256-Bit Widening Arithmetic (`u256_mul_div_rem`):** Pro-rata allocations (`allocate_hamilton`) and Weighted Average Cost (`WacInventoryLot`) inventory draws use overflow-proof widening 256-bit division with `u64` weights/quantities, sweeping exact remaining book value when a lot depletes (`qty_drawn == total_qty`).
- **30/360 Institutional Calendar & `RoundingReserve`:** Daily interest accruals carry forward sub-cent remainders per entity across 30-day months and 360-day years with zero cent leakage.
- **Endogenous Commercial & Central Bank Credit Facility:** Money enters and exits the private sector exclusively through balanced commercial bank balance-sheet postings (`originate_loan` mints deposit liabilities and matching loan assets; `repay_loan_principal` extinguishes them; `liquidate_and_write_off_default` executes an insolvency waterfall). At month-end, 85% of positive bank net interest income is recycled back to households as bank staff salaries (`35%`) and bank dividends (`50%`) so debt service circulates rather than draining M1.

### 2. Stratified 3-Tier Market Microstructure & Dual-Tick Engine (`oligopole-core`)
- **Fast Tick (Daily — 5 Intraday Sub-Phases):**
  1. **Morning B2B & Wholesale Replenishment:** Scheduled bilateral `B2BSupplyContract` deliveries and price-sorted spot wholesale procurement (protecting a 20-day working-capital payroll + interest floor).
  2. **Factory DAG Production & Resource Extraction:** Capitalizes extraction labor and transforms raw material `WacInventoryLot`s into finished goods.
  3. **Price-Time Priority CLOB Matching (`CentralLimitOrderBook`):** Matches Commodity (`IRON CMDTY`) and Secondary Equity (`IRON EQ`, `VALE EQ`, `ACME EQ`, `ZENI EQ`, `PLYR EQ`, `APEX EQ`) limit orders using **Reservation-Counter Escrow** (`reserved_cash`, `reserved_qty`, `reserved_shares`). Institutional/household shareholders (`CapitalistsExecutives`, `ManagersSkilled`, `WageEarners`) post closed-loop two-sided equity market-making quotes and rebalance portfolios daily off trailing earnings yield, dividend yield, and GAAP book value, alongside corporate share buybacks and primary issuance.
  4. **B2C Retail Clearing:** Evaluates 3 stratified household cohorts (`WageEarners`, `ManagersSkilled`, `CapitalistsExecutives`) with Stone-Geary subsistence floors and a strictly monotonic clamped `[-20, 0]` scaling-and-squaring Padé `(2,2)` Multinomial Logit (MNL) demand model (`pade_2_2(x / 32)^32`) in `rust_decimal`.
  5. **Evening Payroll, 30/360 Interest, Dividends & Trial Balance Audit:** Settles payroll and interest (protecting a 5-day survival floor), distributes monthly Hamilton pro-rata dividends (plus a 180-day excess-cash hoard sweep), closes quarterly income statements into `RetainedEarnings`, and audits global double-entry and `CapTable` share conservation.
- **Slow Tick (Monthly — Every 30 Fast Ticks):**
  - Projects an asymmetric `ActorObservationView` per corporation, generates NPV-ranked `JointCapitalAction` tuples (`HoldAndDelever`, `ExpandCapacityCashFunded`, `ExpandCapacityDebtFunded`, `BrandAndQualityCampaign`, `SignB2BContract`, `RepurchaseShares`, `IssueEquityShares`) alongside orthogonal policy dials (`PricingStance`, `WageStance`, `DividendStance`), and reconciles sampled directives through the deterministic Rust CFO constraint solver (`reconcile_strategic_directive`).

### 3. `Clef-Flash` 9B Sidecar, Executive Personas & Deterministic Replay (`oligopole-ai` & `oligopole-store`)
- **Adaptive `DecisionPolicy` (`AdaptiveDecisionPolicy`):** Runs out-of-the-box on any CPU at 35,000+ Fast Ticks/sec via `System0HeuristicPolicy`, or connects over `127.0.0.1:19509` (`TCP_NODELAY` + 4-byte big-endian length-prefixed `msgpack`) to the local Python/Torch RTX 5090 `Clef-Flash` 9B worker (`workers/clef-flash-worker/worker.py`) with automatic graceful fallback if the sidecar is offline.
- **Heterogeneous Executive Persona Archetypes (`ExecutivePersonaKind`):** Each corporation is governed by a distinct `PersonaBiasVector` archetype (`DividendAristocrat` for `IRON`, `AggressiveEmpireBuilder` for `VALE`, `QualityMoatCompounder` for `ACME`, `CostUndercutChallenger` for `ZENI`, `BalancedExecutive` for `PLYR`, and `OpportunisticCapitalAllocator` for `APEX`). Probability distributions are shifted in `rust_decimal` log-odds space, normalized to `10,000 bps` via `allocate_hamilton`, and sampled via `ChaCha8Rng(world_seed, tick, actor_id)`.
- **Self-Contained `.oligopole` SQLite WAL Save Container:** Stores `zstd(postcard)` world snapshots, `blake3` state hashes, 16-byte sign-flipped big-endian sortable `Money(i128)` columns (`self.0.cast_unsigned() ^ (1u128 << 127)`), and the `decision_outbox_journal` for bit-for-bit deterministic replay verification.

### 4. Native 120 FPS Bloomberg-Style Terminal (`oligopole-ui`)
- Built with `eframe`, `egui_tiles`, `egui_plot`, and `egui_extras::TableBuilder`:
  - **Command Bar (`Ctrl+K` or `/`):** `<TICKER> <MARKET> <MNEMONIC>` syntax (e.g., `IRON CMDTY OB`, `VALE EQUITY GP`, `PLYR EQUITY FA`, `ACME EQUITY EX`).
  - **Live AI Backend Telemetry Badge:** Displays `[AI: SYSTEM-0 HEURISTIC (CPU)]` or `[AI: CLEF-FLASH RTX 5090 (BF16)]` in the top command bar.
  - **Color-Coded Channel Linking (`CH A` Red, `CH B` Amber, `CH C` Cyan):** Selecting a corporation in the Market Monitor updates all linked panes simultaneously.
  - **6 Dockable Institutional Panes:** Market Monitor (`MM`), OHLCV Price & Volume Chart (`GP`), L2 CLOB Depth & Tape (`OB`), Balance Sheet & Income Statement (`FA`), Supply-Chain WAC DAG (`OPS`), and Executive Order & AI Delegation Ticket (`EX`).

---

## Monorepo Structure

```text
Oligopole/
├── Cargo.toml                        # Workspace root (Rust 2024, rust-version = 1.87, strict lints)
├── clippy.toml                       # Bans std::collections::HashMap & HashSet for determinism
├── GEMINI.md                         # Root Strategic DDD & Architectural Specification
├── crates/
│   ├── oligopole-ledger/             # Shared Kernel: Money(i128), u256 math, Hamilton, WAC, Bank, Ledger
│   ├── oligopole-core/               # Core Domain: 30/360 Calendar, CLOB, B2B, MNL Retail, CFO, WorldState
│   ├── oligopole-ai/                 # AI Subdomain: ActorObservationView, Candidates, Persona, TCP msgpack
│   ├── oligopole-store/              # Persistence: rusqlite WAL .oligopole container & blake3 replay verifier
│   ├── oligopole-ui/                 # Presentation: 120 FPS eframe / egui_tiles Bloomberg Terminal
│   └── oligopole-cli/                # Composition Root: sim-bench, verify-replay, terminal
└── workers/
    └── clef-flash-worker/            # Local RTX 5090 BF16 Clef-Flash 9B msgpack TCP sidecar
```

---

## Quick Start

> **Runs Out-of-the-Box on Any CPU (No GPU Required):** Both `terminal` and `sim-bench` run natively out-of-the-box on any CPU via the deterministic `System0HeuristicPolicy`. If you have the local Python `clef-flash-worker` sidecar running, pass `--clef-flash 127.0.0.1:19509` (or `--clef-flash`) to route Monthly Slow-Tick evaluations through `Clef-Flash` 9B with automatic graceful fallback to `System0HeuristicPolicy` if the sidecar is offline.

### 1. Run Lints & Full Test Suite (`proptest` + Unit + Integration)
```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### 2. Run a 10,000-Tick Headless Monte Carlo Benchmark (~28 Simulated Years)
```powershell
# Pure-Rust CPU mode (default — no GPU or Python worker required):
cargo run --release -p oligopole-cli -- sim-bench --ticks 10000 --seed 42 --save sim_run.oligopole

# Optional: Connect to local Clef-Flash 9B sidecar (gracefully falls back to CPU if offline):
cargo run --release -p oligopole-cli -- sim-bench --ticks 10000 --seed 42 --clef-flash 127.0.0.1:19509
```
Example output:
```text
================================================================================
 OLIGOPOLE HEADLESS MONTE CARLO BENCHMARK — VERTICAL SLICE 0
================================================================================
 Simulated Ticks : 10000 days (Y28-M10-D11 on 30/360 calendar)
 Wall Time       : 279.38ms (35794 Fast Ticks / sec)
 AI Backend      : AI: SYSTEM-0 HEURISTIC (CPU)
 Genesis Blake3  : 5e4bfe21b6317b230af3a7a9e33691771a917ffef6b844240b0e9a1cf6fe442f
 Final Blake3    : b7c59cf035626f7545911db6759eff3bb351f92a49faec903f1e19fc5989550a
 Endogenous M1   : $15174119.45 | Total Bank Loans: $227053.41
--------------------------------------------------------------------------------
 TICKER | CORPORATION NAME                  | CASH         | LOANS        | SELL PX | EQ PX
--------------------------------------------------------------------------------
 IRON   | IronWorks Mining Corp             |     $4750.24 |    $31595.89 |  $44.62 |   $8.11
 VALE   | Vale Basin Resources Corp         |     $4600.23 |    $72400.34 |  $49.61 |   $7.98
 ACME   | Acme Appliances Corp              |     $6750.34 |     $8948.33 | $127.18 |   $8.12
 ZENI   | Zenith Manufacturing Corp         |     $6500.32 |   $114108.85 | $121.60 |   $8.21
 PLYR   | Oligopole Retail Group (Player)   |     $5500.27 |        $0.00 | $257.12 | $285.43
 APEX   | Apex MegaMart Corp (Rival)        |     $5500.27 |        $0.00 | $247.50 | $336.77
================================================================================
 Deterministic Replay Verification: PASSED (29 snapshot checkpoints bit-for-bit identical)
```

### 3. Verify Bit-for-Bit Deterministic Replay of a `.oligopole` Save Container
```powershell
cargo run --release -p oligopole-cli -- verify-replay --save sim_run.oligopole
```

### 4. Launch the Native 120 FPS Bloomberg-Style Terminal
```powershell
# Launch out-of-the-box on CPU:
cargo run --release -p oligopole-cli -- terminal

# Resume or persist a live .oligopole container (press Ctrl+S or click SAVE in the UI):
cargo run --release -p oligopole-cli -- terminal --save session.oligopole

# Or launch with optional Clef-Flash 9B sidecar connection:
cargo run --release -p oligopole-cli -- terminal --save session.oligopole --clef-flash 127.0.0.1:19509
```

### 5. Start the Optional Local RTX 5090 `Clef-Flash` 9B Sidecar Worker
```powershell
python workers/clef-flash-worker/worker.py --host 127.0.0.1 --port 19509
```

---

## Engineering Guardrails
- **Zero `unsafe` Code:** `unsafe_code = "forbid"` across the entire workspace.
- **Zero `unwrap()`, `expect()`, `panic!`, `todo!`, or `unimplemented!` in Production Paths:** Enforced via `[workspace.lints.clippy]`.
- **Deterministic Iteration Order:** `std::collections::HashMap` and `std::collections::HashSet` are banned via `clippy.toml` (`disallowed-types`) in favor of `BTreeMap`, `BTreeSet`, and `slotmap::SlotMap`.
