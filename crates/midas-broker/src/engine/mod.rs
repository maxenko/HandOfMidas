use std::collections::{HashMap, VecDeque};

use tokio::sync::{broadcast, mpsc, watch};
use uuid::Uuid;

use crate::client::{BrokerCallback, BrokerClient};
use crate::commands::BrokerCommand;
use crate::config::{BrokerConfig, DataSourceConfig, TradingLimits};
use crate::connection::ConnectionState;
use crate::db::BrokerDb;
use crate::error::BrokerError;
use crate::events::BrokerEvent;
use crate::market_data::MarketDataSource;
use crate::orders::bracket::{
    BracketGroup, BracketLifecycleStatus, MarketBracketParams, derive_bracket_status,
    validate_market_bracket, check_bracket_direction,
};
use crate::orders::state::OrderStatus;
use crate::orders::types::{BracketRole, LocalOrder, OrderAction, OrderKind, TimeInForce};

/// Returned by [`start_broker_engine`]. The UI interacts with the broker
/// exclusively through these channel handles.
///
/// - Send commands via `commands` (mpsc).
/// - Receive market data events via `market_events` (broadcast).
/// - Receive order lifecycle events via `order_events` (broadcast).
/// - Watch the connection state via `connection_state` (watch).
pub struct BrokerHandle {
    /// Send commands to the engine (connect, place orders, subscribe, etc.).
    pub commands: mpsc::Sender<BrokerCommand>,
    /// Subscribe to market data events (ticks, bars, depth).
    pub market_events: broadcast::Sender<BrokerEvent>,
    /// Subscribe to order lifecycle events (fills, status changes, errors).
    pub order_events: broadcast::Sender<BrokerEvent>,
    /// Watch the current connection state.
    pub connection_state: watch::Receiver<ConnectionState>,
}

/// Creates the broker engine and returns channel handles.
///
/// The engine runs as a tokio task on the current runtime. It processes
/// commands from the `BrokerHandle::commands` sender, emits events on the
/// broadcast channels, and updates connection state on the watch channel.
///
/// # Panics
///
/// Panics if called outside of a tokio runtime context.
pub fn start_broker_engine(config: BrokerConfig) -> BrokerHandle {
    let (command_tx, command_rx) = mpsc::channel::<BrokerCommand>(256);
    let (market_event_tx, _) = broadcast::channel::<BrokerEvent>(4096);
    let (order_event_tx, _) = broadcast::channel::<BrokerEvent>(8192);
    let (conn_state_tx, conn_state_rx) = watch::channel(ConnectionState::Disconnected);

    let market_tx_clone = market_event_tx.clone();
    let order_tx_clone = order_event_tx.clone();

    let data_source: Option<Box<dyn MarketDataSource>> = match &config.data_source {
        DataSourceConfig::Test => {
            Some(Box::new(crate::testdata::TestDataProvider::new()))
        }
        DataSourceConfig::Live => None, // IB data source created after connect
    };

    let client: Option<Box<dyn BrokerClient>> = match &config.data_source {
        DataSourceConfig::Test => {
            Some(Box::new(crate::test_broker::TestBroker::new(config.test_broker.clone())))
        }
        DataSourceConfig::Live => {
            let conn_cfg = &config.connection;
            Some(Box::new(crate::ib_client::IbClient::new(
                &conn_cfg.host,
                conn_cfg.port,
                conn_cfg.client_id,
            )))
        }
    };

    // Open in-memory DB for tests, or file-backed for production.
    let store = BrokerDb::open_in_memory().ok();

    tokio::spawn(async move {
        let mut engine = BrokerEngine {
            config,
            command_rx,
            market_event_tx: market_tx_clone,
            order_event_tx: order_tx_clone,
            conn_state_tx,
            data_source,
            client,
            store,
            bracket_status_cache: HashMap::new(),
            ib_to_local: HashMap::new(),
            bracket_ib_ids: HashMap::new(),
            terminal_cleanups: VecDeque::new(),
            was_connected: false,
            reconnect_attempt: 0,
        };
        engine.run().await;
    });

    BrokerHandle {
        commands: command_tx,
        market_events: market_event_tx,
        order_events: order_event_tx,
        connection_state: conn_state_rx,
    }
}

/// The internal engine that drives the broker. Not exposed publicly.
struct BrokerEngine {
    config: BrokerConfig,
    command_rx: mpsc::Receiver<BrokerCommand>,
    market_event_tx: broadcast::Sender<BrokerEvent>,
    order_event_tx: broadcast::Sender<BrokerEvent>,
    conn_state_tx: watch::Sender<ConnectionState>,
    data_source: Option<Box<dyn MarketDataSource>>,
    /// Broker client for order submission (real IB or test).
    client: Option<Box<dyn BrokerClient>>,
    /// SQLite persistence handle for order storage.
    store: Option<BrokerDb>,
    /// Last-emitted bracket status per parent_id. Prevents duplicate events.
    bracket_status_cache: HashMap<Uuid, BracketLifecycleStatus>,
    /// Reverse lookup: IB order ID → local UUID. Populated during submission.
    ib_to_local: HashMap<i32, Uuid>,
    /// Parent UUID → IB order IDs for all legs. Used for cleanup.
    bracket_ib_ids: HashMap<Uuid, Vec<i32>>,
    /// Deferred cleanup queue: (time_became_terminal, parent_uuid).
    /// Entries older than 60s are swept on heartbeat.
    terminal_cleanups: VecDeque<(std::time::Instant, Uuid)>,
    /// Whether we were previously connected (for reconnect detection).
    was_connected: bool,
    /// Current reconnect attempt count.
    reconnect_attempt: u32,
}

