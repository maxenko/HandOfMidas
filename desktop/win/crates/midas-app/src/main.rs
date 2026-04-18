//! midas-app: Hand of Midas desktop application.
//!
//! This is the binary entry point. It creates an iced daemon (multi-window
//! capable) and wires together all workspace crates.

// TODO: Wire into MidasApp when LevelStore is replaced by AnnotationStore.
mod annotation_persistence;
mod annotation_store;
mod app;
mod broker_bridge;
mod chart_view;
mod chart_widget;
#[cfg(feature = "dev_harness")]
mod dev_harness;
mod layout;
mod level_store;
mod link;
mod market_cache;
mod order_blotter;
mod order_panel;
mod registry;
mod theme;
mod thumbnail_data;
mod thumbnail_store;
#[path = "thumbnail_widget.rs"]
mod thumbnail_widget;
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
    .title("Hand of Midas")
    .theme(Theme::Dark)
    .subscription(subscription)
    .run()
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

    Subscription::batch(subs)
}
