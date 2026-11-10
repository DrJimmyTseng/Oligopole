# Bounded Context: `oligopole-ui` — 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal

## 1. Domain Purpose & Scope
`oligopole-ui` renders the high-density, dark-mode dockable Bloomberg Terminal workspace at 120 FPS. It reads immutable `Arc<WorldView>` snapshots directly from Rust memory via a frame-budgeted `tokio::sync::watch` channel (zero JSON/IPC serialization), sends `PlayerCommand` orders over an MPSC channel to the simulation thread, and integrates with `oligopole-store` for live `.oligopole` checkpointing and session resumption.

## 2. Core Modules
- **`theme.rs`:** High-contrast Bloomberg Amber/Cyan/Emerald/Dark-Matrix palette and tabular monospace styling.
- **`command_bar.rs`:** Keyboard-first `<TICKER> [MARKET] <MNEMONIC>` parser preserving `CMDTY` vs. `EQ` qualification (e.g., `IRON CMDTY GP`, `IRON EQ OB`, `ACME EQ FA`, `PLYR EQ OPS`).
- **`channel_link.rs`:** Color-coded pane linking (`Channel A`, `Channel B`, `Channel C`, `Unlinked`) synchronizing active instruments across docked panes.
- **`app.rs` & `panes/`:** `OligopoleTerminalApp` managing `egui_tiles::Tree` across Market Monitor (`MM`), OHLCV Price Charts (`GP`), Level-2 Order Book (`OB`), Financial Statements (`FA`), Supply-Chain WAC DAG (`OPS`), and Executive Command & AI Delegation Ticket (`EX`), with `route_to_pane` tab activation, global hotkeys (`/` or `Ctrl+K` command bar focus, `Space` Pause/Resume, `F1–F6` pane focus, `Ctrl+S` / `💾 SAVE` live `.oligopole` checkpoint), live `[AI: CLEF-FLASH RTX 5090 (BF16)]` / `[AI: SYSTEM-0 HEURISTIC (CPU)]` backend badge, `ExecutivePersonaKind` display, and `PLYR` Manual vs. AI Autopilot delegation enforcement.

## 3. Status
- **Status:** Complete & Verified (`3/3` command bar, pane routing, and live `.oligopole` checkpoint/replay unit tests passing, zero Clippy warnings).
