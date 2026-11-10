# Bounded Context: `oligopole-ui` — 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal

## 1. Domain Purpose & Scope
`oligopole-ui` renders the high-density, dark-mode dockable Bloomberg Terminal workspace at 120 FPS. It reads immutable `Arc<WorldView>` snapshots directly from Rust memory via a frame-budgeted `tokio::sync::watch` channel (zero JSON/IPC serialization) and sends `PlayerCommand` orders over an MPSC channel to the simulation thread.

## 2. Core Modules
- **`theme.rs`:** High-contrast Bloomberg Amber/Cyan/Emerald/Dark-Matrix palette and tabular monospace styling.
- **`command_bar.rs`:** Keyboard-first `<TICKER> <MARKET> <MNEMONIC>` parser (e.g., `IRON CMDTY GP`, `ACME EQ FA`, `ACME EQ OB`, `PLAYER EQ OPS`) and `F1–F12` workspace hotkeys.
- **`channel_link.rs`:** Color-coded pane linking (`Channel A`, `Channel B`, `Channel C`, `Unlinked`) synchronizing active instruments across docked panes.
- **`app.rs` & `panes/`:** `OligopoleTerminalApp` managing `egui_tiles::Tree` across Market Monitor, OHLCV Price Charts (`GP`), Level-2 Order Book (`OB`), Financial Statements (`FA`), Supply-Chain WAC DAG (`OPS`), and Executive Command & AI Delegation Ticket.
