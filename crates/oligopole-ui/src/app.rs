use crate::channel_link::{ChannelLinkRegistry, LinkChannel};
use crate::command_bar::{TerminalMnemonic, parse_bloomberg_command};
use crate::panes::TerminalPane;
use crate::theme::{
    AMBER_PRIMARY, BG_PANEL_HEADER, CYAN_ACCENT, EMERALD_ACCENT, apply_bloomberg_theme,
};
use egui::{Context, RichText, TopBottomPanel, Ui, WidgetText};
use egui_tiles::{SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse};
use futures_util::{SinkExt, StreamExt};
use oligopole_ai::{
    ActorObservationView, AdaptiveDecisionPolicy, AiBackendStatus, DecisionPolicy,
    PersonaBiasVector,
};
use oligopole_core::{
    ClientWireFrame, PlayerCommand, ServerWireFrame, WorldState, WorldView, build_vertical_slice_0,
};
use oligopole_store::{DecisionOutboxEntry, DecisionSourceKind, OligopoleStore, to_hex};
use std::path::Path;
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
    save_status_rx: watch::Receiver<String>,
    latest_save_status: String,
    cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
    save_tx: mpsc::UnboundedSender<()>,
    speed_mode: Arc<AtomicU32>,
    last_non_paused_speed: u32,
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
        save_tx: mpsc::UnboundedSender<()>,
        speed_mode: Arc<AtomicU32>,
        ai_status_rx: watch::Receiver<AiBackendStatus>,
        save_status_rx: watch::Receiver<String>,
    ) -> Self {
        let latest_view = view_rx.borrow().clone();
        let latest_ai_status = ai_status_rx.borrow().clone();
        let latest_save_status = save_status_rx.borrow().clone();
        let tree = Self::create_default_bloomberg_tree();
        Self {
            tree,
            registry: ChannelLinkRegistry::default(),
            command_input: String::from("PLYR EQ FA"),
            command_status: String::from(
                "READY • Keys: `/` or `Ctrl+K` Command Bar | `Space` Pause | `F1-F10` Panes (MM/GP/OB/FA/OPS/EX/MSG/ECO/CAP/NEWS) | `Ctrl+S` Save",
            ),
            view_rx,
            latest_view,
            ai_status_rx,
            latest_ai_status,
            save_status_rx,
            latest_save_status,
            cmd_tx,
            save_tx,
            speed_mode,
            last_non_paused_speed: SPEED_NORMAL,
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
        let p_wire = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::CorporateWire,
            LinkChannel::ChannelA,
            "PLYR",
        ));
        let p_eco = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::MacroEconomy,
            LinkChannel::ChannelA,
            "PLYR",
        ));
        let p_cap = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::CapTable,
            LinkChannel::ChannelA,
            "PLYR",
        ));
        let p_news = tiles.insert_pane(TerminalPane::new(
            TerminalMnemonic::NewsWire,
            LinkChannel::ChannelA,
            "PLYR",
        ));

        let top_left = tiles.insert_tab_tile(vec![p_monitor, p_eco]);
        let top_row = tiles.insert_horizontal_tile(vec![top_left, p_chart, p_book]);
        let bot_left = tiles.insert_tab_tile(vec![p_fin, p_cap]);
        let bot_mid = tiles.insert_tab_tile(vec![p_dag, p_news]);
        let bottom_row = tiles.insert_horizontal_tile(vec![bot_left, bot_mid, p_exec, p_wire]);
        let root = tiles.insert_vertical_tile(vec![top_row, bottom_row]);

        Tree::new("oligopole_bloomberg_workspace", root, tiles)
    }

    /// Routes a parsed `<TICKER> [MARKET] <MNEMONIC>` command to the target pane in `self.tree`,
    /// synchronizes its linked channel (`CH-A`, `CH-B`, `CH-C`, or `local_ticker`), and activates its tab.
    pub fn execute_command_bar(&mut self) {
        if let Some(parsed) = parse_bloomberg_command(&self.command_input) {
            self.route_to_pane(parsed.mnemonic, Some(&parsed.ticker));
        } else {
            self.command_status = format!("UNKNOWN MNEMONIC: '{}'", self.command_input);
        }
    }

    /// Focuses the pane matching `target_mnemonic` (and optionally updates its active ticker and channel).
    pub fn route_to_pane(&mut self, target_mnemonic: TerminalMnemonic, new_ticker: Option<&str>) {
        let mut matched_channel = LinkChannel::ChannelA;
        let mut matched_tile_id = None;

        for (tile_id, tile) in self.tree.tiles.iter_mut() {
            if let Tile::Pane(pane) = tile {
                if pane.mnemonic == target_mnemonic {
                    matched_channel = pane.channel;
                    matched_tile_id = Some(*tile_id);
                    if let Some(t) = new_ticker {
                        t.clone_into(&mut pane.local_ticker);
                    }
                    break;
                }
            }
        }

        if let Some(t) = new_ticker {
            if matched_channel == LinkChannel::Unlinked {
                self.registry.set_ticker(LinkChannel::ChannelA, t.to_owned());
            } else {
                self.registry.set_ticker(matched_channel, t.to_owned());
            }
        }

        if let Some(target_id) = matched_tile_id {
            let _ = self.tree.make_active(|id, _| id == target_id);
        }

        let active_ticker = new_ticker.unwrap_or_else(|| {
            self.registry
                .active_ticker_ref(matched_channel)
                .unwrap_or("PLYR")
        });
        self.command_status = format!(
            "ROUTED [{}] -> {} ({})",
            matched_channel.badge_label(),
            active_ticker,
            target_mnemonic.short_code()
        );
    }

    fn handle_global_hotkeys(&mut self, ctx: &Context, cmd_bar_id: egui::Id) {
        let cmd_bar_focused = ctx.memory(|m| m.has_focus(cmd_bar_id));
        let any_text_focused = ctx.wants_keyboard_input();

        // `Ctrl+K` always focuses the Bloomberg Command Bar; `/` focuses it when not already typing
        let focus_cmd_bar = ctx.input(|i| {
            (i.modifiers.command && i.key_pressed(egui::Key::K))
                || (!any_text_focused && i.key_pressed(egui::Key::Slash))
        });
        if focus_cmd_bar {
            ctx.memory_mut(|m| m.request_focus(cmd_bar_id));
            return;
        }

        // `Ctrl+S` triggers an immediate `.oligopole` checkpoint save
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            let _ = self.save_tx.send(());
        }

        if cmd_bar_focused || any_text_focused {
            return;
        }

        // `Space` toggles Pause / Resume
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            let current = self.speed_mode.load(Ordering::Relaxed);
            if current == SPEED_PAUSED {
                let resume = if self.last_non_paused_speed == SPEED_PAUSED {
                    SPEED_NORMAL
                } else {
                    self.last_non_paused_speed
                };
                self.speed_mode.store(resume, Ordering::Relaxed);
                self.command_status = String::from("SIMULATION RESUMED (Space)");
            } else {
                self.last_non_paused_speed = current;
                self.speed_mode.store(SPEED_PAUSED, Ordering::Relaxed);
                self.command_status = String::from("SIMULATION PAUSED (Space)");
            }
        }

        // `F1–F10` institutional function keys route directly to the 10 Bloomberg panes
        for (key, mnemonic) in [
            (egui::Key::F1, TerminalMnemonic::MarketMonitor),
            (egui::Key::F2, TerminalMnemonic::GraphPrice),
            (egui::Key::F3, TerminalMnemonic::OrderBook),
            (egui::Key::F4, TerminalMnemonic::FinancialAnalysis),
            (egui::Key::F5, TerminalMnemonic::OperationsDag),
            (egui::Key::F6, TerminalMnemonic::ExecutiveTicket),
            (egui::Key::F7, TerminalMnemonic::CorporateWire),
            (egui::Key::F8, TerminalMnemonic::MacroEconomy),
            (egui::Key::F9, TerminalMnemonic::CapTable),
            (egui::Key::F10, TerminalMnemonic::NewsWire),
        ] {
            if ctx.input(|i| i.key_pressed(key)) {
                self.route_to_pane(mnemonic, None);
            }
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
        if self.save_status_rx.has_changed().unwrap_or(false) {
            self.latest_save_status
                .clone_from(&self.save_status_rx.borrow_and_update());
        }

        let cmd_bar_id = egui::Id::new("bloomberg_command_input_box");
        self.handle_global_hotkeys(ctx, cmd_bar_id);

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
                        .id(cmd_bar_id)
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
                        if mode != SPEED_PAUSED {
                            self.last_non_paused_speed = mode;
                        }
                        self.speed_mode.store(mode, Ordering::Relaxed);
                    }
                }

                ui.separator();
                if ui
                    .button(RichText::new("💾 SAVE").color(CYAN_ACCENT).strong())
                    .on_hover_text("Save .oligopole checkpoint now (Ctrl+S)")
                    .clicked()
                {
                    let _ = self.save_tx.send(());
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
                        "M1: {} | Loans: {} | CPI: {:.2} | 30D GDP: {} | Sov Reserve: {} | SC: {}",
                        self.latest_view.total_m1_money_supply,
                        self.latest_view.total_bank_loans,
                        f64::from(self.latest_view.macro_telemetry.retail_cpi_index_bps) / 100.0,
                        self.latest_view.macro_telemetry.trailing_30d_gdp,
                        self.latest_view.sovereign_cash_balance,
                        self.latest_view.total_service_credits_supply
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
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.command_status).small());
                ui.separator();
                ui.label(
                    RichText::new(&self.latest_save_status)
                        .color(EMERALD_ACCENT)
                        .small(),
                );
            });
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
    mpsc::UnboundedSender<()>,
    Arc<AtomicU32>,
    watch::Receiver<AiBackendStatus>,
    watch::Receiver<String>,
);

