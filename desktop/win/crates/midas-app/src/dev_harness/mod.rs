//! In-process dev harness: TCP socket that Claude Code (or any client)
//! drives with newline-delimited JSON commands defined in
//! [`midas_devloop_proto`].
//!
//! Compiled only when the `dev_harness` Cargo feature is active. See
//! `plan/devloop-spec.md` for the full design.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use iced::futures::Stream;
use midas_devloop_proto::{Command, ErrorKind, Response};
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::app::Message;

pub mod broker_inject;
pub mod dump;
pub mod event_log;
pub mod fixture;
pub mod idle;
pub mod inject;
pub mod input;
pub mod listener;
pub mod screenshot;
pub mod variant_names;

// ── Responder ─────────────────────────────────────────────────────────

/// Single-use reply channel threaded through `Message::DevHarness` so the
/// iced update loop can answer a harness command.
///
/// Wrapped in an `Arc<Mutex<Option<_>>>` because `Message` must be
/// `Clone`. The first call to [`Responder::send`] takes the sender;
/// subsequent clones are no-ops. That is acceptable: only one side of
/// the update pipeline ever answers a given command.
#[derive(Debug, Clone)]
pub struct Responder(Arc<Mutex<Option<oneshot::Sender<Response>>>>);

impl Responder {
    pub fn new(tx: oneshot::Sender<Response>) -> Self {
        Self(Arc::new(Mutex::new(Some(tx))))
    }

    /// Send a response back to the waiting client. No-op if already sent
    /// or if the client dropped the connection.
    pub fn send(&self, response: Response) {
        if let Some(tx) = self.0.lock().take() {
            let _ = tx.send(response);
        }
    }

    /// Convenience helper for a successful response with no structured
    /// body.
    pub fn ok_empty(&self, log_cursor: u64) {
        self.send(Response::Ok {
            body: serde_json::Value::Null,
            log_cursor,
        });
    }

    /// Convenience helper for a successful response with a JSON body.
    pub fn ok(&self, body: serde_json::Value, log_cursor: u64) {
        self.send(Response::Ok { body, log_cursor });
    }

    /// Convenience helper for an error response.
    pub fn err(&self, kind: ErrorKind, message: impl Into<String>, log_cursor: u64) {
        self.send(Response::Error {
            kind,
            message: message.into(),
            log_cursor,
        });
    }
}

// ── Subscription source ───────────────────────────────────────────────

/// Source for the harness subscription. Opaque wrapper that implements
/// [`Hash`] so iced can deduplicate; carries just the port because the
/// actual listener is spawned from inside the stream.
#[derive(Clone)]
pub struct DevHarnessSource {
    pub port: u16,
}

impl Hash for DevHarnessSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "dev-harness-source".hash(state);
        self.port.hash(state);
    }
}

/// Build the stream that feeds `Message::DevHarness` into iced from the
/// TCP listener. Runs on iced's tokio runtime, so `tokio::spawn` works
/// inside.
pub fn dev_harness_stream(source: &DevHarnessSource) -> impl Stream<Item = Message> {
    let port = source.port;
    iced::stream::channel(64, move |output| listener::run(port, output))
}

// ── Command dispatch ──────────────────────────────────────────────────

