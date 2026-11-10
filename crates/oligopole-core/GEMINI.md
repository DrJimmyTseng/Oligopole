# Bounded Context: `oligopole-core` — Deterministic Arena Simulation, 30/360 Calendar & 3-Tier Markets

## 1. Domain Purpose & Scope
`oligopole-core` implements the deterministic Fast-Tick simulation engine on a 30/360 financial calendar (5 daily sub-phases), stratified 3-tier market clearing (Commodity & Equity Price-Time Priority CLOB with reservation escrow, B2B bilateral supply contracts + spot wholesale, and B2C posted-price retail shelves with Stone-Geary + scaling-and-squaring Padé `(2,2)` Multinomial Logit demand), multi-stage WAC production recipes, and the deterministic CFO constraint solver.

## 2. Core Aggregates & Modules
- **`ids.rs`:** Typed `SlotMap` keys (`ActorId`, `InstrumentId`, `SkuId`, `FacilityId`) and deterministic order/contract sequence IDs.
- **`calendar.rs`:** `Tick(u64)`, `Date360 { year, month, day }` (30 days/month, 90 days/quarter, 360 days/year), and `DailySubPhase`.
- **`clob.rs`:** `CentralLimitOrderBook` with price-time priority (`BTreeMap<Money, VecDeque<LimitOrder>>`), reservation-counter escrow (`reserved_cash`, `reserved_qty`, `reserved_shares`), and `OhlcvBar` aggregation.
- **`b2b_market.rs`:** Scheduled bilateral supply contract tranche delivery and spot wholesale queue matching.
- **`household.rs` & `retail_market.rs`:** 3 income cohorts (`WageEarners`, `ManagersSkilled`, `CapitalistsExecutives`), Stone-Geary subsistence reserve, clamped `[-20, 0]` scaling-and-squaring `pade_2_2(x / 32)^32` logit choice in `rust_decimal`, and Hamilton inventory rationing.
- **`production.rs`:** Factory DAG recipes transforming input `WacInventoryLot`s + labor into output finished goods, `ExecutivePersonaKind` archetypes, `AiDelegationConfig::accepts_slow_tick_ai`, and `Corporation::working_capital_floor` computing $N$-day payroll + 30/360 interest buffers.
- **`cfo.rs`:** Deterministic corporate finance constraint solver enforcing 20-day working-capital floors, 3x working-capital debt service covenants, and safe share buyback/issuance budgets.
- **`world.rs` & `vertical_slice.rs`:** `WorldState` aggregate, 5-subphase Fast-Tick pipeline (with closed-loop secondary Equity CLOB market-making `run_subphase_3_equity_market_making`, fundamental share valuation `compute_fundamental_share_valuation`, corporate share buybacks & primary issuance, 85% monthly bank net interest recycling to households, 180-day excess-cash debt amortization & dividend sweeps, and 5-day survival working-capital protection), `WorldView` snapshot projection, and Vertical Slice 0 (6 corporations with distinct `ExecutivePersonaKind` profiles: `IRON` & `VALE` extractors, `ACME` & `ZENI` manufacturers, `PLYR` & `APEX` retailers).

## 3. Invariants Verified by `proptest`
1. `pade_exp_neg(x)` is strictly positive `(0, 1]` and strictly monotonic across `[-20, 0]`, eliminating the raw Padé `(2,2)` numerator parabola minimum at `x = -3`.
2. `WorldState::audit_world_invariants()` holds at every tick across 360-day full-year runs and randomized multi-month `StrategicDirective` injections (including `RepurchaseShares` and `IssueEquityShares`), verifying `LedgerEngine::audit_trial_balance()`, exact cent-for-cent identity between `WacInventoryLot::total_book_value()` and ledger `InventoryRaw`/`InventoryFinished` balances, and exact `CapTable` share conservation (`sum(owned_shares) == total_shares_outstanding` and `reserved_shares <= owned_shares`).

## 4. Status
- **Status:** Complete & Verified (`6/6` unit and `proptest` suites passing, zero Clippy warnings).
