use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use oligopole_ai::{
    ActorObservationView, AdaptiveDecisionPolicy, AiBackendStatus, AiError, DecisionPolicy,
    PersonaBiasVector,
};
use oligopole_core::{
    ClientWireFrame, ClobDeltaEvent, CoreError, CorpId, PlayerCommand, ServerWireFrame, WorldState,
    build_vertical_slice_0,
};
use oligopole_ledger::Money;
use oligopole_store::{
    DecisionOutboxEntry, DecisionSourceKind, OligopoleStore, StoreError, compute_world_blake3_hash,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast, oneshot},
};

/// Errors returned by the authoritative `oligopole-server` multiplayer engine.
#[derive(Debug, Error)]
pub enum ServerError {
    #[error("core simulation error: {0}")]
    Core(#[from] CoreError),
    #[error("AI decision error: {0}")]
    Ai(#[from] AiError),
    #[error("persistence store error: {0}")]
    Store(#[from] StoreError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("msgpack serialization error: {0}")]
    MsgpackEncode(#[from] rmp_serde::encode::Error),
    #[error("msgpack deserialization error: {0}")]
    MsgpackDecode(#[from] rmp_serde::decode::Error),
}

/// Configuration for the authoritative **Black Tape** multiplayer server (`oligopole-server`).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub save_path: Option<PathBuf>,
    pub tick_ms: u64,
    pub clef_flash_addr: Option<SocketAddr>,
    pub webhook_secret: String,
    pub world_seed: [u8; 32],
    /// Maps bearer session tokens to corporate tickers (e.g., `"PLYR-TOKEN" -> "PLYR"`, `"ADMIN-TOKEN" -> "*"`).
    pub session_tokens: BTreeMap<String, String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let mut session_tokens = BTreeMap::new();
        session_tokens.insert("ADMIN".to_owned(), "*".to_owned());
        session_tokens.insert("PLYR-TOKEN".to_owned(), "PLYR".to_owned());
        session_tokens.insert("APEX-TOKEN".to_owned(), "APEX".to_owned());
        session_tokens.insert("ACME-TOKEN".to_owned(), "ACME".to_owned());
        session_tokens.insert("ZENI-TOKEN".to_owned(), "ZENI".to_owned());
        session_tokens.insert("IRON-TOKEN".to_owned(), "IRON".to_owned());
        session_tokens.insert("VALE-TOKEN".to_owned(), "VALE".to_owned());

        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 19510)),
            save_path: None,
            tick_ms: 15_000,
            clef_flash_addr: None,
            webhook_secret: "black-tape-dev-secret".to_owned(),
            world_seed: [42u8; 32],
            session_tokens,
        }
    }
}

/// JSON payload for `POST /api/v1/webhook/service-credit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCreditWebhookRequest {
    pub recipient_ticker: String,
    pub credits: u64,
    pub webhook_ref: String,
}

/// JSON payload for `POST /api/v1/oracle/tick`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleTickWebhookRequest {
    pub ticker: String,
    pub new_price_cents: i128,
}

/// JSON response returned by webhook endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookAckResponse {
    pub ok: bool,
    pub tick: u64,
    pub seq_no: u32,
    pub message: String,
}

struct AuthoritativeEngine {
    world: WorldState,
    store: Option<OligopoleStore>,
    policy: AdaptiveDecisionPolicy,
    pending_outbox: Vec<DecisionOutboxEntry>,
    next_intraday_seq_no: u32,
    dirty_for_coalesced_view: bool,
}

enum ResolvedSessionRole {
    AdminObserver,
    Corporation(CorpId),
}

impl AuthoritativeEngine {
    fn new(config: &ServerConfig) -> Result<Self, ServerError> {
        let mut store = match &config.save_path {
            Some(path) => Some(OligopoleStore::open_file(Path::new(path))?),
            None => None,
        };

        let world = if let Some(st) = &mut store {
            if let Ok((loaded_world, _)) = st.load_latest_snapshot() {
                loaded_world
            } else {
                let (w, _) = build_vertical_slice_0(config.world_seed)?;
                st.save_checkpoint(&w, &[])?;
                w
            }
        } else {
            let (w, _) = build_vertical_slice_0(config.world_seed)?;
            w
        };

        let addr_str = config.clef_flash_addr.map(|a| a.to_string());
        let policy = AdaptiveDecisionPolicy::connect_or_fallback(addr_str.as_deref());

        Ok(Self {
            world,
            store,
            policy,
            pending_outbox: Vec::new(),
            next_intraday_seq_no: 0,
            dirty_for_coalesced_view: true,
        })
    }