/// Spawns the dedicated simulation thread communicating with `OligopoleTerminalApp` via a
/// frame-budgeted `watch<Arc<WorldView>>` (capped at 120 Hz during 1,000+ TPS fast-forward),
/// an MPSC `PlayerCommand` channel, and an optional `.oligopole` SQLite WAL store.
pub fn spawn_simulation_thread(
    mut world: WorldState,
    clef_addr: Option<&str>,
    save_path: Option<&str>,
) -> Result<SimulationThreadHandles, oligopole_core::CoreError> {
    let mut store = if let Some(p) = save_path {
        if Path::new(p).exists() {
            if let Ok(existing) = OligopoleStore::open_file(p) {
                if let Ok((loaded_world, _)) = existing.load_latest_snapshot() {
                    world = loaded_world;
                }
                existing
            } else {
                OligopoleStore::open_in_memory().map_err(|_| {
                    oligopole_core::CoreError::InvalidOrder {
                        reason: "failed to open in-memory store".to_owned(),
                    }
                })?
            }
        } else {
            let mut created = OligopoleStore::open_file(p).map_err(|_| {
                oligopole_core::CoreError::InvalidOrder {
                    reason: "failed to create .oligopole save file".to_owned(),
                }
            })?;
            let _ = created.save_checkpoint(&world, &[]);
            created
        }
    } else {
        let mut mem = OligopoleStore::open_in_memory().map_err(|_| {
            oligopole_core::CoreError::InvalidOrder {
                reason: "failed to open in-memory store".to_owned(),
            }
        })?;
        let _ = mem.save_checkpoint(&world, &[]);
        mem
    };

    let initial_save_label = match save_path {
        Some(p) => format!("STORE: {p} (Tick {})", world.current_tick.0),
        None => format!("STORE: :memory: (Tick {})", world.current_tick.0),
    };

    let initial_view = Arc::new(world.build_world_view()?);
    let (view_tx, view_rx) = watch::channel(initial_view);
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<PlayerCommand>();
    let (save_tx, mut save_rx) = mpsc::unbounded_channel::<()>();
    let speed_mode = Arc::new(AtomicU32::new(SPEED_NORMAL));
    let speed_clone = Arc::clone(&speed_mode);
    let mut policy = AdaptiveDecisionPolicy::connect_or_fallback(clef_addr);
    let (ai_status_tx, ai_status_rx) = watch::channel(policy.status().clone());
    let (save_status_tx, save_status_rx) = watch::channel(initial_save_label);
    let save_target_name = save_path.unwrap_or(":memory:").to_owned();

    thread::spawn(move || {
        let frame_budget = Duration::from_micros(8_333); // 120 Hz snapshot cap
        let mut last_publish = Instant::now();
        let mut pending = Vec::new();
        let mut pending_outbox = Vec::new();

        loop {
            while save_rx.try_recv().is_ok() {
                if let Ok(hash) = store.save_checkpoint(&world, &pending_outbox) {
                    pending_outbox.clear();
                    let hex_full = to_hex(&hash);
                    let short_hash = hex_full.get(..12).unwrap_or(&hex_full);
                    let _ = save_status_tx.send(format!(
                        "SAVED {save_target_name} @ T{} (blake3: {short_hash}…)",
                        world.current_tick.0
                    ));
                }
            }

            let mode = speed_clone.load(Ordering::Relaxed);
            if mode == SPEED_PAUSED {
                thread::sleep(Duration::from_millis(16));
                continue;
            }

            let next_tick_num = world.current_tick.0 + 1;
            let mut seq_no = 0u32;

            while let Ok(cmd) = cmd_rx.try_recv() {
                pending_outbox.push(DecisionOutboxEntry {
                    tick: next_tick_num,
                    seq_no,
                    source_kind: DecisionSourceKind::PlayerManual,
                    command: cmd.clone(),
                    probs_blake3_hash: [0u8; 32],
                });
                seq_no = seq_no.saturating_add(1);
                pending.push(cmd);
            }

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
                        if let Ok((directive, probs_hash)) =
                            policy.evaluate_actor_slow_tick(world.world_seed, &obs, &persona)
                        {
                            let source_kind = if policy.status().is_clef_flash() {
                                DecisionSourceKind::ClefFlashSlowTick
                            } else {
                                DecisionSourceKind::System0Heuristic
                            };
                            let cmd = PlayerCommand::ApplyStrategicDirective(directive);
                            pending_outbox.push(DecisionOutboxEntry {
                                tick: next_tick_num,
                                seq_no,
                                source_kind,
                                command: cmd.clone(),
                                probs_blake3_hash: probs_hash,
                            });
                            seq_no = seq_no.saturating_add(1);
                            pending.push(cmd);
                        }
                    }
                }
                let _ = ai_status_tx.send(policy.status().clone());
            }

            if world.step_fast_tick(&pending).is_err() {
                break;
            }
            pending.clear();

            // Auto-checkpoint at the end of every simulated year (every 360 ticks)
            if world.current_tick.0.is_multiple_of(360) {
                if let Ok(hash) = store.save_checkpoint(&world, &pending_outbox) {
                    pending_outbox.clear();
                    let hex_full = to_hex(&hash);
                    let short_hash = hex_full.get(..12).unwrap_or(&hex_full);
                    let _ = save_status_tx.send(format!(
                        "AUTO-SAVED {save_target_name} @ T{} (blake3: {short_hash}…)",
                        world.current_tick.0
                    ));
                }
            }

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

    Ok((
        view_rx,
        cmd_tx,
        save_tx,
        speed_mode,
        ai_status_rx,
        save_status_rx,
    ))
}

