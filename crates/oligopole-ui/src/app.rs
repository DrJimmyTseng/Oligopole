use crate::channel_link::{ChannelLinkRegistry, LinkChannel};
use crate::command_bar::{TerminalMnemonic, parse_bloomberg_command};
use crate::panes::TerminalPane;
use crate::theme::{
    AMBER_PRIMARY, BG_PANEL_HEADER, CYAN_ACCENT, EMERALD_ACCENT, apply_bloomberg_theme,
};
use egui::{Context, RichText, TopBottomPanel, Ui, WidgetText};
use egui_tiles::{SimplificationOptions, TileId, Tiles, Tree, UiResponse};
use oligopole_ai::{
    ActorObservationView, AdaptiveDecisionPolicy, AiBackendStatus, DecisionPolicy,
    PersonaBiasVector,
};
use oligopole_core::{PlayerCommand, WorldState, WorldView};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

/// Simulation speed control shared between the 120 FPS UI and the dedicated Sim Thread.
/// `0 = Paused`, `1 = 10 TPS`, `2 = 60 TPS`, `3 = 1,000+ TPS Fast-Forward`.
pub const SPEED_PAUSED: u32 = 0;
pub const SPEED_NORMAL: u32 = 1;
pub const SPEED_FAST: u32 = 2;
pub const SPEED_MAX_TURBO: u32 = 3;

struct TerminalTreeBehavior<'a> {
    view: &'a WorldView,
    registry: &'a mut ChannelLinkRegistry,
    cmd_tx: &'a mpsc::UnboundedSender<PlayerCommand>,
}

impl egui_tiles::Behavior<TerminalPane> for TerminalTreeBehavior<'_> {
    fn tab_title_for_pane(&mut self, pane: &TerminalPane) -> WidgetText {
        pane.title(self.registry).into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        pane: &mut TerminalPane,
    ) -> UiResponse {
        pane.render(ui, self.view, self.registry, self.cmd_tx);
        UiResponse::None
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }
}

/// Native 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal application.
pub struct OligopoleTerminalApp {
    tree: Tree<TerminalPane>,
    registry: ChannelLinkRegistry,
    command_input: String,
    command_status: String,
    view_rx: watch::Receiver<Arc<WorldView>>,
    latest_view: Arc<WorldView>,
    ai_status_rx: watch::Receiver<AiBackendStatus>,
    latest_ai_status: AiBackendStatus,
    cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
    speed_mode: Arc<AtomicU32>,
    theme_initialized: bool,
}

impl std::fmt::Debug for OligopoleTerminalApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OligopoleTerminalApp")
            .field("command_input", &self.command_input)
            .finish_non_exhaustive()
    }
}

impl OligopoleTerminalApp {
    pub fn new(
        view_rx: watch::Receiver<Arc<WorldView>>,
        cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
        speed_mode: Arc<AtomicU32>,
        ai_status_rx: watch::Receiver<AiBackendStatus>,
    ) -> Self {
        let latest_view = view_rx.borrow().clone();
        let latest_ai_status = ai_status_rx.borrow().clone();
        let tree = Self::create_default_bloomberg_tree();
        Self {
            tree,
            registry: ChannelLinkRegistry::default(),
            command_input: String::from("PLYR EQ FA"),
            command_status: String::from("READY • Type `<TICKER> <MM|GP|OB|FA|OPS|EX>` or click Market Monitor"),
            view_rx,
            latest_view,
            ai_status_rx,
            latest_ai_status,
            cmd_tx,
            speed_mode,
            theme_initialized: false,
        }
    }

