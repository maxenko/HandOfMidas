//! midas-app: Hand of Midas desktop application.
//!
//! This is the binary entry point. It creates an iced daemon (multi-window
//! capable) and wires together all workspace crates.

mod account_panel;
mod annotation_persistence;
mod annotation_store;
mod app;
#[cfg(feature = "dev_harness")]
mod chart_parity;
mod chart_view;
mod chart_widget;
mod column_resize;
#[cfg(feature = "dev_harness")]
mod dev_harness;
mod layout;
mod link;
mod market_cache;
mod order_blotter;
mod order_panel;
mod registry;
// S8 (session-aware-charts Phase B). Opt-in module behind the
// `session_chart` Cargo feature. Also declared `pub mod` in `lib.rs`
// so integration tests can reach it; in Cargo's binary + library
// dual-target model these are two separate compilations of the same
// source, which is fine — the module is sans-IO and duplicating it
// has no runtime cost.
#[cfg(feature = "session_chart")]
mod session_chart;
#[cfg(feature = "session_chart")]
mod session_chart_window;
mod sim_child;
mod theme;
mod thumbnail_data;
mod thumbnail_store;
#[path = "thumbnail_widget.rs"]
mod thumbnail_widget;
mod ticker_state;
mod toast;
mod view_models;
mod watchlist;
mod watchlist_columns;
mod window_geometry;

use app::{Message, MidasApp};
use iced::keyboard;
use iced::{window, Element, Subscription, Task, Theme};

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "midas=debug,wgpu=warn".into()),
        )
        .init();

    tracing::info!("Starting Hand of Midas");

    #[cfg(feature = "dev_harness")]
    dev_harness::init(dev_harness::resolve_port());

    #[cfg(feature = "dev_harness")]
    let boot_fixture = parse_boot_fixture_arg();

    // Verify render crate links correctly.
    tracing::debug!(
        "Candle shader loaded: {} bytes",
        midas_render::CANDLE_SHADER_SRC.len()
    );

    // Use iced::daemon so the view function receives a window::Id,
    // enabling multi-window support for floating chart panels.
    //
    // When `--fixture <name>` was passed, apply the fixture to the
    // freshly-constructed app before iced renders the first frame.
    iced::daemon(
        move || {
            #[cfg(feature = "dev_harness")]
            {
                let (mut app, boot_task) = MidasApp::new();
                let final_task = match boot_fixture.clone() {
                    Some(name) => {
                        match crate::dev_harness::fixture::apply_from_disk(&mut app, &name) {
                            Ok(fixture_task) => Task::batch([boot_task, fixture_task]),
                            Err(e) => {
                                tracing::error!("devloop: boot fixture {name} failed: {e}");
                                boot_task
                            }
                        }
                    }
                    None => boot_task,
                };
                (app, final_task)
            }
            #[cfg(not(feature = "dev_harness"))]
            {
                MidasApp::new()
            }
        },
        update,
        view,
    )
    .title(window_title)
    .theme(Theme::Dark)
    .subscription(subscription)
    .run()
}

/// Per-window OS title. Resolves the iced `window::Id` to its
/// `WindowKey` via `MidasApp::iced_id_to_key`. Slice C: rename takes
/// effect next time iced re-polls the title (next focus / event).
fn window_title(state: &MidasApp, id: window::Id) -> String {
    match state.iced_id_to_key.get(&id) {
        Some(key) => format!("Hand of Midas — {}", key.as_str()),
        None => "Hand of Midas".to_string(),
    }
}

/// Parse `--fixture <name>` from the command line. Returns the fixture
/// name, or `None` if the flag was absent. Tolerates both
/// `--fixture name` and `--fixture=name` forms.
#[cfg(feature = "dev_harness")]
fn parse_boot_fixture_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(rest) = arg.strip_prefix("--fixture=") {
            return Some(rest.to_owned());
        }
        if arg == "--fixture" {
            return args.next();
        }
    }
    None
}

/// iced update function -- delegates to the app state.
fn update(state: &mut MidasApp, message: Message) -> Task<Message> {
    state.update(message)
}

/// iced view function -- delegates to the app state.
///
/// Called once per window. The `window_id` identifies which OS window
/// is being rendered (main vs. floating chart).
fn view(state: &MidasApp, window_id: window::Id) -> Element<'_, Message> {
    state.view(window_id)
}

