//! midas-app: Hand of Midas desktop application.
//!
//! This is the binary entry point. It creates an iced daemon (multi-window
//! capable) and wires together all workspace crates.

// TODO: Wire into MidasApp when LevelStore is replaced by AnnotationStore.
mod annotation_persistence;
mod annotation_store;
mod app;
mod broker_bridge;
mod chart_widget;
mod layout;
mod level_store;
mod link;
mod market_cache;
mod order_panel;
mod registry;
mod theme;
mod ticker_order_intent;
#[allow(dead_code)] // Slice 0: full API surface defined, not yet wired to live paths
mod ticker_state;
mod watchlist;
mod watchlist_columns;

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

    // Verify render crate links correctly.
    tracing::debug!(
        "Candle shader loaded: {} bytes",
        midas_render::CANDLE_SHADER_SRC.len()
    );

    // Use iced::daemon so the view function receives a window::Id,
    // enabling multi-window support for floating chart panels.
    iced::daemon(MidasApp::new, update, view)
        .title("Hand of Midas")
        .theme(Theme::Dark)
        .subscription(subscription)
        .run()
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

    // Listen for window close events so we can clean up floating charts
    // and save config when the main window is closed.
    let close_sub = window::close_events().map(Message::FloatingWindowClosed);

    // Track window move/resize for config persistence.
    let window_events_sub = window::events().map(|(_id, event)| match event {
        iced::window::Event::Moved(pos) => Message::WindowMoved(pos.x as i32, pos.y as i32),
        iced::window::Event::Resized(size) => {
            Message::WindowResized(size.width as u32, size.height as u32)
        }
        _ => Message::Tick,
    });

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
        close_sub,
        window_events_sub,
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

    // Broker order event subscription.
    if let Some(ref bridge) = state.broker_bridge {
        let source = bridge.event_source();
        let broker_sub = Subscription::run_with(source, crate::broker_bridge::broker_event_stream);
        subs.push(broker_sub);
    }

    // Broker connection state subscription.
    if let Some(ref bridge) = state.broker_bridge {
        let conn_source = bridge.conn_source();
        let conn_sub =
            Subscription::run_with(conn_source, crate::broker_bridge::broker_conn_stream);
        subs.push(conn_sub);
    }

    Subscription::batch(subs)
}
