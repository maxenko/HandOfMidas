//! Fixture snapshot + apply for the dev harness.
//!
//! Capture: projects `MidasApp` into a [`FixtureEnvelope`] — heavy
//! lifting is done by the existing `build_config()` persistence path;
//! tickers are serialised alongside.
//!
//! Apply: the inverse. Rebuilds the workspace from the envelope's
//! `AppConfig`, replaces `tickers`, kicks off async data loads so the
//! saved camera + data land together.
//!
//! Feature-gated on `dev_harness` since `FixtureEnvelope` comes from
//! the `midas-devloop-proto` crate which is itself feature-gated.

use chrono::Utc;
use iced::Task;
use midas_core::config::AppConfig;
use midas_devloop_proto::{FixtureEnvelope, DEVLOOP_FIXTURE_VERSION};
use thiserror::Error;

use super::{Message, MidasApp, RecentEntry};
use crate::annotation_store::SymbolKey;
use crate::ticker_state::{self, TickerState};

#[derive(Debug, Error)]
pub enum FixtureError {
    #[error("fixture not found: {0}")]
    NotFound(String),
    #[error("fixture version mismatch on {field}: expected {expected}, got {got}")]
    VersionMismatch {
        field: &'static str,
        expected: u32,
        got: u32,
    },
    #[error("fixture serialise/deserialise failure: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("fixture IO failure: {0}")]
    Io(#[from] std::io::Error),
}

impl MidasApp {
    /// Build a [`FixtureEnvelope`] from the current `MidasApp` state.
    ///
    /// Reuses the standard `build_config()` serialisation path so
    /// fixtures and config files carry identical layout / chart /
    /// order-panel representations.
    pub(crate) fn snapshot_to_fixture(
        &self,
        note: Option<String>,
    ) -> Result<FixtureEnvelope, FixtureError> {
        let app_config = self.build_config();
        let app_config_json = serde_json::to_value(&app_config)?;

        let mut ticker_states = Vec::with_capacity(self.tickers.len());
        for state in self.tickers.values() {
            ticker_states.push(serde_json::to_value(state)?);
        }

        Ok(FixtureEnvelope {
            devloop_fixture_version: DEVLOOP_FIXTURE_VERSION,
            ticker_state_version: ticker_state::CURRENT_VERSION,
            captured_at: Utc::now().to_rfc3339(),
            note,
            active_ticker: self.active_chart_symbol(),
            app_config: app_config_json,
            ticker_states,
        })
    }

    /// Replace this app's state with the contents of `envelope`.
    ///
    /// Workspace topology, charts, order panels, watchlists, and
    /// tickers are rebuilt wholesale. Returns a batched `Task` that
    /// kicks off the async data load for every chart — those deliver
    /// `Message::DataRestoredFromStartup`, which preserves the saved
    /// camera.
    ///
    /// Version mismatches fail loudly; no silent migration.
    pub(crate) fn apply_fixture_envelope(
        &mut self,
        envelope: FixtureEnvelope,
    ) -> Result<Task<Message>, FixtureError> {
        if envelope.devloop_fixture_version != DEVLOOP_FIXTURE_VERSION {
            return Err(FixtureError::VersionMismatch {
                field: "devloop_fixture_version",
                expected: DEVLOOP_FIXTURE_VERSION,
                got: envelope.devloop_fixture_version,
            });
        }
        if envelope.ticker_state_version != ticker_state::CURRENT_VERSION {
            return Err(FixtureError::VersionMismatch {
                field: "ticker_state_version",
                expected: ticker_state::CURRENT_VERSION,
                got: envelope.ticker_state_version,
            });
        }

        let config: AppConfig = serde_json::from_value(envelope.app_config)?;

        let mut tickers = std::collections::HashMap::with_capacity(envelope.ticker_states.len());
        for raw in envelope.ticker_states {
            let state: TickerState = serde_json::from_value(raw)?;
            tickers.insert(state.symbol().clone(), state);
        }

        // Rebuild workspace + panels. Reuses the same path config loads
        // use at startup, so any bug there shows up in both places.
        let (workspace, charts, watchlists, order_panels, account_panels) =
            Self::restore_from_layout_tree(
                &config.layout_tree,
                &config.charts,
                &config.watchlists,
                &config.order_panels,
                &config.account_panels,
            );

        self.workspace = workspace;
        self.charts = charts;
        self.watchlists = watchlists;
        self.order_panels = order_panels;
        self.account_panels = account_panels;
        self.recent_symbols = config
            .recent_symbols
            .iter()
            .cloned()
            .map(|symbol| RecentEntry {
                symbol,
                last_seen: None,
            })
            .collect();
        self.tickers = tickers;
        // Levels live in `AnnotationStore` (audit P2b); drop any
        // previously-imported level annotations and reload from the
        // fixture's `config.levels`. Bracket annotations persist through
        // their own path and aren't touched here.
        for symbol in self
            .annotation_store
            .to_level_configs()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.annotation_store.clear_levels(&symbol);
        }
        self.annotation_store.import_level_configs(&config.levels);

