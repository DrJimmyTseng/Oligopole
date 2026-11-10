//! `oligopole-ui`: Native 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal UI for Oligopole.

pub mod app;
pub mod channel_link;
pub mod command_bar;
pub mod panes;
pub mod theme;

pub use app::{
    OligopoleTerminalApp, SPEED_FAST, SPEED_MAX_TURBO, SPEED_NORMAL, SPEED_PAUSED,
    SimulationThreadHandles, launch_native_terminal, spawn_simulation_thread,
};
pub use channel_link::{ChannelLinkRegistry, LinkChannel};
pub use command_bar::{ParsedTerminalCommand, TerminalMnemonic, parse_bloomberg_command};
pub use panes::TerminalPane;
