# Bounded Context: `oligopole-core` — Deterministic Arena Simulation, 30/360 Calendar & 3-Tier Markets

## 1. Domain Purpose & Scope
`oligopole-core` implements the deterministic Fast-Tick simulation engine on a 30/360 financial calendar (5 daily sub-phases), stratified 3-tier market clearing (Commodity & Equity Price-Time Priority CLOB with reservation escrow, B2B bilateral supply contracts + spot wholesale, and B2C posted-price retail shelves with Stone-Geary + scaling-and-squaring Padé `(2,2)` Multinomial Logit demand), multi-stage WAC production recipes, and the deterministic CFO constraint solver.

## 2. Core Aggregates & Modules
- **`ids.rs`:** Typed `SlotMap` keys (`ActorId`, `InstrumentId`, `SkuId`, `FacilityId`) and deterministic order/contract sequence IDs.
- **`calendar.rs`:** `Tick(u64)`, `Date360 { year, month, day }` (30 days/month, 90 days/quarter, 360 days/year), and `DailySubPhase`.
- **`clob.rs`:** `CentralLimitOrderBook` with price-time priority (`BTreeMap<Money, VecDeque<LimitOrder>>`), reservation-counter escrow (`reserved_cash`, `reserved_qty`, `reserved_shares`), and `OhlcvBar` aggregation.
- **`b2b_market.rs`:** Scheduled bilateral supply contract tranche delivery and spot wholesale queue matching.
- **`household.rs` & `retail_market.rs`:** 3 income cohorts (`WageEarners`, `ManagersSkilled`, `CapitalistsExecutives`), Stone-Geary subsistence reserve, clamped `[-20, 0]` scaling-and-squaring `pade_2_2(x / 32)^32` logit choice in `rust_decimal`, and Hamilton inventory rationing.
- **`production.rs`:** Factory DAG recipes transforming input `WacInventoryLot`s + labor into output finished goods.
- **`cfo.rs`:** Deterministic corporate finance constraint solver enforcing minimum cash buffers and debt service covenants.
- **`world.rs` & `vertical_slice.rs`:** `WorldState` aggregate, 5-subphase Fast-Tick pipeline, `WorldView` snapshot projection, and Vertical Slice 0 ("Iron-to-Consumer + Equity CLOB").
