//! midas-app: Hand of Midas desktop application.
//!
//! This is the binary entry point. It creates an iced daemon (multi-window
//! capable) and wires together all workspace crates.

mod app;
mod chart_widget;
mod layout;
mod theme;

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
fn subscription(_state: &MidasApp) -> Subscription<Message> {
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

    Subscription::batch([keyboard_sub, tick_sub, close_sub, window_events_sub])
}
