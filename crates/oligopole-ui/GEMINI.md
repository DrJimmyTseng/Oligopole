# Bounded Context: `oligopole-ui` — 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal

## 1. Domain Purpose & Scope
`oligopole-ui` renders the high-density, dark-mode dockable Bloomberg Terminal workspace at 120 FPS. It supports both **Local Single-Player Mode** (`launch_native_terminal` reading `Arc<WorldView>` from an in-process simulation thread) and **Remote Multiplayer Mode** (`launch_remote_terminal` connecting over WebSocket/`msgpack` to `oligopole-server`, applying immediate `ClobDeltaEvent`s and `100ms` coalesced `WorldViewUpdate` snapshots).

## 2. Core Modules
- **`theme.rs`:** High-contrast Bloomberg Amber/Cyan/Emerald/Dark-Matrix palette and tabular monospace styling.
- **`command_bar.rs`:** Keyboard-first `<TICKER> [MARKET] <MNEMONIC>` parser supporting `CMDTY`, `EQ`, and `SC` qualification across all 10 mnemonics (`MM`, `GP`, `OB`, `FA`, `OPS`, `EX`, `MSG`, `ECO`, `CAP`, `NEWS`).
- **`channel_link.rs`:** Color-coded pane linking (`Channel A`, `Channel B`, `Channel C`, `Unlinked`) synchronizing active instruments across docked panes.
- **`app.rs` & `panes/`:** `OligopoleTerminalApp` managing `egui_tiles::Tree` across 10 dockable panes (`F1–F10`):
  1. `MM` (`F1`): Market Monitor (Domestic Corporations, Private SMEs, Global Depositary Receipts `GOOGL-DR`/`META-DR`/`NVDA-DR`, and `PLEX SC`).
  2. `GP` (`F2`): OHLCV Price Chart with LULD reference band and aggregate short interest telemetry.
  3. `OB` (`F3`): Level-2 Central Limit Order Book depth & trade tape.
  4. `FA` (`F4`): GAAP Double-Entry Financial Statements (including `IllicitCash`, `TaxesPayable`, `TaxAndFineExpense`, `CloudServiceExpense`, `SecurityAndFreightExpense`, `DepreciationExpense`).
  5. `OPS` (`F5`): Supply-Chain WAC Inventory DAG, effective capacity (`facility_condition_bps`), credit rating, & Security/Logistics Shield status.
  6. `EX` (`F6`): Executive Ticket (`Maintenance & R&D Overhaul`, `Expand Capacity`, `SovereignCloud` tier selector, SME incorporation & IPO launch, Short-Locate borrow/cover, Syndicate Crime operations, and Retail Cash Laundering).
  7. `MSG` (`F7`): Instant Bloomberg Corporate Wire & 1-click Smart Attachments (`Accept` / `Reject`).
  8. `ECO` (`F8`): Macroeconomic Telemetry, Retail CPI Index, Trailing 30D GDP Velocity, Sector HHI Concentration, & Corporate Credit Ratings.
  9. `CAP` (`F9`): Shareholder Ownership & Cap Table Inspector (`owned_shares`, `reserved_shares`, `ownership_bps`).
  10. `NEWS` (`F10`): Live Bloomberg Financial News Wire (Quarterly Earnings, Credit Rating Actions, CapEx/R&D, & Dividends).

## 3. Status
- **Status:** Complete & Verified (`3/3` command bar, pane routing, and live `.oligopole` checkpoint/replay unit tests passing, zero Clippy warnings).