    /// Executes an intraday `PlayerCommand` immediately on the authoritative `WorldState`,
    /// records it in `pending_outbox` for `(tick + 1)` deterministic replay, and returns any
    /// resulting `ClobDeltaEvent`.
    fn apply_intraday_command(
        &mut self,
        cmd: PlayerCommand,
        source_kind: DecisionSourceKind,
    ) -> Result<(u32, Option<ClobDeltaEvent>), ServerError> {
        let seq_no = self.next_intraday_seq_no;
        let affected_instrument = match &cmd {
            PlayerCommand::SubmitClobOrder { instrument_id, .. }
            | PlayerCommand::CancelClobOrder { instrument_id, .. }
            | PlayerCommand::IngestOraclePriceTick { instrument_id, .. } => Some(*instrument_id),
            PlayerCommand::BorrowSharesToShort { corp_id, .. } => self
                .world
                .corporations
                .get(*corp_id)
                .and_then(|c| c.equity_instrument_id),
            PlayerCommand::CoverShortPosition { short_loan_id, .. } => self
                .world
                .short_positions
                .get(short_loan_id)
                .and_then(|s| self.world.corporations.get(s.corp_id))
                .and_then(|c| c.equity_instrument_id),
            _ => None,
        };

        self.world.execute_command_sequenced(cmd.clone(), seq_no)?;
        self.next_intraday_seq_no = self.next_intraday_seq_no.saturating_add(1);
        self.dirty_for_coalesced_view = true;

        // Record in outbox under `current_tick + 1` so `verify_deterministic_replay` executes it at the start of the next tick
        let replay_tick = self.world.current_tick.0.saturating_add(1);
        self.pending_outbox.push(DecisionOutboxEntry {
            tick: replay_tick,
            seq_no,
            source_kind,
            command: cmd,
            probs_blake3_hash: [0u8; 32],
        });

        let delta = match affected_instrument {
            Some(inst_id) => Some(self.world.build_clob_delta_event(inst_id, seq_no)?),
            None => None,
        };
        Ok((seq_no, delta))
    }

    /// Advances the authoritative macro-simulation by 1 Fast Tick (1 simulated 30/360 day).
    /// Because intraday commands were already applied live to `self.world`, we pass `&[]` to
    /// `step_fast_tick_sequenced`, and at Month-End (`Day 30`) we evaluate Slow-Tick AI directives.
    fn advance_macro_tick(&mut self) -> Result<(), ServerError> {
        let next_tick = self.world.current_tick.0.saturating_add(1);
        let mut slow_tick_cmds = Vec::new();

        if next_tick.is_multiple_of(30) {
            let corp_ids: Vec<CorpId> = self.world.corporations.keys().collect();
            for cid in corp_ids {
                let (accepts_ai, persona_kind) = match self.world.corporations.get(cid) {
                    Some(c) => (c.delegation.accepts_slow_tick_ai(), c.persona_kind),
                    None => continue,
                };
                if !accepts_ai {
                    continue;
                }
                let obs = ActorObservationView::build(&self.world, cid)?;
                let persona = PersonaBiasVector::from_archetype(persona_kind);
                let (directive, probs_hash) = self
                    .policy
                    .evaluate_actor_slow_tick(self.world.world_seed, &obs, &persona)?;
                let seq_no = self.next_intraday_seq_no;
                self.next_intraday_seq_no = self.next_intraday_seq_no.saturating_add(1);
                let cmd = PlayerCommand::ApplyStrategicDirective(directive);
                slow_tick_cmds.push((seq_no, cmd.clone()));
                self.pending_outbox.push(DecisionOutboxEntry {
                    tick: next_tick,
                    seq_no,
                    source_kind: if matches!(
                        self.policy.status(),
                        AiBackendStatus::ClefFlashConnected { .. }
                    ) {
                        DecisionSourceKind::ClefFlashSlowTick
                    } else {
                        DecisionSourceKind::System0Heuristic
                    },
                    command: cmd,
                    probs_blake3_hash: probs_hash,
                });
            }
        }

        self.world.step_fast_tick_sequenced(&slow_tick_cmds)?;
        self.next_intraday_seq_no = 0;
        self.dirty_for_coalesced_view = true;

        // Auto-checkpoint at year-end (every 360 ticks)
        if self.world.current_tick.is_year_end() {
            let _ = self.save_checkpoint()?;
        }
        Ok(())
    }