/// Spawns a background Tokio WebSocket client thread connecting `OligopoleTerminalApp` to a remote
/// authoritative `oligopole-server` instance (`ws://...`).
///
/// Applies incoming `ServerWireFrame::ClobDelta` frames immediately to the cached `WorldView`
/// and replaces the full snapshot on `100ms` (10 Hz) coalesced `ServerWireFrame::WorldViewUpdate` frames.
pub fn spawn_remote_websocket_client(
    ws_url: &str,
    session_token: &str,
) -> Result<SimulationThreadHandles, String> {
    let (dummy_world, _) = build_vertical_slice_0([0u8; 32]).map_err(|e| e.to_string())?;
    let initial_view = Arc::new(dummy_world.build_world_view().map_err(|e| e.to_string())?);
    let (view_tx, view_rx) = watch::channel(initial_view.clone());
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<PlayerCommand>();
    let (save_tx, mut save_rx) = mpsc::unbounded_channel::<()>();
    let speed_mode = Arc::new(AtomicU32::new(SPEED_NORMAL));
    let (ai_status_tx, ai_status_rx) = watch::channel(AiBackendStatus::System0Heuristic {
        reason: Some(format!("REMOTE SERVER: {ws_url}")),
    });
    let (save_status_tx, save_status_rx) =
        watch::channel(format!("CONNECTING TO {ws_url} (Token: {session_token})…"));

    let url_owned = ws_url.to_owned();
    let token_owned = session_token.to_owned();

    thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let Ok((ws_stream, _)) = tokio_tungstenite::connect_async(&url_owned).await else {
                let _ = save_status_tx.send(format!("CONNECTION FAILED: {url_owned}"));
                return;
            };
            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            // Send Authenticate frame immediately
            if let Ok(auth_bytes) = rmp_serde::to_vec(&ClientWireFrame::Authenticate {
                token: token_owned.clone(),
            }) {
                let _ = ws_tx
                    .send(tokio_tungstenite::tungstenite::Message::Binary(
                        auth_bytes.into(),
                    ))
                    .await;
            }

            let mut cached_view: WorldView = (*initial_view).clone();

            loop {
                tokio::select! {
                    Some(cmd) = cmd_rx.recv() => {
                        if let Ok(buf) = rmp_serde::to_vec(&ClientWireFrame::SubmitCommand(cmd)) {
                            if ws_tx.send(tokio_tungstenite::tungstenite::Message::Binary(buf.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(()) = save_rx.recv() => {
                        if let Ok(buf) = rmp_serde::to_vec(&ClientWireFrame::RequestCheckpointSave) {
                            let _ = ws_tx.send(tokio_tungstenite::tungstenite::Message::Binary(buf.into())).await;
                        }
                    }
                    maybe_msg = ws_rx.next() => {
                        let Some(Ok(msg)) = maybe_msg else {
                            let _ = save_status_tx.send(format!("DISCONNECTED FROM {url_owned}"));
                            break;
                        };
                        if let tokio_tungstenite::tungstenite::Message::Binary(bytes) = msg {
                            let Ok(frame) = rmp_serde::from_slice::<ServerWireFrame>(&bytes) else {
                                continue;
                            };
                            match frame {
                                ServerWireFrame::Authenticated { server_motd, .. } => {
                                    let _ = save_status_tx.send(format!("ONLINE: {server_motd}"));
                                    let _ = ai_status_tx.send(AiBackendStatus::ClefFlashConnected {
                                        addr: url_owned.clone(),
                                        device_name: "Authoritative Multiplayer Server".to_owned(),
                                        dtype: "Dual-Clock".to_owned(),
                                    });
                                }
                                ServerWireFrame::ClobDelta(delta) => {
                                    if let Some(inst) = cached_view
                                        .instruments
                                        .iter_mut()
                                        .find(|i| i.instrument_id == delta.instrument_id)
                                    {
                                        inst.last_price = delta.last_price;
                                        inst.best_bid = delta.best_bid;
                                        inst.best_ask = delta.best_ask;
                                        inst.top_bids = delta.top_bids;
                                        inst.top_asks = delta.top_asks;
                                        inst.recent_fills = delta.recent_fills;
                                    }
                                    let _ = view_tx.send(Arc::new(cached_view.clone()));
                                }
                                ServerWireFrame::WorldViewUpdate(new_view) => {
                                    cached_view = *new_view;
                                    let _ = view_tx.send(Arc::new(cached_view.clone()));
                                }
                                ServerWireFrame::CommandRejected { reason } => {
                                    let _ = save_status_tx.send(format!("REJECTED: {reason}"));
                                }
                                ServerWireFrame::CheckpointSaved { tick, blake3_hex } => {
                                    let short = blake3_hex.get(..12).unwrap_or(&blake3_hex);
                                    let _ = save_status_tx.send(format!("SERVER SAVED @ T{tick} (blake3: {short}…)"));
                                }
                                ServerWireFrame::Pong { .. } => {}
                            }
                        }
                    }
                }
            }
        });
    });

    Ok((
        view_rx,
        cmd_tx,
        save_tx,
        speed_mode,
        ai_status_rx,
        save_status_rx,
    ))
}