    fn create_default_bloomberg_tree() -> Tree<TerminalPane> {
        let mut tiles = Tiles::default();

        let p_monitor = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::MarketMonitor,
            LinkChannel::ChannelA,
            "PLYR",
        ));
        let p_chart = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::GraphPrice,
            LinkChannel::ChannelA,
            "IRON CMDTY",
        ));
        let p_book = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::OrderBook,
            LinkChannel::ChannelB,
            "IRON CMDTY",
        ));
        let p_fin = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::FinancialAnalysis,
            LinkChannel::ChannelA,
            "PLYR",
        ));
        let p_dag = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::OperationsDag,
            LinkChannel::ChannelA,
            "PLYR",
        ));
        let p_exec = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::ExecutiveTicket,
            LinkChannel::ChannelA,
            "PLYR",
        ));

        let top_row = tiles.insert_horizontal_tile(vec![p_monitor, p_chart, p_book]);
        let bottom_row = tiles.insert_horizontal_tile(vec![p_fin, p_dag, p_exec]);
        let root = tiles.insert_vertical_tile(vec![top_row, bottom_row]);

        Tree::new("oligopole_bloomberg_workspace", root, tiles)
    }

    fn execute_command_bar(&mut self) {
        if let Some(parsed) = parse_bloomberg_command(&self.command_input) {
            self.registry
                .set_ticker(LinkChannel::ChannelA, parsed.ticker.clone());
            self.command_status = format!(
                "ROUTED [CH-A] -> {} ({:?})",
                parsed.ticker, parsed.mnemonic
            );
        } else {
            self.command_status = format!("UNKNOWN MNEMONIC: '{}'", self.command_input);
        }
    }
}

impl eframe::App for OligopoleTerminalApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.theme_initialized {
            apply_bloomberg_theme(ctx);
            self.theme_initialized = true;
        }

        if self.view_rx.has_changed().unwrap_or(false) {
            self.latest_view = self.view_rx.borrow_and_update().clone();
        }
        if self.ai_status_rx.has_changed().unwrap_or(false) {
            self.latest_ai_status = self.ai_status_rx.borrow_and_update().clone();
        }

        // Top Bloomberg Command Bar & Macro Telemetry Ribbon
        TopBottomPanel::top("bloomberg_top_command_bar").show(ctx, |ui| {
            ui.visuals_mut().panel_fill = BG_PANEL_HEADER;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("OLIGOPOLE TERMINAL")
                        .color(AMBER_PRIMARY)
                        .strong(),
                );
                ui.separator();

                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.command_input)
                        .desired_width(180.0)
                        .hint_text("IRON CMDTY GP / ACME EQ FA"),
                );
                if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui.button(RichText::new("GO").color(AMBER_PRIMARY).strong()).clicked()
                {
                    self.execute_command_bar();
                }

                ui.separator();
                let current_speed = self.speed_mode.load(Ordering::Relaxed);
                for (mode, label) in [
                    (SPEED_PAUSED, "⏸ PAUSE"),
                    (SPEED_NORMAL, "▶ 10 TPS"),
                    (SPEED_FAST, "⏩ 60 TPS"),
                    (SPEED_MAX_TURBO, "⚡ 1000+ TPS"),
                ] {
                    if ui.selectable_label(current_speed == mode, label).clicked() {
                        self.speed_mode.store(mode, Ordering::Relaxed);
                    }
                }

                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} (Tick {})",
                        self.latest_view.date, self.latest_view.tick.0
                    ))
                    .color(CYAN_ACCENT)
                    .strong(),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "M1: {} | Bank Loans: {}",
                        self.latest_view.total_m1_money_supply, self.latest_view.total_bank_loans
                    ))
                    .color(EMERALD_ACCENT),
                );
                ui.separator();
                let ai_color = if self.latest_ai_status.is_clef_flash() {
                    EMERALD_ACCENT
                } else {
                    AMBER_PRIMARY
                };
                ui.label(
                    RichText::new(format!("[{}]", self.latest_ai_status.badge_label()))
                        .color(ai_color)
                        .strong(),
                );
            });
            ui.label(RichText::new(&self.command_status).small());
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut behavior = TerminalTreeBehavior {
                view: &self.latest_view,
                registry: &mut self.registry,
                cmd_tx: &self.cmd_tx,
            };
            self.tree.ui(&mut behavior, ui);
        });

        ctx.request_repaint_after(Duration::from_millis(8));
    }
}