    fn save_checkpoint(&mut self) -> Result<(u64, String), ServerError> {
        let tick = self.world.current_tick.0;
        let hash = if let Some(st) = &mut self.store {
            let outbox = std::mem::take(&mut self.pending_outbox);
            st.save_checkpoint(&self.world, &outbox)?
        } else {
            self.pending_outbox.clear();
            compute_world_blake3_hash(&self.world)?
        };
        let mut hex = String::with_capacity(64);
        for b in hash {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{b:02x}");
        }
        Ok((tick, hex))
    }

    fn resolve_token(
        &self,
        token: &str,
        token_map: &BTreeMap<String, String>,
    ) -> Option<ResolvedSessionRole> {
        let trimmed = token.trim();
        if let Some(mapped_ticker) = token_map.get(trimmed) {
            if mapped_ticker == "*" {
                return Some(ResolvedSessionRole::AdminObserver);
            }
            return self
                .world
                .corporations
                .iter()
                .find(|(_, c)| c.ticker.eq_ignore_ascii_case(mapped_ticker))
                .map(|(cid, _)| ResolvedSessionRole::Corporation(cid));
        }
        // Fallback convenience: allow matching `<TICKER>` directly in local/lan sessions
        self.world
            .corporations
            .iter()
            .find(|(_, c)| c.ticker.eq_ignore_ascii_case(trimmed))
            .map(|(cid, _)| ResolvedSessionRole::Corporation(cid))
    }
}

/// Shared state across Axum HTTP/WebSocket handlers and background tick/broadcast tasks.
#[derive(Clone)]
pub struct SharedServerState {
    engine: Arc<Mutex<AuthoritativeEngine>>,
    config: Arc<ServerConfig>,
    clob_delta_tx: broadcast::Sender<ClobDeltaEvent>,
    coalesced_tick_tx: broadcast::Sender<u64>,
}

impl std::fmt::Debug for SharedServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedServerState")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}
#[derive(Debug)]
pub struct RunningServerHandle {
    pub local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl RunningServerHandle {
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Spawns the authoritative **Black Tape** Axum server on `config.bind_addr` along with:
/// - Background Macro-Tick Scheduler (`config.tick_ms`)
/// - Guard #6 `100ms` (10 Hz) Coalesced `WorldView` Broadcast Signal
pub async fn spawn_authoritative_server(
    config: ServerConfig,
) -> Result<RunningServerHandle, ServerError> {
    let engine = AuthoritativeEngine::new(&config)?;
    let (clob_delta_tx, _) = broadcast::channel(512);
    let (coalesced_tick_tx, _) = broadcast::channel(64);

    let shared = SharedServerState {
        engine: Arc::new(Mutex::new(engine)),
        config: Arc::new(config.clone()),
        clob_delta_tx,
        coalesced_tick_tx: coalesced_tick_tx.clone(),
    };

    // 1. Spawn Guard #6 100ms (10 Hz) coalesced WorldView broadcast trigger
    let coalesced_state = shared.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            let mut guard = coalesced_state.engine.lock().await;
            if guard.dirty_for_coalesced_view {
                guard.dirty_for_coalesced_view = false;
                let tick = guard.world.current_tick.0;
                drop(guard);
                let _ = coalesced_state.coalesced_tick_tx.send(tick);
            }
        }
    });

    // 2. Spawn Macro-Tick timer if `tick_ms > 0`
    if config.tick_ms > 0 {
        let tick_state = shared.clone();
        let tick_ms = config.tick_ms;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(tick_ms));
            interval.tick().await; // consume immediate first tick
            loop {
                interval.tick().await;
                let mut guard = tick_state.engine.lock().await;
                let _ = guard.advance_macro_tick();
            }
        });
    }

    let app = Router::new()
        .route("/ws", get(ws_upgrade_handler))
        .route(
            "/api/v1/webhook/service-credit",
            post(webhook_service_credit_handler),
        )
        .route("/api/v1/oracle/tick", post(webhook_oracle_tick_handler))
        .route("/api/v1/admin/step-tick", post(admin_step_tick_handler))
        .with_state(shared);

    let listener = TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    Ok(RunningServerHandle {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
    })
}

