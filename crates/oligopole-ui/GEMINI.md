# Bounded Context: `oligopole-ui` — 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal

## 1. Domain Purpose & Scope
`oligopole-ui` renders the high-density, dark-mode dockable Bloomberg Terminal workspace at 120 FPS. It supports both **Local Single-Player Mode** (`launch_native_terminal` reading `Arc<WorldView>` from an in-process simulation thread) and **Remote Multiplayer Mode** (`launch_remote_terminal` connecting over WebSocket/`msgpack` to `oligopole-server`, applying immediate `ClobDeltaEvent`s and `100ms` coalesced `WorldViewUpdate` snapshots).

## 2. Core Modules
- **`theme.rs`:** High-contrast Bloomberg Amber/Cyan/Emerald/Dark-Matrix palette and tabular monospace styling.
- **`command_bar.rs`:** Keyboard-first `<TICKER> [MARKET] <MNEMONIC>` parser supporting `CMDTY`, `EQ`, and `SC` qualification across all 7 mnemonics (`MM`, `GP`, `OB`, `FA`, `OPS`, `EX`, `MSG`).
- **`channel_link.rs`:** Color-coded pane linking (`Channel A`, `Channel B`, `Channel C`, `Unlinked`) synchronizing active instruments across docked panes.
- **`app.rs` & `panes/`:** `OligopoleTerminalApp` managing `egui_tiles::Tree` across 7 dockable panes:
  1. `MM` (`F1`): Market Monitor (Domestic Corporations, Private SMEs, Global Depositary Receipts `GOOGL-DR`/`META-DR`/`NVDA-DR`, and `PLEX SC`).
  2. `GP` (`F2`): OHLCV Price Chart with LULD reference band and aggregate short interest telemetry.
  3. `OB` (`F3`): Level-2 Central Limit Order Book depth & trade tape.
  4. `FA` (`F4`): GAAP Double-Entry Financial Statements (including `IllicitCash`, `TaxesPayable`, `TaxAndFineExpense`, `CloudServiceExpense`, `SecurityAndFreightExpense`).
  5. `OPS` (`F5`): Supply-Chain WAC Inventory DAG & Security/Logistics Shield status.
  6. `EX` (`F6`): Executive Ticket (`SovereignCloud` tier selector, SME incorporation & IPO launch, Short-Locate borrow/cover, Syndicate Crime operations, and Retail Cash Laundering).
  7. `MSG` (`F7`): Instant Bloomberg Corporate Wire & 1-click Smart Attachments (`Accept` / `Reject`).

## 3. Status
- **Status:** Complete & Verified (`3/3` command bar, pane routing, and live `.oligopole` checkpoint/replay unit tests passing, zero Clippy warnings).