/// Launches the native 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal connected over WebSocket
/// to a remote authoritative `oligopole-server`.
pub fn launch_remote_terminal(ws_url: &str, session_token: &str) -> Result<(), String> {
    let (view_rx, cmd_tx, save_tx, speed_mode, ai_status_rx, save_status_rx) =
        spawn_remote_websocket_client(ws_url, session_token)?;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("BLACK TAPE TERMINAL — Connected to {ws_url}"))
            .with_inner_size([1560.0_f32, 920.0_f32]),
        ..Default::default()
    };

    eframe::run_native(
        "Black Tape Multiplayer Terminal",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(OligopoleTerminalApp::new(
                view_rx,
                cmd_tx,
                save_tx,
                speed_mode,
                ai_status_rx,
                save_status_rx,
            )))
        }),
    )
    .map_err(|e| e.to_string())
}

/// Launches the native 120 FPS `eframe` / `egui_tiles` Bloomberg Terminal window for `world`.
pub fn launch_native_terminal(
    world: WorldState,
    clef_addr: Option<&str>,
    save_path: Option<&str>,
) -> Result<(), String> {
    let (view_rx, cmd_tx, save_tx, speed_mode, ai_status_rx, save_status_rx) =
        spawn_simulation_thread(world, clef_addr, save_path).map_err(|e| e.to_string())?;

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
                save_tx,
                speed_mode,
                ai_status_rx,
                save_status_rx,
            )))
        }),
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oligopole_core::build_vertical_slice_0;

    #[test]
    fn test_command_bar_routes_to_target_pane_and_updates_linked_channel() {
        let (world, _) = build_vertical_slice_0([42u8; 32]).unwrap();
        let initial_view = Arc::new(world.build_world_view().unwrap());
        let (_view_tx, view_rx) = watch::channel(initial_view);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let (save_tx, _save_rx) = mpsc::unbounded_channel();
        let speed_mode = Arc::new(AtomicU32::new(SPEED_PAUSED));
        let (_ai_tx, ai_rx) = watch::channel(AiBackendStatus::System0Heuristic { reason: None });
        let (_save_st_tx, save_st_rx) = watch::channel(String::from("STORE: :memory:"));

        let mut app = OligopoleTerminalApp::new(
            view_rx,
            cmd_tx,
            save_tx,
            speed_mode,
            ai_rx,
            save_st_rx,
        );

        // 1. Route to OrderBook (`OB`, linked to `CH-B` by default) with `VALE EQ OB`
        app.command_input = "VALE EQ OB".to_owned();
        app.execute_command_bar();
        assert_eq!(app.registry.ticker_for(LinkChannel::ChannelB, ""), "VALE EQ");
        assert!(app.command_status.contains("ROUTED [CH-B] -> VALE EQ (OB)"));

        // 2. Route to FinancialAnalysis (`FA`, linked to `CH-A` by default) with `ACME EQ FA`
        app.command_input = "ACME EQ FA".to_owned();
        app.execute_command_bar();
        assert_eq!(app.registry.ticker_for(LinkChannel::ChannelA, ""), "ACME");
        assert!(app.command_status.contains("ROUTED [CH-A] -> ACME (FA)"));

        // 3. Direct F-key routing (`route_to_pane`) preserves channel ticker
        app.route_to_pane(TerminalMnemonic::ExecutiveTicket, None);
        assert!(app.command_status.contains("ROUTED [CH-A] -> ACME (EX)"));

        // 4. Route to Milestone 9 mnemonics (`ECO`, `CAP`, `NEWS`)
        app.command_input = "ECO".to_owned();
        app.execute_command_bar();
        assert!(app.command_status.contains("ROUTED [CH-A] -> PLYR (ECO)"));

        app.command_input = "ZENI CAP".to_owned();
        app.execute_command_bar();
        assert!(app.command_status.contains("ROUTED [CH-A] -> ZENI (CAP)"));

        app.command_input = "NEWS".to_owned();
        app.execute_command_bar();
        assert!(app.command_status.contains("ROUTED [CH-A] -> PLYR (NEWS)"));
    }

    #[test]
    fn test_spawn_simulation_thread_live_checkpoint_and_replay() {
        let seed = [33u8; 32];
        let (world, _) = build_vertical_slice_0(seed).unwrap();
        let temp_path = std::env::temp_dir().join("oligopole_ui_test_session.oligopole");
        let _ = std::fs::remove_file(&temp_path);
        let path_str = temp_path.to_str().unwrap();

        {
            let (_view_rx, _cmd_tx, save_tx, speed_mode, _ai_rx, mut save_status_rx) =
                spawn_simulation_thread(world, None, Some(path_str)).unwrap();

            // Let simulation advance a couple ticks, then pause and request a live checkpoint
            thread::sleep(Duration::from_millis(150));
            speed_mode.store(SPEED_PAUSED, Ordering::Relaxed);
            save_tx.send(()).unwrap();

            // Wait briefly for the save status notification
            for _ in 0..20 {
                if save_status_rx.borrow_and_update().starts_with("SAVED") {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
            assert!(save_status_rx.borrow().starts_with("SAVED"));
        }

        // Verify the saved `.oligopole` container replays bit-for-bit from Genesis
        let reopened = OligopoleStore::open_file(&temp_path).unwrap();
        let (genesis, _) = build_vertical_slice_0(seed).unwrap();
        let verified = reopened.verify_deterministic_replay(genesis).unwrap();
        assert!(verified >= 1);
        let _ = std::fs::remove_file(&temp_path);
    }
}