async fn webhook_service_credit_handler(
    State(state): State<SharedServerState>,
    headers: HeaderMap,
    Json(payload): Json<ServiceCreditWebhookRequest>,
) -> impl IntoResponse {
    let secret = headers
        .get("x-oligopole-webhook-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if secret != state.config.webhook_secret {
        return (
            StatusCode::UNAUTHORIZED,
            Json(WebhookAckResponse {
                ok: false,
                tick: 0,
                seq_no: 0,
                message: "invalid X-Oligopole-Webhook-Secret header".to_owned(),
            }),
        );
    }

    let mut guard = state.engine.lock().await;
    let recipient_entity = guard
        .world
        .corporations
        .values()
        .find(|c| c.ticker.eq_ignore_ascii_case(payload.recipient_ticker.trim()))
        .map(|c| c.entity_id);

    let Some(recipient_entity_id) = recipient_entity else {
        return (
            StatusCode::BAD_REQUEST,
            Json(WebhookAckResponse {
                ok: false,
                tick: guard.world.current_tick.0,
                seq_no: 0,
                message: format!("unknown corporation ticker '{}'", payload.recipient_ticker),
            }),
        );
    };

    let cmd = PlayerCommand::AuthorityMintServiceCredit {
        recipient_entity_id,
        credits: payload.credits,
        webhook_ref: payload.webhook_ref.clone(),
    };

    match guard.apply_intraday_command(cmd, DecisionSourceKind::AuthorityWebhook) {
        Ok((seq_no, _)) => (
            StatusCode::OK,
            Json(WebhookAckResponse {
                ok: true,
                tick: guard.world.current_tick.0,
                seq_no,
                message: format!("minted {} ServiceCredits idempotently", payload.credits),
            }),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(WebhookAckResponse {
                ok: false,
                tick: guard.world.current_tick.0,
                seq_no: 0,
                message: e.to_string(),
            }),
        ),
    }
}

async fn webhook_oracle_tick_handler(
    State(state): State<SharedServerState>,
    headers: HeaderMap,
    Json(payload): Json<OracleTickWebhookRequest>,
) -> impl IntoResponse {
    let secret = headers
        .get("x-oligopole-webhook-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if secret != state.config.webhook_secret {
        return (
            StatusCode::UNAUTHORIZED,
            Json(WebhookAckResponse {
                ok: false,
                tick: 0,
                seq_no: 0,
                message: "invalid X-Oligopole-Webhook-Secret header".to_owned(),
            }),
        );
    }

    let mut guard = state.engine.lock().await;
    let target_q = payload.ticker.trim();
    let inst_id = guard
        .world
        .order_books
        .iter()
        .find(|(_, b)| {
            b.ticker.eq_ignore_ascii_case(target_q)
                || b.ticker
                    .split_whitespace()
                    .next()
                    .is_some_and(| first| first.eq_ignore_ascii_case(target_q))
        })
        .map(|(id, _)| id);

    let Some(instrument_id) = inst_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(WebhookAckResponse {
                ok: false,
                tick: guard.world.current_tick.0,
                seq_no: 0,
                message: format!("unknown Depositary Receipt ticker '{}'", payload.ticker),
            }),
        );
    };

    let cmd = PlayerCommand::IngestOraclePriceTick {
        instrument_id,
        new_oracle_price: Money::from_cents(payload.new_price_cents),
    };

    match guard.apply_intraday_command(cmd, DecisionSourceKind::OracleFeed) {
        Ok((seq_no, maybe_delta)) => {
            let tick = guard.world.current_tick.0;
            drop(guard);
            if let Some(delta) = maybe_delta {
                let _ = state.clob_delta_tx.send(delta);
            }
            (
                StatusCode::OK,
                Json(WebhookAckResponse {
                    ok: true,
                    tick,
                    seq_no,
                    message: "oracle LULD reference re-anchored and DR quotes refreshed".to_owned(),
                }),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(WebhookAckResponse {
                ok: false,
                tick: guard.world.current_tick.0,
                seq_no: 0,
                message: e.to_string(),
            }),
        ),
    }
}