/// iced subscription -- listens for keyboard events, periodic ticks,
/// and window close events.
fn subscription(state: &MidasApp) -> Subscription<Message> {
    let keyboard_sub = keyboard::listen().map(|event| match event {
        keyboard::Event::KeyPressed { key, modifiers, .. } => {
            // Ctrl+N: add new chart.
            if modifiers.control() {
                if let keyboard::Key::Character(ref c) = key {
                    if c.as_str() == "n" {
                        return Message::AddChart;
                    }
                }
            }
            Message::KeyPressed(key)
        }
        _ => Message::Tick,
    });

    // Periodic tick at 1 Hz for clock updates and debounced config saves.
    let tick_sub = iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::Tick);

    // Slice F1 retired the legacy `close_events()` subscription —
    // every window close now flows through `window::events()`'s
    // `CloseRequested` (handled below by the slice-C
    // `Message::WindowCloseRequested(window::Id)` handler).

    // Track window move/resize/focus/close-requested for config
    // persistence and slice-C multi-window lifecycle. Wrapped in the
    // appropriate variants — geometry events still flow through
    // `Message::Window` for the WindowGeometry controller (audit P1
    // slice 2); focus and close-requested go to the slice-C lifecycle
    // handler.
    let window_events_sub = window::events().map(|(id, event)| match event {
        iced::window::Event::Moved(pos) => Message::Window(
            window_geometry::WindowGeometryMsg::Moved(pos.x as i32, pos.y as i32),
        ),
        iced::window::Event::Resized(size) => Message::Window(
            window_geometry::WindowGeometryMsg::Resized(size.width as u32, size.height as u32),
        ),
        iced::window::Event::Focused => Message::WindowFocused(id),
        iced::window::Event::Unfocused => Message::WindowUnfocused(id),
        iced::window::Event::CloseRequested => Message::WindowCloseRequested(id),
        _ => Message::Tick,
    });

    // Slice-C 1 Hz watchdog. Scans `pending_window_opens` for entries
    // older than `WINDOW_ATTACH_TIMEOUT_SECS` and fires
    // `WindowAttachFailed` for each. Decoupled from `tick_sub` so the
    // existing `Tick` handler doesn't need to grow another arm.
    let attach_watchdog =
        iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::WindowAttachWatchdog);

    // Refresh watchlist market data every 60 seconds.
    let market_refresh =
        iced::time::every(std::time::Duration::from_secs(60)).map(|_| Message::RefreshMarketData);

    // Always track cursor position so drag preview appears at the right spot.
    let cursor_sub = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::DragCursorMoved(position))
        }
        _ => None,
    });

    let mut subs = vec![
        keyboard_sub,
        tick_sub,
        window_events_sub,
        attach_watchdog,
        market_refresh,
        cursor_sub,
    ];

    // Global mouse-up detection during pending or active drag.
    if state.pending_drag.is_some() || state.dragging_ticker.is_some() {
        let mouse_up_sub = iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                Some(Message::DragMouseUp)
            }
            _ => None,
        });
        subs.push(mouse_up_sub);
    }

    // Dev harness subscription (feature-gated).
    #[cfg(feature = "dev_harness")]
    {
        let source = crate::dev_harness::DevHarnessSource {
            port: crate::dev_harness::resolve_port(),
        };
        subs.push(Subscription::run_with(
            source,
            crate::dev_harness::dev_harness_stream,
        ));
    }

    // Router-refactor: per-consumer subscriptions (S7). Each returns
    // `Subscription::none()` until `self.router` is `Some(..)` and a
    // handle has been installed in the relevant registry, so adding
    // them now is a no-op on every existing code path.
    subs.push(state.chart_subscriptions());
    subs.push(state.watchlist_subscription());
    subs.push(state.ticker_subscription());

    // Router connection-state stream. Drives the title-bar status and
    // the per-account-panel "Disconnected — data may be stale" banner
    // off the router's actual `ConnectionState` watch instead of the
    // legacy broker-engine path that doesn't fire for sim or IB-router
    // construction. Without this, the banner is always on.
    if let Some(ref router) = state.router {
        let source = crate::app::connection_subscription::ConnectionStateSource {
            router: router.clone(),
        };
        subs.push(Subscription::run_with(
            source,
            crate::app::connection_subscription::connection_state_stream,
        ));
    }

    // Router-era positions subscription (BR-14).
    if let Some(ref order_client) = state.router_order_client {
        let source = crate::account_panel::PositionEventsSource {
            order_client: order_client.clone(),
        };
        let positions_sub = crate::account_panel::router_positions_subscription(source)
            .map(Message::AccountPositionsBatch);
        subs.push(positions_sub);
    }

    // Router-era order-lifecycle subscription (slice 10c). Fans every
    // `midas_broker::OrderEvent` into `Message::RouterOrderEvent` so
    // the app-side handler can translate IB order ids to local UUIDs
    // and drive the existing order-blotter / TickerState paths.
    if let Some(ref order_client) = state.router_order_client {
        let source = crate::app::order_events_subscription::OrderEventsSource {
            order_client: order_client.clone(),
        };
        subs.push(Subscription::run_with(
            source,
            crate::app::order_events_subscription::order_events_stream,
        ));
    }

    Subscription::batch(subs)
}