impl BrokerEngine {
    /// Main event loop. Runs until the command channel is closed or a
    /// `Shutdown` command is received.
    async fn run(&mut self) {
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));
        let mut poll_interval = tokio::time::interval(std::time::Duration::from_millis(10));

        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(command) => {
                            if self.handle_command(command).await {
                                break;
                            }
                        }
                        None => {
                            // All senders dropped; shut down.
                            tracing::info!("Command channel closed, engine stopping");
                            break;
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    self.sweep_terminal_brackets();
                    self.check_reconnect().await;
                }
                _ = poll_interval.tick() => {
                    if let Some(ref client) = self.client {
                        let callbacks = client.poll_callbacks();
                        for cb in callbacks {
                            self.handle_broker_callback(cb);
                        }
                    }
                }
            }
        }

        tracing::info!("Broker engine stopped");
    }

    /// Handle a single command. Returns `true` if the engine should stop.
    async fn handle_command(&mut self, cmd: BrokerCommand) -> bool {
        match cmd {
            BrokerCommand::Shutdown => {
                tracing::info!("Broker engine shutting down");
                self.command_rx.close();
                true
            }
            BrokerCommand::RequestHistoricalData {
                symbol,
                con_id,
                duration,
                bar_size,
                request_id,
            } => {
                if let Some(ref mut source) = self.data_source {
                    if let Err(e) = Self::dispatch_historical(
                        source.as_mut(),
                        &self.market_event_tx,
                        &symbol,
                        con_id,
                        &duration,
                        &bar_size,
                        request_id,
                    ) {
                        let _ = self.market_event_tx.send(BrokerEvent::Error {
                            code: -1,
                            message: format!("historical data error: {e}"),
                        });
                    }
                } else {
                    tracing::debug!(
                        "RequestHistoricalData: no data source configured (IB not yet implemented)"
                    );
                }
                false
            }
            BrokerCommand::CreateMarketBracket(params) => {
                self.handle_create_market_bracket(params);
                false
            }
            BrokerCommand::CancelBracket { parent_id } => {
                self.handle_cancel_bracket(parent_id);
                false
            }
            BrokerCommand::ModifyBracketLeg { order_id, new_price } => {
                self.handle_modify_bracket_leg(order_id, new_price);
                false
            }
            BrokerCommand::Connect => {
                if let Some(ref client) = self.client {
                    match client.connect() {
                        Ok(ver) => {
                            let _ = self.order_event_tx.send(BrokerEvent::Connected {
                                server_version: ver,
                            });
                            tracing::info!("Connected to broker (server version {ver})");
                        }
                        Err(e) => {
                            let _ = self.order_event_tx.send(BrokerEvent::Error {
                                code: -10,
                                message: format!("connection failed: {e}"),
                            });
                        }
                    }
                }
                false
            }
            BrokerCommand::Disconnect => {
                if let Some(ref client) = self.client {
                    client.disconnect();
                    let _ = self.order_event_tx.send(BrokerEvent::Disconnected {
                        reason: "user requested disconnect".to_string(),
                    });
                    tracing::info!("Disconnected from broker");
                }
                false
            }
            BrokerCommand::SubscribeMarketData { symbol, con_id } => {
                if let Some(ref client) = self.client {
                    client.subscribe_market_data(&symbol, con_id);
                }
                false
            }
            BrokerCommand::UnsubscribeMarketData { symbol } => {
                if let Some(ref client) = self.client {
                    client.unsubscribe_market_data(&symbol);
                }
                false
            }
            BrokerCommand::RequestPositions => {
                if let Some(ref client) = self.client {
                    for pos in client.request_positions() {
                        let _ = self.order_event_tx.send(BrokerEvent::PositionUpdate {
                            account: String::new(),
                            symbol: pos.symbol,
                            con_id: 0,
                            quantity: pos.quantity,
                            avg_cost: pos.avg_cost,
                        });
                    }
                }
                false
            }
            BrokerCommand::RequestAccountSummary => {
                if let Some(ref client) = self.client {
                    let summary = client.request_account_summary();
                    let _ = self.order_event_tx.send(BrokerEvent::PnlUpdate {
                        daily_pnl: 0.0,
                        unrealized_pnl: summary.unrealized_pnl,
                        realized_pnl: summary.realized_pnl,
                    });
                    let _ = self.order_event_tx.send(BrokerEvent::AccountValueUpdate {
                        account: String::new(),
                        key: "CashBalance".to_string(),
                        value: format!("{:.2}", summary.cash_balance),
                        currency: "USD".to_string(),
                    });
                }
                false
            }
            BrokerCommand::CancelOrder { order_id } => {
                self.handle_cancel_order(order_id);
                false
            }
            BrokerCommand::ModifyOrder { order_id, new_price, new_qty } => {
                self.handle_modify_order(order_id, new_price, new_qty);
                false
            }
            BrokerCommand::RequestOrderSnapshot => {
                self.handle_request_order_snapshot();
                false
            }
        }
    }

    /// Parse IB strings, fetch bars from the data source, and emit events.
    fn dispatch_historical(
        source: &mut dyn MarketDataSource,
        tx: &broadcast::Sender<BrokerEvent>,
        symbol: &str,
        con_id: i32,
        duration: &str,
        bar_size: &str,
        request_id: u64,
    ) -> Result<(), BrokerError> {
        use crate::ib_strings::{duration_to_start, parse_bar_size};

        let timeframe = parse_bar_size(bar_size)?;
        let end = chrono::Utc::now().timestamp();
        let start = duration_to_start(end, duration)?;

        let result = source.historical_bars(symbol, con_id, timeframe, start, end, request_id)?;

        for bar in &result.bars {
            let _ = tx.send(BrokerEvent::BarClosed {
                symbol: result.symbol.clone(),
                timestamp: bar.timestamp,
                open: bar.open,
                high: bar.high,
                low: bar.low,
                close: bar.close,
                volume: bar.volume,
            });
        }

        let _ = tx.send(BrokerEvent::HistoricalDataComplete {
            request_id: result.request_id,
            symbol: result.symbol.clone(),
        });

        Ok(())
    }

    // ── Market Bracket Handling ─────────────────────────────────────────

    /// Handle CreateMarketBracket: validate → build → persist → emit.
    fn handle_create_market_bracket(&mut self, params: MarketBracketParams) {
        // 1. Validate params
        if let Err(errors) = validate_market_bracket(&params) {
            let msg = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
            tracing::warn!("Market bracket validation failed: {msg}");
            let _ = self.order_event_tx.send(BrokerEvent::OrderValidationFailed {
                code: -1,
                message: format!("bracket validation failed: {msg}"),
            });
            return;
        }

        // 2. Order size guard
        if let Err(e) = validate_order_size(&params, &self.config.trading_limits) {
            tracing::warn!("Order size guard rejected bracket: {e}");
            let _ = self.order_event_tx.send(BrokerEvent::OrderValidationFailed {
                code: -2,
                message: format!("order size rejected: {e}"),
            });
            return;
        }

        // 3. Directional warnings (log only, don't reject)
        if let Some(ref_price) = params.reference_price {
            let warnings = check_bracket_direction(
                params.action,
                ref_price,
                params.take_profit.as_ref().map(|tp| tp.price),
                params.stop_loss.as_ref().map(|sl| sl.stop_price),
            );
            for w in &warnings {
                tracing::warn!("Bracket direction warning: {w:?}");
            }
        }

        // 4. Build bracket orders
        let mut group = build_market_bracket(&params);
        let parent_id = group.parent.id;

        // 5. Persist bracket and transition all legs to PendingSubmit
        //    Uses a single SQLite transaction with audit trail.
        if let Some(ref store) = self.store {
            let conn = store.conn().lock().expect("db mutex poisoned");
            if let Err(e) = crate::persist::order_repo::persist_and_transition_to_pending_submit(&conn, &mut group) {
                tracing::error!("Failed to persist bracket {parent_id}: {e}");
                let _ = self.order_event_tx.send(BrokerEvent::OrderError {
                    order_id: parent_id,
                    code: -3,
                    message: format!("bracket persistence failed: {e}"),
                });
                return;
            }
            tracing::info!(
                "Bracket {parent_id} persisted ({} legs)",
                group.legs().len()
            );
        }

        // 7. Cache initial bracket status
        let status = derive_bracket_status(&group);
        self.bracket_status_cache.insert(parent_id, status);

        // 8. Submit bracket to broker client
        if let Err(e) = self.submit_bracket_to_ib(&mut group) {
            tracing::error!("Bracket {parent_id} submission failed: {e}");
            // Transition all legs to Error in DB
            if let Some(ref store) = self.store {
                let conn = store.conn().lock().expect("db mutex poisoned");
                let _ = crate::persist::order_repo::transition_bracket_to_error(
                    &conn, &group, &e,
                );
            }
            let _ = self.order_event_tx.send(BrokerEvent::OrderError {
                order_id: parent_id,
                code: -4,
                message: format!("bracket submission failed: {e}"),
            });
            return;
        }

        // Populate IB-to-local reverse lookup for callback translation
        let mut ib_ids = Vec::new();
        if let Some(ib_id) = group.parent.ib_order_id {
            self.ib_to_local.insert(ib_id, group.parent.id);
            ib_ids.push(ib_id);
        }
        if let Some(ref tp) = group.take_profit {
            if let Some(ib_id) = tp.ib_order_id {
                self.ib_to_local.insert(ib_id, tp.id);
                ib_ids.push(ib_id);
            }
        }
        if let Some(ref sl) = group.stop_loss {
            if let Some(ib_id) = sl.ib_order_id {
                self.ib_to_local.insert(ib_id, sl.id);
                ib_ids.push(ib_id);
            }
        }
        self.bracket_ib_ids.insert(parent_id, ib_ids);

        // 9. Emit BracketCreated event (after successful persistence and submission)
        let _ = self.order_event_tx.send(BrokerEvent::BracketCreated {
            parent_id,
            take_profit_id: group.take_profit.as_ref().map(|tp| tp.id),
            stop_loss_id: group.stop_loss.as_ref().map(|sl| sl.id),
            symbol: params.symbol.clone(),
            action: params.action,
            quantity: params.quantity,
            tp_price: params.take_profit.as_ref().map(|tp| tp.price),
            sl_price: params.stop_loss.as_ref().map(|sl| sl.stop_price),
            reference_price: params.reference_price,
        });

        tracing::info!(
            "Market bracket {parent_id} submitted: {} {} {} (TP: {}, SL: {})",
            params.action,
            params.quantity,
            params.symbol,
            params.take_profit.is_some(),
            params.stop_loss.is_some(),
        );
    }

    // ── Bracket IB Submission ──────────────────────────────────────────

    /// Submit a bracket to the broker client with correct transmit flag ordering.
    ///
    /// IB bracket semantics: the last child must be placed with `transmit=true`,
    /// which atomically transmits the entire bracket. Parent and non-last
    /// children are placed with `transmit=false`.
    fn submit_bracket_to_ib(&self, group: &mut BracketGroup) -> Result<(), String> {
        let client = self.client.as_ref().ok_or("no broker client configured")?;

        let has_children = group.take_profit.is_some() || group.stop_loss.is_some();

        // Step 1: Allocate IB order IDs (one per leg)
        let parent_ib_id = client.next_order_id();
        group.parent.ib_order_id = Some(parent_ib_id);

        if let Some(ref mut tp) = group.take_profit {
            tp.ib_order_id = Some(client.next_order_id());
        }
        if let Some(ref mut sl) = group.stop_loss {
            sl.ib_order_id = Some(client.next_order_id());
        }

        // Step 2: Place parent (transmit=false if has children)
        client.place_order(
            parent_ib_id,
            &group.parent.symbol,
            &group.parent.action.to_string(),
            &group.parent.order_type.to_string(),
            group.parent.quantity,
            group.parent.limit_price,
            group.parent.stop_price,
            None,
            !has_children,
            &group.parent.tif.to_string(),
            group.parent.outside_rth,
        ).map_err(|e| format!("parent placement failed: {e}"))?;

        // Emit OrderSubmitted for parent
        // ib_perm_id is set to 0; the real perm_id arrives via OrderStatus callback.
        let _ = self.order_event_tx.send(BrokerEvent::OrderSubmitted {
            order_id: group.parent.id,
            ib_order_id: parent_ib_id,
            ib_perm_id: 0,
        });

        // Step 3: Place TP (transmit=false if SL follows, true if last child)
        if let Some(ref tp) = group.take_profit {
            let tp_ib_id = tp.ib_order_id
                .ok_or_else(|| "TP leg missing ib_order_id after allocation".to_string())?;
            let is_last_child = group.stop_loss.is_none();
            client.place_order(
                tp_ib_id,
                &tp.symbol,
                &tp.action.to_string(),
                &tp.order_type.to_string(),
                tp.quantity,
                tp.limit_price,
                tp.stop_price,
                Some(parent_ib_id),
                is_last_child,
                &tp.tif.to_string(),
                tp.outside_rth,
            ).map_err(|e| {
                // Cancel parent on TP failure
                let _ = client.cancel_order(parent_ib_id);
                format!("TP placement failed: {e}")
            })?;

            // ib_perm_id is set to 0; the real perm_id arrives via OrderStatus callback.
            let _ = self.order_event_tx.send(BrokerEvent::OrderSubmitted {
                order_id: tp.id,
                ib_order_id: tp_ib_id,
                ib_perm_id: 0,
            });
        }

        // Step 4: Place SL (always transmit=true — triggers entire bracket)
        if let Some(ref sl) = group.stop_loss {
            let sl_ib_id = sl.ib_order_id
                .ok_or_else(|| "SL leg missing ib_order_id after allocation".to_string())?;
            client.place_order(
                sl_ib_id,
                &sl.symbol,
                &sl.action.to_string(),
                &sl.order_type.to_string(),
                sl.quantity,
                sl.limit_price,
                sl.stop_price,
                Some(parent_ib_id),
                true,
                &sl.tif.to_string(),
                sl.outside_rth,
            ).map_err(|e| {
                // Cancel parent and TP on SL failure
                let _ = client.cancel_order(parent_ib_id);
                if let Some(ref tp) = group.take_profit {
                    if let Some(tp_id) = tp.ib_order_id {
                        let _ = client.cancel_order(tp_id);
                    }
                }
                format!("SL placement failed: {e}")
            })?;

            // ib_perm_id is set to 0; the real perm_id arrives via OrderStatus callback.
            let _ = self.order_event_tx.send(BrokerEvent::OrderSubmitted {
                order_id: sl.id,
                ib_order_id: sl_ib_id,
                ib_perm_id: 0,
            });
        }

        Ok(())
    }

    // ── Cancel Bracket Handling ────────────────────────────────────────

    /// Handle CancelBracket: cancel all legs of a bracket.
    fn handle_cancel_bracket(&mut self, parent_id: Uuid) {
        // Load bracket from DB (if store available)
        if let Some(ref store) = self.store {
            // 1. Acquire lock, read necessary data, drop lock
            let (parent_row, children) = {
                let conn = store.conn().lock().expect("db mutex poisoned");

                let parent_row = match crate::persist::order_repo::get_order(&conn, &parent_id.to_string()) {
                    Ok(Some(row)) => row,
                    Ok(None) => {
                        tracing::warn!("CancelBracket: parent {parent_id} not found");
                        return;
                    }
                    Err(e) => {
                        tracing::error!("CancelBracket: DB error loading parent {parent_id}: {e}");
                        return;
                    }
                };

                let children = match crate::persist::order_repo::get_orders_by_parent_id(&conn, &parent_id.to_string()) {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::error!("CancelBracket: DB error loading children for {parent_id}: {e}");
                        return;
                    }
                };

                (parent_row, children)
                // MutexGuard drops here
            };

            // 2. Cancel orders at broker client (no lock held)
            if let Some(ref client) = self.client {
                if !["Filled", "Cancelled", "Rejected"].contains(&parent_row.status.as_str()) {
                    if let Some(ib_id) = parent_row.ib_order_id {
                        let _ = client.cancel_order(ib_id);
                    }
                }
                for child in &children {
                    if !["Filled", "Cancelled", "Rejected"].contains(&child.status.as_str()) {
                        if let Some(ib_id) = child.ib_order_id {
                            let _ = client.cancel_order(ib_id);
                        }
                    }
                }
            }

            // 3. Re-acquire lock for DB writes
            {
                let conn = store.conn().lock().expect("db mutex poisoned");
                let now = chrono::Utc::now().to_rfc3339();

                if !["Filled", "Cancelled", "Rejected"].contains(&parent_row.status.as_str()) {
                    let _ = crate::persist::order_repo::update_order_status(&conn, &parent_row.local_id, "Cancelled", &now);
                    let _ = crate::persist::order_repo::write_audit(&conn, &parent_row.local_id, &parent_row.status, "Cancelled", Some("bracket cancel"), "engine");
                    let _ = self.order_event_tx.send(BrokerEvent::OrderCancelled {
                        order_id: parent_id,
                        reason: "bracket cancelled".to_string(),
                    });
                }

                for child in &children {
                    if !["Filled", "Cancelled", "Rejected"].contains(&child.status.as_str()) {
                        let _ = crate::persist::order_repo::update_order_status(&conn, &child.local_id, "Cancelled", &now);
                        let _ = crate::persist::order_repo::write_audit(&conn, &child.local_id, &child.status, "Cancelled", Some("bracket cancel"), "engine");
                        if let Ok(child_id) = child.local_id.parse::<Uuid>() {
                            let _ = self.order_event_tx.send(BrokerEvent::OrderCancelled {
                                order_id: child_id,
                                reason: "bracket cancelled".to_string(),
                            });
                        }
                    }
                }
            }

            // 4. Derive bracket status instead of hard-coding Cancelled (W5)
            //    Re-load the bracket group with updated statuses to derive correctly.
            let lifecycle = {
                let conn = store.conn().lock().expect("db mutex poisoned");
                let parent_row = crate::persist::order_repo::get_order(&conn, &parent_id.to_string())
                    .ok().flatten();
                let children = crate::persist::order_repo::get_orders_by_parent_id(&conn, &parent_id.to_string())
                    .unwrap_or_default();

                if let Some(pr) = parent_row {
                    if let Ok(parent_local) = crate::persist::order_repo::order_row_to_local(&pr) {
                        let mut tp = None;
                        let mut sl = None;
                        for child in &children {
                            if let Ok(child_local) = crate::persist::order_repo::order_row_to_local(child) {
                                match child.bracket_role.as_deref() {
                                    Some("TAKE_PROFIT") => tp = Some(child_local),
                                    Some("STOP_LOSS") => sl = Some(child_local),
                                    _ => {}
                                }
                            }
                        }
                        let group = BracketGroup { parent: parent_local, take_profit: tp, stop_loss: sl };
                        Some((derive_bracket_status(&group), group.parent.avg_fill_price))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((status, entry_fill_price)) = lifecycle {
                self.bracket_status_cache.insert(parent_id, status);
                let _ = self.order_event_tx.send(BrokerEvent::BracketStatusChanged {
                    parent_id,
                    status,
                    entry_fill_price,
                });
                if status.is_terminal() {
                    self.terminal_cleanups.push_back((std::time::Instant::now(), parent_id));
                }
            }
            tracing::info!("Bracket {parent_id} cancelled ({} children)", children.len());
        } else {
            tracing::warn!("CancelBracket: no store available");
        }
    }

    // ── Modify Bracket Leg Handling ────────────────────────────────────

    /// Handle ModifyBracketLeg: modify a single leg's price.
    fn handle_modify_bracket_leg(&mut self, order_id: Uuid, new_price: f64) {
        if let Some(ref store) = self.store {
            // 1. Acquire lock, read data, update DB, drop lock
            let row = {
                let conn = store.conn().lock().expect("db mutex poisoned");

                let row = match crate::persist::order_repo::get_order(&conn, &order_id.to_string()) {
                    Ok(Some(row)) => row,
                    Ok(None) => {
                        tracing::warn!("ModifyBracketLeg: order {order_id} not found");
                        return;
                    }
                    Err(e) => {
                        tracing::error!("ModifyBracketLeg: DB error: {e}");
                        return;
                    }
                };

                // Verify this is a bracket child (TP or SL)
                let role = row.bracket_role.as_deref();
                if role != Some("TAKE_PROFIT") && role != Some("STOP_LOSS") {
                    tracing::warn!("ModifyBracketLeg: order {order_id} is not a bracket child (role={role:?})");
                    return;
                }

                // Verify order is in a modifiable state
                let modifiable = ["PreSubmitted", "Submitted", "PartiallyFilled"];
                if !modifiable.contains(&row.status.as_str()) {
                    tracing::warn!("ModifyBracketLeg: order {order_id} is not modifiable (status={})", row.status);
                    return;
                }

                // Update price in DB (W6: handle StopLimit with both prices)
                let now = chrono::Utc::now().to_rfc3339();
                if role == Some("TAKE_PROFIT") {
                    let sql = "UPDATE orders SET limit_price = ?1, updated_at = ?2 WHERE local_id = ?3";
                    if let Err(e) = conn.execute(sql, rusqlite::params![new_price, now, order_id.to_string()]) {
                        tracing::error!("ModifyBracketLeg: DB update failed: {e}");
                        return;
                    }
                } else {
                    // STOP_LOSS: for STP LMT, update both stop_price and limit_price
                    if row.order_type == "STP LMT" {
                        let sql = "UPDATE orders SET stop_price = ?1, limit_price = ?2, updated_at = ?3 WHERE local_id = ?4";
                        if let Err(e) = conn.execute(sql, rusqlite::params![new_price, new_price, now, order_id.to_string()]) {
                            tracing::error!("ModifyBracketLeg: DB update failed: {e}");
                            return;
                        }
                    } else {
                        let sql = "UPDATE orders SET stop_price = ?1, updated_at = ?2 WHERE local_id = ?3";
                        if let Err(e) = conn.execute(sql, rusqlite::params![new_price, now, order_id.to_string()]) {
                            tracing::error!("ModifyBracketLeg: DB update failed: {e}");
                            return;
                        }
                    }
                }

                let price_field = if role == Some("TAKE_PROFIT") { "limit_price" } else { "stop_price" };
                let _ = crate::persist::order_repo::write_audit(
                    &conn, &order_id.to_string(), &row.status, &row.status,
                    Some(&format!("{price_field} changed to {new_price}")), "engine"
                );

                row
                // MutexGuard drops here
            };

            // 2. Make IB client calls (no lock held)
            let role = row.bracket_role.as_deref();
            let price_field = if role == Some("TAKE_PROFIT") { "limit_price" } else { "stop_price" };

            if let Some(ref client) = self.client {
                if let Some(ib_id) = row.ib_order_id {
                    // C2: Look up parent's ib_order_id from DB using parent_id UUID
                    let parent_ib_id: Option<i32> = row.parent_id.as_ref().and_then(|pid| {
                        let conn = store.conn().lock().expect("db mutex poisoned");
                        crate::persist::order_repo::get_order(&conn, pid)
                            .ok()
                            .flatten()
                            .and_then(|parent_row| parent_row.ib_order_id)
                    });

                    // W6: For StopLimit, send both stop_price and limit_price to IB
                    let (limit, stop) = if role == Some("TAKE_PROFIT") {
                        (Some(new_price), None)
                    } else if row.order_type == "STP LMT" {
                        // StopLimit: update both prices
                        (Some(new_price), Some(new_price))
                    } else {
                        (None, Some(new_price))
                    };
                    if let Err(e) = client.place_order(
                        ib_id,
                        &row.symbol,
                        &row.action,
                        &row.order_type,
                        row.quantity,
                        limit,
                        stop,
                        parent_ib_id,
                        true,
                        &row.tif,
                        row.outside_rth,
                    ) {
                        tracing::error!("ModifyBracketLeg: broker modify failed: {e}");
                    }
                }
            }
            tracing::info!("Bracket leg {order_id} {price_field} modified to {new_price}");
        } else {
            tracing::warn!("ModifyBracketLeg: no store available");
        }
    }

    // ── Cancel Order Handling ──────────────────────────────────────────

    /// Handle CancelOrder: cancel a single standalone order by its local UUID.
    fn handle_cancel_order(&mut self, order_id: Uuid) {
        if let Some(ref store) = self.store {
            let conn = store.conn().lock().expect("db mutex poisoned");

            let row = match crate::persist::order_repo::get_order(&conn, &order_id.to_string()) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    tracing::warn!("CancelOrder: order {order_id} not found");
                    return;
                }
                Err(e) => {
                    tracing::error!("CancelOrder: DB error: {e}");
                    return;
                }
            };

            // Only cancel non-terminal orders
            if ["Filled", "Cancelled", "Rejected", "Error"].contains(&row.status.as_str()) {
                tracing::warn!("CancelOrder: order {order_id} is already terminal ({})", row.status);
                return;
            }

            // Cancel at broker
            if let Some(ref client) = self.client {
                if let Some(ib_id) = row.ib_order_id {
                    if let Err(e) = client.cancel_order(ib_id) {
                        tracing::error!("CancelOrder: broker cancel failed for {order_id}: {e}");
                    }
                }
            }

            // Update DB
            let now = chrono::Utc::now().to_rfc3339();
            let _ = crate::persist::order_repo::update_order_status(
                &conn, &order_id.to_string(), "PendingCancel", &now,
            );
            let _ = crate::persist::order_repo::write_audit(
                &conn, &order_id.to_string(), &row.status, "PendingCancel",
                Some("user cancel"), "engine",
            );

            let _ = self.order_event_tx.send(BrokerEvent::OrderStatusChanged {
                order_id,
                old_status: row.status,
                new_status: "PendingCancel".to_string(),
                filled_qty: row.filled_qty,
                remaining_qty: row.remaining_qty,
                avg_fill_price: row.avg_fill_price.unwrap_or(0.0),
            });
        }
    }

    // ── Modify Order Handling ─────────────────────────────────────────

    /// Handle ModifyOrder: modify price or quantity of a standalone order.
    fn handle_modify_order(&mut self, order_id: Uuid, new_price: Option<f64>, new_qty: Option<f64>) {
        if let Some(ref store) = self.store {
            let row = {
                let conn = store.conn().lock().expect("db mutex poisoned");

                let row = match crate::persist::order_repo::get_order(&conn, &order_id.to_string()) {
                    Ok(Some(row)) => row,
                    Ok(None) => {
                        tracing::warn!("ModifyOrder: order {order_id} not found");
                        return;
                    }
                    Err(e) => {
                        tracing::error!("ModifyOrder: DB error: {e}");
                        return;
                    }
                };

                // Only modify live orders
                let modifiable = ["PreSubmitted", "Submitted", "PartiallyFilled"];
                if !modifiable.contains(&row.status.as_str()) {
                    tracing::warn!("ModifyOrder: order {order_id} not modifiable ({})", row.status);
                    return;
                }

                // Update DB
                let now = chrono::Utc::now().to_rfc3339();
                if let Some(price) = new_price {
                    let sql = match row.order_type.as_str() {
                        "STP" | "STP LMT" => "UPDATE orders SET stop_price = ?1, updated_at = ?2 WHERE local_id = ?3",
                        _ => "UPDATE orders SET limit_price = ?1, updated_at = ?2 WHERE local_id = ?3",
                    };
                    let _ = conn.execute(sql, rusqlite::params![price, now, order_id.to_string()]);
                }
                if let Some(qty) = new_qty {
                    let sql = "UPDATE orders SET quantity = ?1, remaining_qty = ?2, updated_at = ?3 WHERE local_id = ?4";
                    let remaining = qty - row.filled_qty;
                    let _ = conn.execute(sql, rusqlite::params![qty, remaining, now, order_id.to_string()]);
                }

                let details = format!(
                    "modified: price={:?}, qty={:?}",
                    new_price, new_qty,
                );
                let _ = crate::persist::order_repo::write_audit(
                    &conn, &order_id.to_string(), &row.status, &row.status,
                    Some(&details), "engine",
                );

                row
            };

            // Re-submit to broker with updated values
            if let Some(ref client) = self.client {
                if let Some(ib_id) = row.ib_order_id {
                    let limit = new_price.or(row.limit_price);
                    let stop = if matches!(row.order_type.as_str(), "STP" | "STP LMT") {
                        new_price.or(row.stop_price)
                    } else {
                        row.stop_price
                    };
                    let qty = new_qty.unwrap_or(row.quantity);

                    if let Err(e) = client.place_order(
                        ib_id,
                        &row.symbol,
                        &row.action,
                        &row.order_type,
                        qty,
                        limit,
                        stop,
                        None,
                        true,
                        &row.tif,
                        row.outside_rth,
                    ) {
                        tracing::error!("ModifyOrder: broker modify failed for {order_id}: {e}");
                    }
                }
            }

            tracing::info!("Order {order_id} modified: price={new_price:?}, qty={new_qty:?}");
        }
    }

    // ── Order Snapshot Handling ────────────────────────────────────────

    /// Handle RequestOrderSnapshot: emit current state of all tracked orders.
    fn handle_request_order_snapshot(&self) {
        if let Some(ref store) = self.store {
            let conn = store.conn().lock().expect("db mutex poisoned");

            // Emit all non-terminal orders as status events for UI sync
            let active_statuses = [
                "Inactive", "PendingSubmit", "PreSubmitted", "Submitted",
                "PartiallyFilled", "PendingCancel",
            ];

            for status_str in &active_statuses {
                if let Ok(rows) = crate::persist::order_repo::get_orders_by_status(&conn, status_str) {
                    for row in &rows {
                        if let Ok(order_id) = row.local_id.parse::<Uuid>() {
                            let _ = self.order_event_tx.send(BrokerEvent::OrderStatusChanged {
                                order_id,
                                old_status: row.status.clone(),
                                new_status: row.status.clone(),
                                filled_qty: row.filled_qty,
                                remaining_qty: row.remaining_qty,
                                avg_fill_price: row.avg_fill_price.unwrap_or(0.0),
                            });
                        }
                    }
                }
            }
        }
    }

    // ── Broker Callback Translation ──────────────────────────────────

    /// Translate a broker callback into BrokerEvents.
    ///
    /// Called from the poll loop for each callback returned by
    /// `BrokerClient::poll_callbacks()`. Maps IB order IDs to local UUIDs,
    /// validates status transitions, updates the DB, and emits events.
    fn handle_broker_callback(&mut self, cb: BrokerCallback) {
        match cb {
            BrokerCallback::OrderStatus {
                ib_order_id,
                status,
                filled,
                remaining,
                avg_fill_price,
            } => {
                // Look up local UUID via ib_to_local map
                let local_id = match self.ib_to_local.get(&ib_order_id) {
                    Some(&id) => id,
                    None => {
                        tracing::warn!("Callback for unknown IB order ID {ib_order_id}");
                        return;
                    }
                };

                // Map IB status string to our canonical OrderStatus
                let new_status = OrderStatus::from_ib_status(&status);

                // Bracket info extracted from DB row before dropping the lock,
                // so we can call check_bracket_status_change() afterwards.
                let mut bracket_info: Option<(Option<String>, Option<String>)> = None;

                // Update DB if store available
                if let Some(ref store) = self.store {
                    let conn = store.conn().lock().expect("db mutex poisoned");
                    let now = chrono::Utc::now().to_rfc3339();

                    // Get current order row for transition validation
                    if let Ok(Some(row)) =
                        crate::persist::order_repo::get_order(&conn, &local_id.to_string())
                    {
                        let old_status_str = row.status.clone();

                        // W2: Idempotency guard — skip duplicate status updates
                        if let Ok(old_status) = old_status_str.parse::<OrderStatus>() {
                            if new_status == old_status {
                                tracing::debug!("Ignoring duplicate status {new_status} for order {local_id}");
                                return;
                            }

                            // Validate transition (log warning but proceed — IB is authoritative)
                            if let Err(e) =
                                OrderStatus::validate_transition(old_status, new_status)
                            {
                                tracing::warn!(
                                    "Invalid status transition for order {local_id}: {e}"
                                );
                            }
                        }

                        // Update status and audit in a transaction
                        let id_str = local_id.to_string();
                        let txn_result = (|| -> Result<(), rusqlite::Error> {
                            conn.execute_batch("BEGIN")?;
                            crate::persist::order_repo::update_order_status(
                                &conn,
                                &id_str,
                                &new_status.to_string(),
                                &now,
                            )?;
                            crate::persist::order_repo::write_audit(
                                &conn,
                                &id_str,
                                &old_status_str,
                                &new_status.to_string(),
                                None,
                                "ib_callback",
                            )?;
                            conn.execute_batch("COMMIT")?;
                            Ok(())
                        })();
                        if let Err(e) = txn_result {
                            tracing::error!(
                                "Failed to persist status change for order {local_id}: {e}"
                            );
                            let _ = conn.execute_batch("ROLLBACK");
                        }

                        // Emit event
                        let _ = self.order_event_tx.send(BrokerEvent::OrderStatusChanged {
                            order_id: local_id,
                            old_status: old_status_str,
                            new_status: new_status.to_string(),
                            filled_qty: filled,
                            remaining_qty: remaining,
                            avg_fill_price,
                        });

                        // Stash bracket info for post-lock bracket status check
                        if row.bracket_role.is_some() {
                            bracket_info =
                                Some((row.bracket_role.clone(), row.parent_id.clone()));
                        }
                    }
                    // MutexGuard (conn) drops here
                }

                // Note: We intentionally keep ib_to_local mappings for terminal orders
                // because IB may send Execution callbacks after the terminal OrderStatus.
                // Cleanup happens on engine shutdown or periodic sweep.

                // Check bracket status after releasing the DB lock
                if let Some((role, parent_id_str)) = bracket_info {
                    self.check_bracket_status_change(
                        local_id,
                        role.as_deref(),
                        parent_id_str.as_deref(),
                    );
                }
            }

            BrokerCallback::Execution {
                ib_order_id,
                exec_id,
                shares,
                price,
                commission,
                side,
            } => {
                let local_id = match self.ib_to_local.get(&ib_order_id) {
                    Some(&id) => id,
                    None => {
                        tracing::warn!("Execution callback for unknown IB order ID {ib_order_id}");
                        return;
                    }
                };

                // Persist fill to DB
                if let Some(ref store) = self.store {
                    let conn = store.conn().lock().expect("db mutex poisoned");
                    let now = chrono::Utc::now().to_rfc3339();
                    let fill = crate::persist::order_repo::FillRow {
                        order_local_id: local_id.to_string(),
                        ib_exec_id: exec_id.clone(),
                        timestamp: now,
                        shares,
                        price,
                        commission: Some(commission),
                        exchange: None,
                        side: side.clone(),
                    };
                    if let Err(e) = crate::persist::order_repo::insert_fill(&conn, &fill) {
                        tracing::error!("Failed to persist fill for order {local_id}: {e}");
                    }
                }

                // Emit fill event
                let _ = self.order_event_tx.send(BrokerEvent::OrderFilled {
                    order_id: local_id,
                    ib_exec_id: exec_id,
                    shares,
                    price,
                    commission: Some(commission),
                });
            }

            BrokerCallback::OrderRejected {
                ib_order_id,
                reason,
            } => {
                let local_id = match self.ib_to_local.get(&ib_order_id) {
                    Some(&id) => id,
                    None => {
                        tracing::warn!(
                            "Rejection callback for unknown IB order ID {ib_order_id}"
                        );
                        return;
                    }
                };

                // Update DB status to Rejected
                if let Some(ref store) = self.store {
                    let conn = store.conn().lock().expect("db mutex poisoned");
                    let now = chrono::Utc::now().to_rfc3339();

                    if let Ok(Some(row)) =
                        crate::persist::order_repo::get_order(&conn, &local_id.to_string())
                    {
                        let _ = crate::persist::order_repo::update_order_status(
                            &conn,
                            &local_id.to_string(),
                            &OrderStatus::Rejected.to_string(),
                            &now,
                        );
                        let _ = crate::persist::order_repo::write_audit(
                            &conn,
                            &local_id.to_string(),
                            &row.status,
                            &OrderStatus::Rejected.to_string(),
                            Some(&reason),
                            "ib_callback",
                        );
                    }
                }

                let _ = self.order_event_tx.send(BrokerEvent::OrderRejected {
                    order_id: local_id,
                    reason,
                });
            }

            BrokerCallback::ConnectionStatus {
                connected,
                server_version,
            } => {
                tracing::info!(
                    "Connection status changed: connected={connected}, server_version={server_version:?}"
                );
                if connected {
                    if let Some(ver) = server_version {
                        let _ = self.order_event_tx.send(BrokerEvent::Connected {
                            server_version: ver,
                        });
                    }
                } else {
                    let _ = self.order_event_tx.send(BrokerEvent::Disconnected {
                        reason: "broker connection lost".to_string(),
                    });
                }
            }

            BrokerCallback::Tick {
                symbol,
                con_id,
                bid,
                ask,
                last,
                volume,
            } => {
                let _ = self.market_event_tx.send(BrokerEvent::Tick {
                    symbol: midas_core::SymbolKey {
                        contract_id: con_id,
                        symbol,
                    },
                    bid,
                    ask,
                    last,
                    volume,
                    timestamp: chrono::Utc::now(),
                });
            }

            BrokerCallback::BarUpdated {
                symbol,
                timestamp,
                open,
                high,
                low,
                close,
                volume,
            } => {
                let _ = self.market_event_tx.send(BrokerEvent::BarUpdated {
                    symbol: midas_core::SymbolKey {
                        contract_id: 0,
                        symbol,
                    },
                    timestamp,
                    open,
                    high,
                    low,
                    close,
                    volume,
                });
            }

            BrokerCallback::BarClosed {
                symbol,
                timestamp,
                open,
                high,
                low,
                close,
                volume,
            } => {
                let _ = self.market_event_tx.send(BrokerEvent::BarClosed {
                    symbol: midas_core::SymbolKey {
                        contract_id: 0,
                        symbol,
                    },
                    timestamp,
                    open,
                    high,
                    low,
                    close,
                    volume,
                });
            }

            BrokerCallback::Position { symbol, quantity, avg_cost } => {
                let _ = self.order_event_tx.send(BrokerEvent::PositionUpdate {
                    account: String::new(),
                    symbol,
                    con_id: 0,
                    quantity,
                    avg_cost,
                });
            }

            BrokerCallback::Account { cash_balance, unrealized_pnl, realized_pnl } => {
                let _ = self.order_event_tx.send(BrokerEvent::PnlUpdate {
                    daily_pnl: 0.0,
                    unrealized_pnl,
                    realized_pnl,
                });
                let _ = self.order_event_tx.send(BrokerEvent::AccountValueUpdate {
                    account: String::new(),
                    key: "CashBalance".to_string(),
                    value: format!("{cash_balance:.2}"),
                    currency: "USD".to_string(),
                });
            }
        }
    }

    // ── Bracket Status Change Detection ───────────────────────────────

    /// Post-processing: check if a bracket's lifecycle status has changed.
    /// Called after any individual order status update for bracket members.
    fn check_bracket_status_change(&mut self, order_id: Uuid, bracket_role: Option<&str>, parent_id_str: Option<&str>) {
        // Determine the bracket's parent_id
        let parent_id = match bracket_role {
            Some("PARENT") => order_id,
            Some(_) => {
                match parent_id_str.and_then(|s| s.parse::<Uuid>().ok()) {
                    Some(pid) => pid,
                    None => return,
                }
            }
            None => return, // Not a bracket member
        };

        if let Some(ref store) = self.store {
            let conn = store.conn().lock().expect("db mutex poisoned");

            // Load parent
            let parent_row = match crate::persist::order_repo::get_order(&conn, &parent_id.to_string()) {
                Ok(Some(row)) => row,
                _ => return,
            };

            // Load children
            let children = match crate::persist::order_repo::get_orders_by_parent_id(&conn, &parent_id.to_string()) {
                Ok(rows) => rows,
                _ => return,
            };

            // Convert to LocalOrder for derive_bracket_status
            let parent_local = match crate::persist::order_repo::order_row_to_local(&parent_row) {
                Ok(o) => o,
                Err(_) => return,
            };

            let mut tp = None;
            let mut sl = None;
            for child in &children {
                if let Ok(child_local) = crate::persist::order_repo::order_row_to_local(child) {
                    match child.bracket_role.as_deref() {
                        Some("TAKE_PROFIT") => tp = Some(child_local),
                        Some("STOP_LOSS") => sl = Some(child_local),
                        _ => {}
                    }
                }
            }

            let group = BracketGroup { parent: parent_local, take_profit: tp, stop_loss: sl };
            let new_status = derive_bracket_status(&group);

            // Compare with cached status
            let prev = self.bracket_status_cache.get(&parent_id).copied();
            if prev != Some(new_status) {
                self.bracket_status_cache.insert(parent_id, new_status);
                let _ = self.order_event_tx.send(BrokerEvent::BracketStatusChanged {
                    parent_id,
                    status: new_status,
                    entry_fill_price: group.parent.avg_fill_price,
                });
                tracing::info!("Bracket {parent_id} status changed: {prev:?} -> {new_status}");

                // Queue deferred cleanup for terminal brackets. We keep the cache
                // entries for 60s to absorb late IB callbacks, then sweep.
                if new_status.is_terminal() {
                    self.terminal_cleanups.push_back((std::time::Instant::now(), parent_id));
                }
            }
        }
    }

    // ── Reconnect Logic ────────────────────────────────────────────────

    /// Check if the broker client has disconnected and attempt reconnection
    /// with exponential backoff per the `ReconnectConfig`.
    async fn check_reconnect(&mut self) {
        // Only reconnect for live data source
        if !matches!(self.config.data_source, DataSourceConfig::Live) {
            return;
        }

        let connected = self.client.as_ref().is_some_and(|c| c.is_connected());

        if connected {
            if !self.was_connected {
                // Just reconnected
                self.was_connected = true;
                self.reconnect_attempt = 0;
                let _ = self.conn_state_tx.send(ConnectionState::Ready);
                let _ = self.order_event_tx.send(BrokerEvent::Reconnected);
                tracing::info!("Reconnected to broker");

                // Request order snapshot for reconciliation
                self.handle_request_order_snapshot();
            }
            return;
        }

        // Not connected
        if self.was_connected {
            // Just lost connection
            self.was_connected = false;
            let _ = self.conn_state_tx.send(ConnectionState::Disconnected);
            tracing::warn!("Lost connection to broker");
        }

        let cfg = &self.config.reconnect;
        if self.reconnect_attempt >= cfg.max_retries {
            return; // Exhausted retries
        }

        self.reconnect_attempt += 1;
        let delay = std::cmp::min(
            cfg.initial_delay_secs * 2u64.saturating_pow(self.reconnect_attempt.saturating_sub(1)),
            cfg.max_delay_secs,
        );

        let _ = self.conn_state_tx.send(ConnectionState::Reconnecting {
            attempt: self.reconnect_attempt,
        });
        let _ = self.order_event_tx.send(BrokerEvent::Reconnecting {
            attempt: self.reconnect_attempt,
            next_retry_secs: delay,
        });

        tracing::info!(
            "Reconnect attempt {}/{} (delay {}s)",
            self.reconnect_attempt,
            cfg.max_retries,
            delay,
        );

        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

        if let Some(ref client) = self.client {
            match client.connect() {
                Ok(ver) => {
                    tracing::info!("Reconnected (server version {ver})");
                    // was_connected will be set on next heartbeat check
                }
                Err(e) => {
                    tracing::warn!("Reconnect attempt {} failed: {e}", self.reconnect_attempt);
                }
            }
        }
    }

    /// Remove stale entries from bracket_status_cache, ib_to_local, and
    /// bracket_ib_ids for brackets that reached terminal status > 60s ago.
    fn sweep_terminal_brackets(&mut self) {
        let cutoff = std::time::Duration::from_secs(60);
        while let Some(&(ts, parent_id)) = self.terminal_cleanups.front() {
            if ts.elapsed() < cutoff {
                break; // Queue is ordered by time; everything after this is newer.
            }
            self.terminal_cleanups.pop_front();
            self.bracket_status_cache.remove(&parent_id);
            if let Some(ib_ids) = self.bracket_ib_ids.remove(&parent_id) {
                for ib_id in ib_ids {
                    self.ib_to_local.remove(&ib_id);
                }
            }
        }
    }
}