async fn admin_step_tick_handler(
    State(state): State<SharedServerState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let secret = headers
        .get("x-oligopole-webhook-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if secret != state.config.webhook_secret {
        return (
            StatusCode::UNAUTHORIZED,
            Json(WebhookAckResponse {
                ok: false,
                tick: 0,
                seq_no: 0,
                message: "invalid X-Oligopole-Webhook-Secret header".to_owned(),
            }),
        );
    }

    let mut guard = state.engine.lock().await;
    match guard.advance_macro_tick() {
        Ok(()) => (
            StatusCode::OK,
            Json(WebhookAckResponse {
                ok: true,
                tick: guard.world.current_tick.0,
                seq_no: 0,
                message: "advanced 1 macro fast-tick".to_owned(),
            }),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(WebhookAckResponse {
                ok: false,
                tick: guard.world.current_tick.0,
                seq_no: 0,
                message: e.to_string(),
            }),
        ),
    }
}

async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedServerState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket_session(socket, state))
}

async fn handle_websocket_session(socket: WebSocket, state: SharedServerState) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut delta_rx = state.clob_delta_tx.subscribe();
    let mut coalesced_rx = state.coalesced_tick_tx.subscribe();

    // Default unauthenticated sessions to PLYR in local dev until `Authenticate` frame arrives
    let mut viewer_corp_id: Option<CorpId> = None;

    // Push initial WorldView snapshot immediately upon connection
    if let Ok(initial_bytes) = {
        let guard = state.engine.lock().await;
        guard
            .world
            .build_authorized_world_view(viewer_corp_id)
            .and_then(|v| {
                rmp_serde::to_vec(&ServerWireFrame::WorldViewUpdate(Box::new(v)))
                    .map_err(|e| CoreError::InvalidOrder {
                        reason: e.to_string(),
                    })
            })
    } {
        if ws_tx.send(Message::Binary(initial_bytes.into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            maybe_msg = ws_rx.next() => {
                let Some(Ok(msg)) = maybe_msg else {
                    break;
                };
                if let Message::Binary(bytes) = msg {
                    let Ok(client_frame) = rmp_serde::from_slice::<ClientWireFrame>(&bytes) else {
                        continue;
                    };
                    match client_frame {
                        ClientWireFrame::Authenticate { token } => {
                            let mut guard = state.engine.lock().await;
                            if let Some(role) = guard.resolve_token(&token, &state.config.session_tokens) {
                                viewer_corp_id = match role {
                                    ResolvedSessionRole::AdminObserver => None,
                                    ResolvedSessionRole::Corporation(cid) => Some(cid),
                                };
                                guard.dirty_for_coalesced_view = true;
                                let auth_frame = ServerWireFrame::Authenticated {
                                    assigned_corp_id: viewer_corp_id,
                                    server_motd: "BLACK TAPE AUTHORITATIVE EXCHANGE ONLINE".to_owned(),
                                };
                                let maybe_view = guard.world.build_authorized_world_view(viewer_corp_id).ok();
                                drop(guard);
                                if let Ok(buf) = rmp_serde::to_vec(&auth_frame) {
                                    let _ = ws_tx.send(Message::Binary(buf.into())).await;
                                }
                                if let Some(view) = maybe_view {
                                    if let Ok(buf) = rmp_serde::to_vec(&ServerWireFrame::WorldViewUpdate(Box::new(view))) {
                                        let _ = ws_tx.send(Message::Binary(buf.into())).await;
                                    }
                                }
                            } else {
                                drop(guard);
                                let rej = ServerWireFrame::CommandRejected {
                                    reason: format!("unknown session token '{token}'"),
                                };
                                if let Ok(buf) = rmp_serde::to_vec(&rej) {
                                    let _ = ws_tx.send(Message::Binary(buf.into())).await;
                                }
                            }
                        }
                        ClientWireFrame::SubmitCommand(cmd) => {
                            let mut guard = state.engine.lock().await;
                            match guard.apply_intraday_command(cmd, DecisionSourceKind::PlayerManual) {
                                Ok((_, maybe_delta)) => {
                                    drop(guard);
                                    if let Some(delta) = maybe_delta {
                                        let _ = state.clob_delta_tx.send(delta);
                                    }
                                }
                                Err(e) => {
                                    drop(guard);
                                    let rej = ServerWireFrame::CommandRejected {
                                        reason: e.to_string(),
                                    };
                                    if let Ok(buf) = rmp_serde::to_vec(&rej) {
                                        let _ = ws_tx.send(Message::Binary(buf.into())).await;
                                    }
                                }
                            }
                        }
                        ClientWireFrame::RequestCheckpointSave => {
                            let mut guard = state.engine.lock().await;
                            if let Ok((tick, blake3_hex)) = guard.save_checkpoint() {
                                drop(guard);
                                let saved = ServerWireFrame::CheckpointSaved { tick, blake3_hex };
                                if let Ok(buf) = rmp_serde::to_vec(&saved) {
                                    let _ = ws_tx.send(Message::Binary(buf.into())).await;
                                }
                            }
                        }
                        ClientWireFrame::Ping { client_ts_ms } => {
                            let server_tick = state.engine.lock().await.world.current_tick.0;
                            let pong = ServerWireFrame::Pong {
                                client_ts_ms,
                                server_tick,
                            };
                            if let Ok(buf) = rmp_serde::to_vec(&pong) {
                                let _ = ws_tx.send(Message::Binary(buf.into())).await;
                            }
                        }
                    }
                }
            }
            Ok(delta) = delta_rx.recv() => {
                if let Ok(buf) = rmp_serde::to_vec(&ServerWireFrame::ClobDelta(delta)) {
                    if ws_tx.send(Message::Binary(buf.into())).await.is_err() {
                        break;
                    }
                }
            }
            Ok(_) = coalesced_rx.recv() => {
                let maybe_view = {
                    let guard = state.engine.lock().await;
                    guard.world.build_authorized_world_view(viewer_corp_id).ok()
                };
                if let Some(view) = maybe_view {
                    if let Ok(buf) = rmp_serde::to_vec(&ServerWireFrame::WorldViewUpdate(Box::new(view))) {
                        if ws_tx.send(Message::Binary(buf.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oligopole_core::{OrderSide, WireAttachment};
    use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};

    #[tokio::test]
    async fn test_authoritative_server_ws_clob_deltas_information_barrier_and_webhooks() {
        let config = ServerConfig {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            save_path: None,
            tick_ms: 0, // Manual tick stepping for deterministic test assertions
            clef_flash_addr: None,
            webhook_secret: "test-secret".to_owned(),
            world_seed: [77u8; 32],
            ..ServerConfig::default()
        };

        let mut server = spawn_authoritative_server(config).await.unwrap();
        let ws_url = format!("ws://{}/ws", server.local_addr);

        let (mut ws_plyr, _) = connect_async(&ws_url).await.unwrap();
        let (mut ws_iron, _) = connect_async(&ws_url).await.unwrap();

        // Drain initial WorldViewUpdate on both clients
        let _ = ws_plyr.next().await.unwrap().unwrap();
        let _ = ws_iron.next().await.unwrap().unwrap();

        // Authenticate Client 1 as PLYR and Client 2 as IRON
        let auth_plyr = rmp_serde::to_vec(&ClientWireFrame::Authenticate {
            token: "PLYR-TOKEN".to_owned(),
        })
        .unwrap();
        ws_plyr
            .send(WsMessage::Binary(auth_plyr.into()))
            .await
            .unwrap();

        let auth_iron = rmp_serde::to_vec(&ClientWireFrame::Authenticate {
            token: "IRON-TOKEN".to_owned(),
        })
        .unwrap();
        ws_iron
            .send(WsMessage::Binary(auth_iron.into()))
            .await
            .unwrap();

        // Read Authenticated + WorldViewUpdate frames for PLYR
        let mut plyr_corp_id = None;
        let mut plyr_view = None;
        for _ in 0..2 {
            if let Some(Ok(WsMessage::Binary(b))) = ws_plyr.next().await {
                match rmp_serde::from_slice::<ServerWireFrame>(&b).unwrap() {
                    ServerWireFrame::Authenticated {
                        assigned_corp_id, ..
                    } => plyr_corp_id = assigned_corp_id,
                    ServerWireFrame::WorldViewUpdate(v) => plyr_view = Some(*v),
                    _ => {}
                }
            }
        }
        let plyr_cid = plyr_corp_id.unwrap();
        let view = plyr_view.unwrap();
        let plyr_snap = view
            .corporations
            .iter()
            .find(|c| c.corp_id == plyr_cid)
            .unwrap();
        let apex_snap = view
            .corporations
            .iter()
            .find(|c| c.ticker == "APEX")
            .unwrap();
        let googl_inst = view
            .instruments
            .iter()
            .find(|i| i.ticker == "GOOGL-DR EQ")
            .unwrap();

        // Drain IRON's Authenticated + WorldViewUpdate frames
        for _ in 0..2 {
            let _ = ws_iron.next().await;
        }

        // PLYR submits a real-time Bid on GOOGL-DR EQ -> both clients receive immediate ClobDelta!
        let order_cmd = ClientWireFrame::SubmitCommand(PlayerCommand::SubmitClobOrder {
            trader_entity_id: plyr_snap.entity_id,
            instrument_id: googl_inst.instrument_id,
            side: OrderSide::Bid,
            limit_price: Money::from_dollars(280),
            qty: 5,
        });
        ws_plyr
            .send(WsMessage::Binary(rmp_serde::to_vec(&order_cmd).unwrap().into()))
            .await
            .unwrap();

        // Verify IRON receives the immediate ClobDelta frame
        let iron_frame = loop {
            if let Some(Ok(WsMessage::Binary(b))) = ws_iron.next().await {
                let frame = rmp_serde::from_slice::<ServerWireFrame>(&b).unwrap();
                if matches!(frame, ServerWireFrame::ClobDelta(_)) {
                    break frame;
                }
            }
        };
        if let ServerWireFrame::ClobDelta(delta) = iron_frame {
            assert_eq!(delta.instrument_id, googl_inst.instrument_id);
        }

        // PLYR sends a private WireMessage to APEX -> IRON's coalesced WorldView must NOT see it!
        let wire_cmd = ClientWireFrame::SubmitCommand(PlayerCommand::SendWireMessage {
            sender_corp_id: plyr_cid,
            recipient_corp_id: apex_snap.corp_id,
            subject: "Confidential Merger Proposal".to_owned(),
            body: "Top secret".to_owned(),
            attachment: Some(WireAttachment::SyndicateExtortionDemand {
                demand_amount: Money::from_dollars(10_000),
                immunity_days: 30,
            }),
        });
        ws_plyr
            .send(WsMessage::Binary(rmp_serde::to_vec(&wire_cmd).unwrap().into()))
            .await
            .unwrap();

        // Wait for 100ms coalesced WorldViewUpdate on PLYR containing the wire message
        let plyr_updated_view = loop {
            if let Some(Ok(WsMessage::Binary(b))) = ws_plyr.next().await {
                if let ServerWireFrame::WorldViewUpdate(v) =
                    rmp_serde::from_slice::<ServerWireFrame>(&b).unwrap()
                {
                    if !v.wire_messages.is_empty() {
                        break *v;
                    }
                }
            }
        };
        assert_eq!(plyr_updated_view.wire_messages.len(), 1);

        // Trigger another state mutation (e.g. SetCorporationPrice) so IRON gets a fresh coalesced WorldViewUpdate
        // AFTER the wire message was already recorded, and verify IRON still sees 0 wire_messages!
        let price_cmd = ClientWireFrame::SubmitCommand(PlayerCommand::SetCorporationPrice {
            corp_id: plyr_cid,
            new_price: Money::from_dollars(225),
        });
        ws_plyr
            .send(WsMessage::Binary(rmp_serde::to_vec(&price_cmd).unwrap().into()))
            .await
            .unwrap();

        let iron_updated_view = loop {
            if let Some(Ok(WsMessage::Binary(b))) = ws_iron.next().await {
                if let ServerWireFrame::WorldViewUpdate(v) =
                    rmp_serde::from_slice::<ServerWireFrame>(&b).unwrap()
                {
                    let plyr_in_iron = v
                        .corporations
                        .iter()
                        .find(|c| c.corp_id == plyr_cid)
                        .unwrap();
                    if plyr_in_iron.posted_selling_price == Money::from_dollars(225) {
                        break *v;
                    }
                }
            }
        };

        assert!(
            iron_updated_view.wire_messages.is_empty(),
            "Information barrier must mask PLYR <-> APEX private wire from IRON"
        );

        server.shutdown();
    }
}
