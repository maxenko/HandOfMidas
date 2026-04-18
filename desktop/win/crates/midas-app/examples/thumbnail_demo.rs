//! Standalone demo for the thumbnail shader widget (Slice 2).
//!
//! Renders three thumbnails in a row:
//! * up-trend ("1m"),
//! * down-trend ("5m"),
//! * flat ("1d").
//!
//! Each is 120 × 30 px. Clicking a thumbnail increments its own
//! counter; the window title bar shows the running tally so the
//! `mouse_area` hookup can be validated without any GUI automation.
//!
//! Run:
//! ```bash
//! cd desktop/win
//! cargo run -p midas-app --example thumbnail_demo
//! ```
//!
//! Window resize must not panic — that validates the device-lifecycle
//! guarantee of Decision 7 in `plan/feature-chart-thumbnail-cells.md`.

use std::sync::Arc;

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Task, Theme};

use midas_app::thumbnail_widget::{thumbnail_cell, ThumbnailSnapshot};

/// Stable, caller-assigned identity for each demo thumbnail — matches
/// the `widget_key` contract documented on [`ThumbnailSnapshot`].
const KEY_UP: u64 = 1;
const KEY_DOWN: u64 = 2;
const KEY_FLAT: u64 = 3;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .window_size((640.0, 240.0))
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Clicked(u64),
}

#[derive(Debug)]
struct App {
    clicks_up: u32,
    clicks_down: u32,
    clicks_flat: u32,

    up_closes: Arc<Vec<f32>>,
    down_closes: Arc<Vec<f32>>,
    flat_closes: Arc<Vec<f32>>,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        // Fifty synthetic closes per series — enough to visually
        // distinguish the three shapes at 120 × 30 px.
        let up_closes: Vec<f32> = (0..50).map(|i| 10.0 + i as f32 * 0.4).collect();
        let down_closes: Vec<f32> = (0..50).map(|i| 40.0 - i as f32 * 0.4).collect();
        let flat_closes: Vec<f32> = (0..50).map(|_| 20.0).collect();

        (
            Self {
                clicks_up: 0,
                clicks_down: 0,
                clicks_flat: 0,
                up_closes: Arc::new(up_closes),
                down_closes: Arc::new(down_closes),
                flat_closes: Arc::new(flat_closes),
            },
            Task::none(),
        )
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn title(&self) -> String {
        format!(
            "thumbnail_demo — clicks: up={} down={} flat={}",
            self.clicks_up, self.clicks_down, self.clicks_flat
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Clicked(KEY_UP) => self.clicks_up += 1,
            Message::Clicked(KEY_DOWN) => self.clicks_down += 1,
            Message::Clicked(KEY_FLAT) => self.clicks_flat += 1,
            Message::Clicked(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let up_snap = ThumbnailSnapshot {
            widget_key: KEY_UP,
            closes: self.up_closes.clone(),
            y_min: 10.0,
            y_max: 30.0,
            color: [0.15, 0.65, 0.45, 1.0], // greenish
            generation: 1,
            label: "1m".to_string(),
        };
        let down_snap = ThumbnailSnapshot {
            widget_key: KEY_DOWN,
            closes: self.down_closes.clone(),
            y_min: 20.0,
            y_max: 40.0,
            color: [0.75, 0.30, 0.30, 1.0], // reddish
            generation: 1,
            label: "5m".to_string(),
        };
        let flat_snap = ThumbnailSnapshot {
            widget_key: KEY_FLAT,
            closes: self.flat_closes.clone(),
            y_min: 18.0,
            y_max: 22.0,
            color: [0.55, 0.55, 0.60, 1.0], // muted grey-blue
            generation: 1,
            label: "1d".to_string(),
        };

        let cell_size = (Length::Fixed(120.0), Length::Fixed(30.0));

        let make_cell = |snap: ThumbnailSnapshot| -> Element<'_, Message> {
            let widget_key = snap.widget_key;
            container(thumbnail_cell(snap, Message::Clicked(widget_key)))
                .width(cell_size.0)
                .height(cell_size.1)
                .style(|_theme: &Theme| container::Style {
                    background: Some(iced::Color::from_rgb(0.08, 0.08, 0.12).into()),
                    ..Default::default()
                })
                .into()
        };

        let thumbnails = row![
            make_cell(up_snap),
            make_cell(down_snap),
            make_cell(flat_snap),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let heading = text("Thumbnail demo — click any cell to bump its counter").size(14);

        container(
            column![heading, thumbnails]
                .spacing(16)
                .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(20)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    }
}
