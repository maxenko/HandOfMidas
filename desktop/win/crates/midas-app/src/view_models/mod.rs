//! View-model projections from [`crate::app::MidasApp`] to
//! self-contained, pure-data structs the view layer consumes.
//!
//! # Why
//!
//! Audit P1 finding: the `view_account_*` and other render helpers reach
//! all over `MidasApp` — `self.order_blotter`, `self.positions`,
//! `self.account_panels`, `self.recent_symbols`,
//! `self.broker_connection_display`, `self.link_picker_open`. Threading
//! every render decision through `&self` makes the view functions
//! impossible to test without booting the iced runtime, and tangles
//! presentation rules with state ownership.
//!
//! View-models break that coupling: a builder method on `MidasApp`
//! gathers the inputs once, projects them into a small `*Vm` struct of
//! plain values, and the view function consumes the struct (not `self`).
//! Builders are pure-`&self` and trivially unit-testable.
//!
//! # Slice 3A scope
//!
//! Only the Account panel's *header chrome* (tab badges, banner
//! visibility) lands in this slice. Sub-tab bodies (Orders / Positions /
//! TradeHistory / Recents) keep the existing `&self`-reading shape until
//! follow-up slices migrate them one at a time.

pub mod account_panel;
pub mod chart_pane;
pub mod order_panel;
pub mod status_bar;
pub mod toolbar;
pub mod watchlist;