/// Entry point called from `MidasApp::update` when a `DevHarness` message
/// arrives. Returns any follow-up `Task` the command produces (e.g.
/// shutdown).
pub fn handle_command(
    command: Command,
    responder: Responder,
    app: &mut crate::app::MidasApp,
) -> iced::Task<Message> {
    let cursor_now = || event_log::try_global().map(|l| l.cursor()).unwrap_or(0);

    match command {
        Command::Ping => {
            responder.ok(
                serde_json::json!({ "pid": std::process::id() }),
                cursor_now(),
            );
            iced::Task::none()
        }

        Command::Shutdown => {
            responder.ok_empty(cursor_now());
            tracing::info!("devloop: Shutdown received, exiting");
            iced::exit()
        }

        Command::WaitForEvent {
            event_type,
            timeout_ms,
            since_cursor,
        } => {
            let Some(log) = event_log::try_global() else {
                responder.err(
                    ErrorKind::Internal,
                    "event log not initialised",
                    cursor_now(),
                );
                return iced::Task::none();
            };
            let since = since_cursor.unwrap_or(0);
            let timeout = Duration::from_millis(timeout_ms);
            iced::Task::perform(
                async move {
                    let outcome = log.wait_for_event(&event_type, since, timeout).await;
                    let cursor = log.cursor();
                    match outcome {
                        Some(matched) => {
                            responder.ok(serde_json::json!({ "matched_cursor": matched }), cursor)
                        }
                        None => responder.err(
                            ErrorKind::Timeout,
                            format!("no event {event_type} within {timeout_ms}ms"),
                            cursor,
                        ),
                    }
                },
                |_| Message::Tick,
            )
        }

        Command::WaitForIdle { timeout_ms } => {
            let Some(tracker) = idle::try_global() else {
                responder.err(
                    ErrorKind::Internal,
                    "idle tracker not initialised",
                    cursor_now(),
                );
                return iced::Task::none();
            };
            let timeout = Duration::from_millis(timeout_ms);
            iced::Task::perform(
                async move {
                    let idle_ok = tracker.wait_until_idle(timeout).await;
                    let cursor = event_log::try_global().map(|l| l.cursor()).unwrap_or(0);
                    if idle_ok {
                        responder.ok_empty(cursor);
                    } else {
                        responder.err(
                            ErrorKind::Timeout,
                            format!("not idle after {timeout_ms}ms"),
                            cursor,
                        );
                    }
                },
                |_| Message::Tick,
            )
        }

        Command::LoadFixture { name } => match fixture::apply_from_disk(app, &name) {
            Ok(tasks) => {
                responder.ok(serde_json::json!({ "name": name }), cursor_now());
                tasks
            }
            Err(crate::app::FixtureError::NotFound(_)) => {
                responder.err(
                    ErrorKind::FixtureNotFound,
                    format!("fixture not found: {name}"),
                    cursor_now(),
                );
                iced::Task::none()
            }
            Err(crate::app::FixtureError::VersionMismatch {
                field,
                expected,
                got,
            }) => {
                responder.err(
                    ErrorKind::FixtureVersionMismatch,
                    format!("{field}: expected {expected}, got {got}"),
                    cursor_now(),
                );
                iced::Task::none()
            }
            Err(e) => {
                responder.err(ErrorKind::Internal, e.to_string(), cursor_now());
                iced::Task::none()
            }
        },

        Command::SnapshotFixture { name, note } => {
            match fixture::snapshot_to_disk(app, &name, note) {
                Ok(path) => responder.ok(
                    serde_json::json!({ "path": path.display().to_string() }),
                    cursor_now(),
                ),
                Err(e) => responder.err(ErrorKind::Internal, e.to_string(), cursor_now()),
            }
            iced::Task::none()
        }

        Command::DumpState { path } => {
            let projection = dump::build(app);
            let body = match path.as_deref() {
                Some(p) => match dump::walk_path(&projection, p) {
                    Some(v) => v.clone(),
                    None => {
                        responder.err(
                            ErrorKind::Internal,
                            format!("path not found: {p}"),
                            cursor_now(),
                        );
                        return iced::Task::none();
                    }
                },
                None => projection,
            };
            responder.ok(body, cursor_now());
            iced::Task::none()
        }

        Command::Screenshot { out_path } => {
            let Some(main_id) = app.main_window else {
                responder.err(
                    ErrorKind::Internal,
                    "main window not yet created",
                    cursor_now(),
                );
                return iced::Task::none();
            };
            iced::window::screenshot(main_id).map(move |screenshot| {
                Message::DevHarnessScreenshotReady {
                    screenshot,
                    out_path: out_path.clone(),
                    responder: responder.clone(),
                }
            })
        }

        Command::InjectTickerMsg { symbol, msg_json } => match inject::parse(&msg_json) {
            Ok(msg) => {
                let variant = variant_names::ticker_msg_variant(&msg);
                responder.ok(
                    serde_json::json!({
                        "symbol": symbol,
                        "variant": variant,
                    }),
                    cursor_now(),
                );
                let key = crate::annotation_store::SymbolKey::new(&symbol);
                app.update(Message::Ticker(key, msg))
            }
            Err(e) => {
                responder.err(ErrorKind::Internal, e.to_string(), cursor_now());
                iced::Task::none()
            }
        },

        Command::InjectBrokerEvent { event_json } => match broker_inject::parse(&event_json) {
            Ok(event) => {
                let variant = variant_names::broker_event_variant(&event);
                responder.ok(serde_json::json!({ "variant": variant }), cursor_now());
                app.update(Message::BrokerEventReceived(Box::new(event)))
            }
            Err(e) => {
                responder.err(ErrorKind::Internal, e.to_string(), cursor_now());
                iced::Task::none()
            }
        },

        Command::OpenOrdersPanel => {
            responder.ok_empty(cursor_now());
            app.update(Message::AddAccountPanel)
        }

        Command::CycleThumbnail { symbol } => {
            responder.ok(serde_json::json!({ "symbol": symbol }), cursor_now());
            app.update(Message::ThumbnailIntervalCycle(symbol))
        }

        Command::SetAccountTab { tab } => {
            use crate::account_panel::AccountMsg;
            use midas_core::config::AccountTab;
            let parsed = match tab.as_str() {
                "positions" => AccountTab::Positions,
                "orders" => AccountTab::Orders,
                "trade-history" => AccountTab::TradeHistory,
                "recents" => AccountTab::Recents,
                other => {
                    responder.err(
                        ErrorKind::Internal,
                        format!(
                            "SetAccountTab: unknown tab '{other}' (expected positions|orders|\
                             trade-history|recents)"
                        ),
                        cursor_now(),
                    );
                    return iced::Task::none();
                }
            };
            // Target every Account panel currently mounted in the
            // workspace pane grid. This matters when a config has stale
            // `account_panels` entries that aren't in the visible layout
            // — scripting the active tab should affect the panel the
            // user is looking at, not an orphaned one.
            use crate::layout::PanelContent;
            let visible_ids: Vec<midas_core::AccountPanelId> = app
                .workspace
                .panes
                .iter()
                .filter_map(|(_, state)| match state.content {
                    PanelContent::Account(id) => Some(id),
                    _ => None,
                })
                .collect();
            if visible_ids.is_empty() {
                responder.err(
                    ErrorKind::Internal,
                    "SetAccountTab: no Account panel is mounted in the workspace",
                    cursor_now(),
                );
                return iced::Task::none();
            }
            responder.ok(
                serde_json::json!({
                    "account_panel_ids": visible_ids.iter().map(|i| i.0).collect::<Vec<_>>(),
                    "tab": tab,
                }),
                cursor_now(),
            );
            // Dispatch TabSelected to every mounted Account pane. For
            // scripted visual checks the common case is one pane, so
            // "all" == "the one the user sees". Iced batches the Task
            // results; each panel's state advances synchronously inside
            // the `update` call.
            let tasks: Vec<_> = visible_ids
                .into_iter()
                .map(|id| app.update(Message::Account(id, AccountMsg::TabSelected(parsed))))
                .collect();
            iced::Task::batch(tasks)
        }

        Command::Key { combo } => match input::dispatch_key(app, &combo) {
            Ok(task) => {
                responder.ok(serde_json::json!({ "combo": combo }), cursor_now());
                task
            }
            Err(e) => {
                responder.err(ErrorKind::Internal, e.to_string(), cursor_now());
                iced::Task::none()
            }
        },

        Command::Scroll { x, y, dx, dy } => match input::dispatch_scroll(app, x, y, dx, dy) {
            Ok(task) => {
                responder.ok(serde_json::json!({ "dx": dx, "dy": dy }), cursor_now());
                task
            }
            Err(e) => {
                responder.err(ErrorKind::Internal, e.to_string(), cursor_now());
                iced::Task::none()
            }
        },

        Command::Click { .. } | Command::ClickPrice { .. } | Command::Drag { .. } => {
            responder.err(
                ErrorKind::Internal,
                "click/drag injection not supported in v1 — use `inject_ticker_msg` \
                 for domain mutations. See plan/devloop-spec.md for the design rationale.",
                cursor_now(),
            );
            iced::Task::none()
        }
    }
}

