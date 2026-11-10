# Bounded Context: `oligopole-core` — Deterministic Arena Simulation, 30/360 Calendar & 3-Tier Markets

## 1. Domain Purpose & Scope
`oligopole-core` implements the deterministic Fast-Tick simulation engine on a 30/360 financial calendar (5 daily sub-phases), stratified 3-tier market clearing (Commodity, Domestic Equity, Global Depositary Receipt, and `ServiceCredit` (`PLEX`) Price-Time Priority CLOB with reservation escrow and $\pm 10\%$ LULD volatility bands, B2B bilateral supply contracts + spot wholesale, and B2C posted-price retail shelves with Stone-Geary + scaling-and-squaring Padé `(2,2)` Multinomial Logit demand), multi-stage WAC production recipes, organised crime & forensic money laundering, Corporate Wire (`MSG`) smart attachments, and the deterministic CFO constraint solver.

## 2. Core Aggregates & Modules
- **`ids.rs`:** Typed `SlotMap` keys (`CorpId`, `CohortId`, `InstrumentId`, `SkuId`) and deterministic sequence IDs (`OrderId`, `ContractId`, `WireMessageId`, `ShortLoanId`).
- **`calendar.rs`:** `Tick(u64)`, `Date360 { year, month, day }` (30 days/month, 90 days/quarter, 360 days/year), and `DailySubPhase`.
- **`clob.rs`:** `CentralLimitOrderBook` supporting `Commodity`, `Equity`, `DepositaryReceipt` (`GOOGL-DR`, `META-DR`, `NVDA-DR`), and `ServiceCredit` (`PLEX SC`) with price-time priority (`BTreeMap<Money, VecDeque<LimitOrder>>`), reservation-counter escrow (`reserved_cash`, `reserved_qty`, `reserved_shares`, `reserved_credits`), $\pm 10\%$ LULD circuit-breaker bands (`validate_luld_price`, `reanchor_luld_reference`), and `OhlcvBar` aggregation.
- **`wire.rs`:** `ServiceCreditPosition`, `ShortPositionRecord`, `CrimeOperationKind` (`HijackCargo`, `ExtortRetailer`, `CyberRansomware`, `CorporateWiretap`), `WireAttachment` (`B2BContractOffer`, `OtcBlockTradeOffer`, `SyndicateExtortionDemand`, `ServiceRetainerOffer`), `ClobDeltaEvent`, `ClientWireFrame`, and `ServerWireFrame`.
- **`production.rs`:** 5 industrial roles (`ResourceExtractor`, `Manufacturer`, `Retailer`, `LogisticsTrucking`, `PrivateSecurity`), `CloudTier` (`Offline`, `Tier1PosWire`, `Tier2L2Telemetry`, `Tier3ClefFirewall`), `ExecutivePersonaKind` archetypes, `AiDelegationConfig`, and `Corporation::working_capital_floor`.
- **`world.rs` & `vertical_slice.rs`:** `WorldState` aggregate implementing:
  - **Sovereign Monopoly Toll Gates & Guard #1 (80% Fiscal Recycling):** `10 bps` CLOB rake, `300 bps` IPO underwriting fee, `500 bps` laundering placement tax, `150%` forensic fines, and monthly `SovereignCloud` fees paid to `SovereignTreasury`, with 80% of month-end surplus above `sovereign_dr_reserve_buffer` (`$500,000 VC`) recycled into household wages and B2B procurement.
  - **Guard #2 (Physical Share-Borrow Short-Locate Vault, Dividend-in-Lieu & 115% Maintenance Margin):** `BorrowSharesToShort` physically transfers borrowed shares from `CapitalistsExecutives`, locks `150%` cash collateral, accrues daily `300 bps` borrow fees, debits monthly Payment-in-Lieu-of-Dividend, and triggers automatic CLOB Forced Buy-Ins below `115%` maintenance margin.
  - **Guard #3 (Oracle LULD Re-Anchoring):** `IngestOraclePriceTick` re-centers `luld_reference_price` on `DepositaryReceipt` books so $>10\%$ real-world gaps do not lock trading.
  - **Guard #4 (Non-Negative Balance Protection on `150%` Forensic Fines):** Caps immediate cash seizure at `IllicitCash + available_cash` and books any unpaid shortfall as a senior `TaxLienPayable` (`tax_liens`), garnished in `Sub-Phase 5`.
  - **Guard #5 (Dual-Clock `(tick, seq_no)` RNG & Webhook Idempotency):** `execute_command_sequenced` seeds `ChaCha8Rng` via `blake3(world_seed, current_tick, seq_no)`, and `processed_webhook_refs: BTreeSet<String>` deduplicates `AuthorityMintServiceCredit`.
  - **Guard #6 (Information-Barrier `build_authorized_world_view` & Bounded `wire_messages`):** Masks rival private `wire_messages`, rival `illicit_cash_balance`, and individual `ShortPositionRecord`s unless wiretapped, and prunes resolved `wire_messages` to `MAX_RESOLVED_WIRE_MESSAGES_PER_CORP = 64`.

## 3. Status
- **Status:** Complete & Verified (`7/7` unit and `proptest` suites passing, zero Clippy warnings).
