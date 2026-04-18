//! Visual demo + screenshot harness for the `midas-ui` widget library.
//!
//! Uses [`Tabs`] as the top-level navigator; each tab shows a showcase of
//! one of the library's widgets.
//!
//! Run normally to see the widgets interactively:
//!   cargo run --example ui_demo -p midas-ui
//!
//! Set `UI_DEMO_SCREENSHOT=path/to/out.png` to capture a single PNG and
//! exit automatically (used by the dev loop for visual verification):
//!   UI_DEMO_SCREENSHOT=ui_demo.png cargo run --example ui_demo -p midas-ui

use std::path::PathBuf;
use std::time::Duration;

use iced::widget::tooltip::Position;
use iced::widget::{column, container, row, text};
use iced::window::Screenshot;
use iced::{window, Color, Element, Length, Subscription, Task, Theme};

use midas_ui::{
    ButtonGroup, EditableLabel, IconButton, Label, TabItem, Tabs, TextButton, Tooltip, UiTheme,
};

fn main() -> iced::Result {
    iced::application(Demo::boot, Demo::update, Demo::view)
        .title("midas-ui Widget Demo")
        .theme(Theme::Dark)
        .subscription(Demo::subscription)
        .window_size((1200.0, 520.0))
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Section {
    #[default]
    Label,
    EditableLabel,
    Button,
    Icon,
    Group,
    Tooltip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timeframe {
    M1,
    M5,
    M15,
    H1,
    D1,
}

struct Demo {
    selected: Section,
    theme: UiTheme,
    main_window: Option<window::Id>,
    screenshot_taken: bool,
    screenshot_path: Option<PathBuf>,
    // EditableLabel state (owned by parent, as the widget requires).
    symbol: String,
    symbol_edit_text: String,
    symbol_editing: bool,
    // ButtonGroup selection.
    timeframe: Timeframe,
    // TextButton demo: pressed counter proves the handler fires.
    press_count: u32,
}

#[derive(Debug, Clone)]
enum Message {
    Selected(Section),
    WindowOpened(window::Id),
    Tick,
    ScreenshotReady(Screenshot),
    // Widget-under-demo messages.
    SymbolEditStart,
    SymbolEditChanged(String),
    SymbolEditConfirm(String),
    SymbolEditCancel,
    TimeframeSelected(Timeframe),
    Pressed,
    IconPressed,
    Noop,
}

impl Demo {
    fn boot() -> (Self, Task<Message>) {
        let screenshot_path = std::env::var_os("UI_DEMO_SCREENSHOT").map(PathBuf::from);
        (
            Self {
                selected: Section::Label,
                theme: UiTheme::default(),
                main_window: None,
                screenshot_taken: false,
                screenshot_path,
                symbol: "AAPL".to_owned(),
                symbol_edit_text: String::new(),
                symbol_editing: false,
                timeframe: Timeframe::D1,
                press_count: 0,
            },
            Task::none(),
        )
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Selected(s) => {
                self.selected = s;
                Task::none()
            }
            Message::WindowOpened(id) => {
                if self.main_window.is_none() {
                    self.main_window = Some(id);
                }
                Task::none()
            }
            Message::Tick => {
                if self.screenshot_taken || self.screenshot_path.is_none() {
                    return Task::none();
                }
                if let Some(id) = self.main_window {
                    self.screenshot_taken = true;
                    return window::screenshot(id).map(Message::ScreenshotReady);
                }
                Task::none()
            }
            Message::ScreenshotReady(screenshot) => {
                if let Some(out) = self.screenshot_path.clone() {
                    match save_png(&screenshot, &out) {
                        Ok(()) => eprintln!(
                            "ui_demo: saved {}x{} screenshot to {}",
                            screenshot.size.width,
                            screenshot.size.height,
                            out.display()
                        ),
                        Err(e) => eprintln!("ui_demo: save failed: {e}"),
                    }
                }
                iced::exit()
            }
            Message::SymbolEditStart => {
                self.symbol_editing = true;
                self.symbol_edit_text = self.symbol.clone();
                Task::none()
            }
            Message::SymbolEditChanged(s) => {
                self.symbol_edit_text = s;
                Task::none()
            }
            Message::SymbolEditConfirm(s) => {
                self.symbol = s;
                self.symbol_editing = false;
                Task::none()
            }
            Message::SymbolEditCancel => {
                self.symbol_editing = false;
                Task::none()
            }
            Message::TimeframeSelected(tf) => {
                self.timeframe = tf;
                Task::none()
            }
            Message::Pressed => {
                self.press_count = self.press_count.saturating_add(1);
                Task::none()
            }
            Message::IconPressed => Task::none(),
            Message::Noop => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let tabs = Tabs::new(
            vec![
                TabItem::new("Label", Section::Label),
                TabItem::new("EditableLabel", Section::EditableLabel),
                TabItem::new("TextButton", Section::Button),
                TabItem::new("IconButton", Section::Icon),
                TabItem::new("ButtonGroup", Section::Group),
                TabItem::new("Tooltip", Section::Tooltip).with_badge(2),
            ],
            self.selected,
            Message::Selected,
        )
        .view(&self.theme);

        let body = match self.selected {
            Section::Label => self.view_label(),
            Section::EditableLabel => self.view_editable_label(),
            Section::Button => self.view_text_button(),
            Section::Icon => self.view_icon_button(),
            Section::Group => self.view_button_group(),
            Section::Tooltip => self.view_tooltip(),
        };

        container(column![tabs, body].spacing(20))
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Color::from_rgb(0.06, 0.06, 0.08).into()),
                ..Default::default()
            })
            .into()
    }

    fn view_label(&self) -> Element<'_, Message> {
        let theme = &self.theme;
        column![
            Label::new("Default label").view(theme),
            Label::new("Bold label").bold().view(theme),
            Label::new("Large bold (18)").size(18.0).bold().view(theme),
            Label::new("Accent color").color(theme.accent).view(theme),
            Label::new("Muted secondary")
                .color(theme.text_secondary)
                .view(theme),
        ]
        .spacing(8)
        .into()
    }

    fn view_editable_label(&self) -> Element<'_, Message> {
        let theme = &self.theme;
        let editable = EditableLabel::new(
            &self.symbol,
            &self.symbol_edit_text,
            self.symbol_editing,
            Message::SymbolEditChanged,
            Message::SymbolEditConfirm,
            Message::SymbolEditStart,
        )
        .on_cancel(Message::SymbolEditCancel)
        .size(18.0)
        .bold()
        .view(theme);

        column![
            Label::new("Click to edit; Enter to confirm, Esc to cancel.")
                .color(theme.text_secondary)
                .view(theme),
            editable,
            text(format!("Current value: {}", self.symbol))
                .size(theme.label_font_size)
                .color(theme.text_secondary),
        ]
        .spacing(8)
        .into()
    }

    fn view_text_button(&self) -> Element<'_, Message> {
        let theme = &self.theme;
        let buttons = row![
            TextButton::new("Primary")
                .on_press(Message::Pressed)
                .view(theme),
            TextButton::new("Accent")
                .on_press(Message::Pressed)
                .background(theme.accent)
                .text_color(Color::WHITE)
                .view(theme),
            TextButton::new("Wide")
                .on_press(Message::Pressed)
                .padding_h(20.0)
                .view(theme),
            TextButton::<Message>::new("Disabled")
                .disabled(true)
                .view(theme),
        ]
        .spacing(8);

        column![
            Label::new("Four visual states; press count on the right proves the handler fires.")
                .color(theme.text_secondary)
                .view(theme),
            buttons,
            text(format!("Presses: {}", self.press_count))
                .size(theme.label_font_size)
                .color(theme.text_primary),
        ]
        .spacing(12)
        .into()
    }

    fn view_icon_button(&self) -> Element<'_, Message> {
        let theme = &self.theme;
        let row = row![
            IconButton::new("\u{00D7}")
                .on_press(Message::IconPressed)
                .tooltip("Close")
                .view(theme),
            IconButton::new("\u{2699}")
                .on_press(Message::IconPressed)
                .tooltip("Settings")
                .view(theme),
            IconButton::new("+")
                .on_press(Message::IconPressed)
                .tooltip("Add")
                .view(theme),
            IconButton::new("\u{21BB}")
                .on_press(Message::IconPressed)
                .tooltip("Refresh")
                .view(theme),
            IconButton::<Message>::new("\u{1F512}")
                .disabled(true)
                .tooltip("Locked")
                .view(theme),
        ]
        .spacing(4);

        column![
            Label::new("Transparent at rest; hover to surface. Tooltips on hover.")
                .color(theme.text_secondary)
                .view(theme),
            row,
        ]
        .spacing(12)
        .into()
    }

    fn view_button_group(&self) -> Element<'_, Message> {
        let theme = &self.theme;
        let group = ButtonGroup::new(
            vec![
                ("1m", Timeframe::M1),
                ("5m", Timeframe::M5),
                ("15m", Timeframe::M15),
                ("1h", Timeframe::H1),
                ("1D", Timeframe::D1),
            ],
            self.timeframe,
            Message::TimeframeSelected,
        )
        .view(theme);

        let selected_label = match self.timeframe {
            Timeframe::M1 => "1m",
            Timeframe::M5 => "5m",
            Timeframe::M15 => "15m",
            Timeframe::H1 => "1h",
            Timeframe::D1 => "1D",
        };

        column![
            Label::new("Exactly one selection at a time.")
                .color(theme.text_secondary)
                .view(theme),
            group,
            text(format!("Selected: {selected_label}"))
                .size(theme.label_font_size)
                .color(theme.text_primary),
        ]
        .spacing(12)
        .into()
    }

    fn view_tooltip(&self) -> Element<'_, Message> {
        let theme = &self.theme;
        let plain_label: Element<'_, Message> = Label::new("Hover me (bottom tip)").view(theme);
        let with_bottom = Tooltip::new(plain_label, "Appears below the target").view(theme);

        let small_label: Element<'_, Message> = Label::new("Hover me (top tip)").view(theme);
        let with_top = Tooltip::new(small_label, "Appears above the target")
            .position(Position::Top)
            .gap(8.0)
            .view(theme);

        // Also wrap a TextButton — real callers often do this for subtle affordances.
        let btn: Element<'_, Message> = TextButton::new("Hover button")
            .on_press(Message::Noop)
            .view(theme);
        let wrapped_btn = Tooltip::new(btn, "Button with a tooltip")
            .position(Position::Right)
            .view(theme);

        column![
            Label::new("Theme-styled wrapper around iced::widget::tooltip.")
                .color(theme.text_secondary)
                .view(theme),
            row![with_bottom, with_top, wrapped_btn].spacing(20),
        ]
        .spacing(12)
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::open_events().map(Message::WindowOpened),
            // Poll every 400ms; capture once the window is open and we have an id.
            // Two ticks of warm-up gives wgpu time to actually render.
            iced::time::every(Duration::from_millis(400)).map(|_| Message::Tick),
        ])
    }
}

fn save_png(screenshot: &Screenshot, out: &PathBuf) -> Result<(), String> {
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
    }
    let buf = image::RgbaImage::from_raw(
        screenshot.size.width,
        screenshot.size.height,
        screenshot.rgba.to_vec(),
    )
    .ok_or_else(|| "rgba size mismatch".to_string())?;
    buf.save_with_format(out, image::ImageFormat::Png)
        .map_err(|e| format!("encode: {e}"))
}