// ===========================================================================
// Bracket Builder (free function, testable without engine)
// ===========================================================================

/// Build a BracketGroup from MarketBracketParams.
///
/// All orders start in Inactive status. The engine transitions them to
/// PendingSubmit in a single atomic DB transaction before IB submission.
fn build_market_bracket(params: &MarketBracketParams) -> BracketGroup {
    let parent_id = Uuid::now_v7();

    // -- Parent: Market Order --
    let mut parent = LocalOrder::new_draft(
        &params.symbol,
        params.action,
        OrderKind::Market,
        params.quantity,
    );
    parent.id = parent_id;
    parent.status = OrderStatus::Inactive;
    parent.con_id = params.con_id;
    parent.sec_type = params.sec_type;
    parent.exchange = params.exchange.clone();
    parent.currency = params.currency.clone();
    parent.outside_rth = params.outside_rth;
    parent.bracket_role = Some(BracketRole::Parent);
    parent.strategy = params.strategy.clone();
    parent.tags = params.tags.clone();

    // -- Take Profit: Limit Order (opposite side) --
    let take_profit = params.take_profit.as_ref().map(|tp| {
        let opposite = match params.action {
            OrderAction::Buy => OrderAction::Sell,
            OrderAction::Sell => OrderAction::Buy,
        };
        let mut order = LocalOrder::new_draft(
            &params.symbol,
            opposite,
            OrderKind::Limit,
            params.quantity,
        );
        order.status = OrderStatus::Inactive;
        order.con_id = params.con_id;
        order.sec_type = params.sec_type;
        order.exchange = params.exchange.clone();
        order.currency = params.currency.clone();
        order.limit_price = Some(tp.price);
        order.tif = tp.tif.unwrap_or(TimeInForce::Gtc);
        order.parent_id = Some(parent_id);
        order.bracket_role = Some(BracketRole::TakeProfit);
        order.strategy = params.strategy.clone();
        order.tags = params.tags.clone();
        order
    });

    // -- Stop Loss: Stop or StopLimit Order (opposite side) --
    let stop_loss = params.stop_loss.as_ref().map(|sl| {
        let opposite = match params.action {
            OrderAction::Buy => OrderAction::Sell,
            OrderAction::Sell => OrderAction::Buy,
        };
        let kind = if sl.limit_price.is_some() {
            OrderKind::StopLimit
        } else {
            OrderKind::Stop
        };
        let mut order = LocalOrder::new_draft(
            &params.symbol,
            opposite,
            kind,
            params.quantity,
        );
        order.status = OrderStatus::Inactive;
        order.con_id = params.con_id;
        order.sec_type = params.sec_type;
        order.exchange = params.exchange.clone();
        order.currency = params.currency.clone();
        order.stop_price = Some(sl.stop_price);
        order.limit_price = sl.limit_price;
        order.tif = sl.tif.unwrap_or(TimeInForce::Gtc);
        order.parent_id = Some(parent_id);
        order.bracket_role = Some(BracketRole::StopLoss);
        order.strategy = params.strategy.clone();
        order.tags = params.tags.clone();
        order
    });

    BracketGroup { parent, take_profit, stop_loss }
}