// ── Startup side-effects ──────────────────────────────────────────────

/// Called once from `main()` when the feature is enabled. Ensures
/// `.devloop/` exists, writes the per-port PID file, truncates any stale
/// event log, and installs the panic hook.
pub fn init(port: u16) {
    if let Err(e) = std::fs::create_dir_all(".devloop") {
        tracing::warn!("devloop: could not create .devloop/: {e}");
        return;
    }

    let pid_path = format!(".devloop/app.{port}.pid");
    if let Err(e) = std::fs::write(&pid_path, std::process::id().to_string()) {
        tracing::warn!("devloop: could not write {pid_path}: {e}");
    }

    // Event log (truncates any existing file on open).
    match event_log::EventLog::new(".devloop/events.jsonl") {
        Ok(log) => {
            event_log::init_global(Arc::new(log));
        }
        Err(e) => tracing::error!("devloop: event log init failed: {e}"),
    }

    // Idle tracker.
    idle::init_global(Arc::new(idle::IdleTracker::new()));

    install_panic_hook();

    tracing::info!("devloop: init complete on port {port}");
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("{info}\n{:?}", std::backtrace::Backtrace::force_capture());
        if let Err(e) = std::fs::write(".devloop/panic.txt", &msg) {
            eprintln!("devloop: failed to write panic.txt: {e}");
        }
        previous(info);
    }));
}

/// Resolve the port from the `DEVLOOP_PORT` env var, falling back to the
/// proto crate's default.
pub fn resolve_port() -> u16 {
    std::env::var("DEVLOOP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(midas_devloop_proto::DEFAULT_PORT)
}

/// Called from `MidasApp::update` when a `DevHarnessScreenshotReady`
/// message arrives. Encodes the PNG, diffs against the reference (if
/// any), and fires the pending responder.
pub fn handle_screenshot_ready(
    screenshot: iced::window::Screenshot,
    out_path: std::path::PathBuf,
    responder: Responder,
) {
    let cursor = event_log::try_global().map(|l| l.cursor()).unwrap_or(0);
    match screenshot::capture(&screenshot, &out_path) {
        Ok(result) => {
            let body = serde_json::json!({
                "out_path": result.out_path.display().to_string(),
                "mirror_path": result.mirror_path.as_ref().map(|p| p.display().to_string()),
                "width": result.width,
                "height": result.height,
                "scale_factor": result.scale_factor,
                "reference_path": result.reference_path.as_ref().map(|p| p.display().to_string()),
                "diff_path": result.diff_path.as_ref().map(|p| p.display().to_string()),
                "ssim": result.ssim,
                "diff_fraction": result.diff_fraction,
            });
            responder.ok(body, cursor);
        }
        Err(e) => {
            responder.err(ErrorKind::Internal, format!("screenshot: {e}"), cursor);
        }
    }
}
