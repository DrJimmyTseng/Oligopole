# Bounded Context: `oligopole-ai` — Candidate Generator, Persona Log-Odds & Clef-Flash IPC

## 1. Domain Purpose & Scope
`oligopole-ai` bridges the deterministic `oligopole-core` simulation with the Monthly Slow-Tick decision pipeline. It enforces asymmetric information via `ActorObservationView`, generates NPV-ranked `JointActionTuple` strategic candidates alongside orthogonal policy dials, queries either the local RTX 5090 `Clef-Flash` 9B sidecar over `127.0.0.1:19509` (`TCP_NODELAY` + 4-byte length-prefixed `msgpack`) or the deterministic `System0HeuristicPolicy`, modulates probabilities by each actor's `PersonaBiasVector` in `rust_decimal` log-odds space, and samples reproducibly with `ChaCha8Rng(world_seed, tick, actor_id)`.

## 2. Core Modules
- **`observation.rs`:** `ActorObservationView` (private ledger/inventory + share price, book value per share, `credit_rating`, `facility_condition_bps`, and lagged public competitor quotes).
- **`candidate.rs`:** `JointActionTuple` (composite CapEx, Financing, `MaintenanceAndRdOverhaul`, and `RepurchaseShares` options respecting working-capital floors) and discrete policy dials (`PricingStance`, `WageStance`, `DividendPolicy`).
- **`persona.rs`:** `PersonaBiasVector` (with `from_archetype(ExecutivePersonaKind)` mapping all 6 corporate leadership archetypes), log-odds softmax modulation in `rust_decimal`, Hamilton basis-point normalization, and seeded `ChaCha8Rng` sampling.
- **`protocol.rs` & `client.rs`:** Length-prefixed `msgpack` wire protocol (`noul`, `choice`, `score`) and advisory RTX 5090 `PingHandshake`.
- **`system0.rs`:** `AdaptiveDecisionPolicy` (live `Clef-Flash` TCP client with automatic graceful fallback to `System0HeuristicPolicy`), `AiBackendStatus` telemetry badge, and pure-Rust NPV & persona heuristic evaluator.

## 3. Invariants Verified by `proptest` & Integration Tests
1. `modulate_and_sample_choice` normalizes log-odds shifted weights via `allocate_hamilton` to exact basis points summing to `10_000` and samples reproducibly from `seeded_actor_rng`.
2. `ClefFlashTcpClient` and `AdaptiveDecisionPolicy` verify `TCP_NODELAY` 4-byte big-endian length-prefixed `msgpack` framing (`PingHandshake` with advisory RTX 5090 device check, `EvaluateSlowTickBatch`, and automatic graceful fallback to `System0HeuristicPolicy` when unreachable).

## 4. Status
- **Status:** Complete & Verified (`4/4` unit and `proptest` suites passing, zero Clippy warnings).