// ===========================================================================
// Order Size Guard
// ===========================================================================

/// Order size validation errors.
#[derive(Debug)]
pub enum OrderSizeError {
    QuantityExceedsLimit { quantity: f64, limit: f64 },
    NotionalExceedsLimit { notional: f64, limit: f64 },
    MissingReferencePrice { symbol: String },
}

impl std::fmt::Display for OrderSizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuantityExceedsLimit { quantity, limit } => {
                write!(f, "quantity {quantity} exceeds limit {limit}")
            }
            Self::NotionalExceedsLimit { notional, limit } => {
                write!(f, "notional ${notional:.0} exceeds limit ${limit:.0}")
            }
            Self::MissingReferencePrice { symbol } => {
                write!(f, "no reference price for {symbol} — notional guard requires price")
            }
        }
    }
}

/// Engine-level order size guard. Hard reject — not bypassable from UI.
/// Runs before build_market_bracket().
fn validate_order_size(
    params: &MarketBracketParams,
    limits: &TradingLimits,
) -> Result<(), OrderSizeError> {
    if limits.max_order_quantity > 0.0 && params.quantity > limits.max_order_quantity {
        return Err(OrderSizeError::QuantityExceedsLimit {
            quantity: params.quantity,
            limit: limits.max_order_quantity,
        });
    }

    if limits.max_notional_value > 0.0 {
        match params.reference_price {
            Some(price) => {
                let notional = params.quantity * price;
                if notional > limits.max_notional_value {
                    return Err(OrderSizeError::NotionalExceedsLimit {
                        notional,
                        limit: limits.max_notional_value,
                    });
                }
            }
            None => {
                return Err(OrderSizeError::MissingReferencePrice {
                    symbol: params.symbol.clone(),
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