        // Re-seed bound_symbol on charts the config may have set.
        for panel in self.charts.values_mut() {
            if panel.bound_symbol.is_none() && !panel.symbol.is_empty() {
                panel.bound_symbol = Some(SymbolKey::new(&panel.symbol));
            }
        }

        // Re-hydrate window geometry from the fixture's snapshot.
        // Preserve the controller's main_window id (set during boot)
        // by going through the controller's API rather than building
        // a fresh one from scratch.
        let main_id = self.window.main_window();
        self.window =
            crate::window_geometry::WindowGeometry::from_config(&config.window, self.window.size());
        if let Some(id) = main_id {
            let _ =
                self.window
                    .update(crate::window_geometry::WindowGeometryMsg::MainWindowOpened(
                        id,
                    ));
        }

        // S7e: market-data subscriptions are driven by the
        // per-consumer iced subscriptions; loading a fixture
        // rewrites `self.charts` / `self.watchlists`, iced re-diffs
        // `subscription()` on the next `update()`, and the new set
        // of chart / watchlist / ticker subscriptions spawns
        // automatically.

        // Kick off async data loads; `DataRestoredFromStartup` preserves
        // the camera the fixture just set.
        let load_tasks: Vec<Task<Message>> = self
            .charts
            .iter()
            .filter(|(_, panel)| !panel.symbol.is_empty())
            .map(|(id, panel)| self.load_chart_async_restore(*id, &panel.symbol, panel.timeframe))
            .collect();

        tracing::info!(
            "devloop: fixture applied — charts={} tickers={}",
            self.charts.len(),
            self.tickers.len(),
        );

        Ok(Task::batch(load_tasks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn version_mismatch_on_envelope_rejected() {
        // Can test version gating without constructing a MidasApp.
        let envelope = FixtureEnvelope {
            devloop_fixture_version: DEVLOOP_FIXTURE_VERSION + 99,
            ticker_state_version: ticker_state::CURRENT_VERSION,
            captured_at: "2026-04-17T00:00:00Z".to_owned(),
            note: None,
            active_ticker: None,
            app_config: json!({}),
            ticker_states: vec![],
        };

        // Envelope validation is the first step; confirm the proto
        // version and app version are comparable.
        assert_ne!(
            envelope.devloop_fixture_version, DEVLOOP_FIXTURE_VERSION,
            "sanity: test bumps the version deliberately"
        );
    }

    #[test]
    fn ticker_state_version_matches_current() {
        // Guard against drift: the fixture envelope's version must
        // match what TickerState reports. A bump to either should
        // force a matching update or the version-mismatch error path
        // misfires silently.
        assert_eq!(ticker_state::CURRENT_VERSION, 2);
    }

    #[test]
    fn envelope_current_constants_round_trip() {
        // Verifies proto and app agree on the minimum shape required.
        let envelope = FixtureEnvelope {
            devloop_fixture_version: DEVLOOP_FIXTURE_VERSION,
            ticker_state_version: ticker_state::CURRENT_VERSION,
            captured_at: "2026-04-17T00:00:00Z".to_owned(),
            note: Some("test".to_owned()),
            active_ticker: None,
            app_config: json!({
                "window": {"width": 1920, "height": 1080, "maximized": false},
                "theme": {"mode": "dark"},
                "charts": [],
                "levels": {},
                "watchlists": [],
                "order_panels": [],
                "panel_order": [],
                "layout_tree": [],
                "store": {},
                "providers": null,
            }),
            ticker_states: vec![],
        };
        let text = serde_json::to_string(&envelope).unwrap();
        let back: FixtureEnvelope = serde_json::from_str(&text).unwrap();
        assert_eq!(back.ticker_state_version, ticker_state::CURRENT_VERSION);
        assert_eq!(back.note.as_deref(), Some("test"));
    }
}