/// Communication handles connecting `OligopoleTerminalApp` to the background simulation thread.
pub type SimulationThreadHandles = (
    watch::Receiver<Arc<WorldView>>,
    mpsc::UnboundedSender<PlayerCommand>,
    Arc<AtomicU32>,
    watch::Receiver<AiBackendStatus>,
);

/// Spawns the dedicated simulation thread communicating with `OligopoleTerminalApp` via a
/// frame-budgeted `watch<Arc<WorldView>>` (capped at 120 Hz during 1,000+ TPS fast-forward)
/// and an MPSC `PlayerCommand` channel.
pub fn spawn_simulation_thread(
    mut world: WorldState,
    clef_addr: Option<&str>,
) -> Result<SimulationThreadHandles, oligopole_core::CoreError> {
    let initial_view = Arc::new(world.build_world_view()?);
    let (view_tx, view_rx) = watch::channel(initial_view);
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<PlayerCommand>();
    let speed_mode = Arc::new(AtomicU32::new(SPEED_NORMAL));
    let speed_clone = Arc::clone(&speed_mode);
    let mut policy = AdaptiveDecisionPolicy::connect_or_fallback(clef_addr);
    let (ai_status_tx, ai_status_rx) = watch::channel(policy.status().clone());

    thread::spawn(move || {
        let frame_budget = Duration::from_micros(8_333); // 120 Hz snapshot cap
        let mut last_publish = Instant::now();
        let mut pending = Vec::new();

        loop {
            let mode = speed_clone.load(Ordering::Relaxed);
            if mode == SPEED_PAUSED {
                thread::sleep(Duration::from_millis(16));
                continue;
            }

            while let Ok(cmd) = cmd_rx.try_recv() {
                pending.push(cmd);
            }

            let next_tick_num = world.current_tick.0 + 1;
            if next_tick_num.is_multiple_of(30) {
                let corp_ids: Vec<_> = world.corporations.keys().collect();
                for cid in corp_ids {
                    let Some((persona_kind, accepts_ai)) = world
                        .corporations
                        .get(cid)
                        .map(|c| (c.persona_kind, c.delegation.accepts_slow_tick_ai()))
                    else {
                        continue;
                    };
                    if !accepts_ai {
                        continue;
                    }
                    let persona = PersonaBiasVector::from_archetype(persona_kind);
                    if let Ok(obs) = ActorObservationView::build(&world, cid) {
                        if let Ok((directive, _)) =
                            policy.evaluate_actor_slow_tick(world.world_seed, &obs, &persona)
                        {
                            pending.push(PlayerCommand::ApplyStrategicDirective(directive));
                        }
                    }
                }
                let _ = ai_status_tx.send(policy.status().clone());
            }

            if world.step_fast_tick(&pending).is_err() {
                break;
            }
            pending.clear();

            if last_publish.elapsed() >= frame_budget || mode != SPEED_MAX_TURBO {
                if let Ok(snapshot) = world.build_world_view() {
                    if view_tx.send(Arc::new(snapshot)).is_err() {
                        break;
                    }
                    last_publish = Instant::now();
                }
            }

            match mode {
                SPEED_NORMAL => thread::sleep(Duration::from_millis(100)),
                SPEED_FAST => thread::sleep(Duration::from_millis(16)),
                _ => {}
            }
        }
    });

    Ok((view_rx, cmd_tx, speed_mode, ai_status_rx))
}

/// Launches the native 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal window for `world`.
pub fn launch_native_terminal(
    world: WorldState,
    clef_addr: Option<&str>,
) -> Result<(), String> {
    let (view_rx, cmd_tx, speed_mode, ai_status_rx) =
        spawn_simulation_thread(world, clef_addr).map_err(|e| e.to_string())?;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OLIGOPOLE — Institutional Macroeconomic & Corporate Terminal")
            .with_inner_size([1560.0_f32, 920.0_f32]),
        ..Default::default()
    };

    eframe::run_native(
        "Oligopole Terminal",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(OligopoleTerminalApp::new(
                view_rx,
                cmd_tx,
                speed_mode,
                ai_status_rx,
            )))
        }),
    )
    .map_err(|e| e.to_string())
}
