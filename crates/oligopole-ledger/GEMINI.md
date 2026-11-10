# Bounded Context: `oligopole-ledger` — Closed-Loop `i128` Accounting & Endogenous Banking

## 1. Domain Purpose & Scope
`oligopole-ledger` is the pure financial accounting core of **Oligopole**. It has zero dependencies on other workspace crates and enforces strict closed-loop monetary conservation across all double-entry postings, pro-rata distributions, inventory cost accounting, and commercial bank credit facilities.

## 2. Core Aggregates, Value Objects & Domain Services

### Value Objects
- **`Money(i128)` (`src/money.rs`):**
  - Signed 128-bit integer count of minor currency units (cents).
  - Checked arithmetic (`checked_add`, `checked_sub`, `checked_mul_i128`, `checked_div_euclid`).
  - Sortable 16-byte big-endian SQLite encoding (`to_sortable_be_bytes` / `from_sortable_be_bytes`) using sign-bit flip `self.0.cast_unsigned() ^ (1u128 << 127)`.
- **`RoundingReserve` (`src/rounding.rs`):**
  - Deterministic sub-cent numerator accumulator for 30/360 daily basis-point interest and tax accruals (`denominator = 10_000 * 360 = 3_600_000`).
- **`WacInventoryLot` (`src/inventory_wac.rs`):**
  - $O(1)$ Weighted Average Cost SKU tracker `(total_qty: u64, total_book_value: Money, reserved_qty: u64)`.
  - Uses widening 256-bit `u256_mul_div_rem` to compute COGS when drawing $q < Q$ units, and sweeps exact remaining `total_book_value` when $q == Q$.
  - Maintains `reserved_qty` for open CLOB Ask orders so legal ownership and WAC cost basis remain undisturbed until a trade fills.

### Domain Services & Aggregates
- **`u256_mul_div_rem` (`src/u256_math.rs`):**
  - Safe widening 256-bit multiply-divide `(a: u128 * b: u64) / d: u128 -> (quotient: u128, remainder: u128)` preventing `i128` overflow on arbitrary `proptest` inputs.
- **`allocate_hamilton` (`src/rounding.rs`):**
  - Largest Remainder pro-rata splitter over `(Id, u64)` weights sorted deterministically by `(Remainder DESC, Id ASC)`. Guarantees $\sum \text{shares}_i == \text{total}$.
- **`LedgerEngine` & `JournalEntry` (`src/account.rs`, `src/journal.rs`):**
  - Manages entity Charts of Accounts (`Asset`, `Liability`, `Equity`, `Revenue`, `Expense`) and `reserved_cash` escrow counters.
  - Enforces $\sum \text{Debits} == \sum \text{Credits}$ on every `JournalEntry`, rejects negative cash/deposit balances on non-overdraft asset accounts, and audits global trial balance conservation.
- **`CommercialBankFacility` (`src/bank.rs`):**
  - Endogenous loan origination (`Dr LoanReceivable / Cr CustomerDepositsLiability` on Bank books; `Dr CashAndDeposits / Cr LoansPayable` on Borrower books), principal amortization, 30/360 interest collection, and insolvency liquidation waterfall + equity write-down on default.
- **`FinancialStatementReport` (`src/statement.rs`):**
  - Generates point-in-time `BalanceSheet` and period `IncomeStatement` rollups per entity.

## 3. Invariants Verified by `proptest`
1. `Money::from_sortable_be_bytes(m.to_sortable_be_bytes()) == m` and byte-lexicographical order matches `Ord` for all `a, b: i128`.
2. `allocate_hamilton(total, weights)` sums to `total` for all non-negative `total: Money` and non-empty `weights: &[(Id, u64)]` with `sum(weights) > 0`.
3. `WacInventoryLot` sequence of additions and partial/full draws never leaves negative book value and ends at `total_book_value == Money::ZERO` whenever `total_qty == 0`.
4. `LedgerEngine::audit_trial_balance()` holds before and after any sequence of valid journal postings, endogenous loan originations, repayments, and bankruptcy liquidations.

## 4. Status
- **Status:** Complete & Verified (`12/12` unit and `proptest` suites passing, zero Clippy warnings).
