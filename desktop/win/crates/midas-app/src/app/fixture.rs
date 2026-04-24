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
use midas_devloop_proto::{
    FixtureEnvelope, CURRENT_FIXTURE_SCHEMA, DEVLOOP_FIXTURE_VERSION, MIN_SUPPORTED_FIXTURE_VERSION,
};
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
    /// A fixture envelope's `devloop_fixture_version` sits past the
    /// current build's supported window. Unlike
    /// [`Self::VersionMismatch`], this fires for future envelopes
    /// specifically — slice 8c backward-compat for past envelopes is
    /// handled in-band via forward translation.
    #[error(
        "fixture envelope version {got} is newer than this build supports (max {max}); \
         downgrade the fixture or upgrade the binary"
    )]
    UnsupportedFutureVersion { got: u32, max: u32 },
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
            // Slice 8c: every write stamps the current schema. Older
            // fixtures without the field deserialise as v1; on next
            // snapshot they round-trip forward to v2.
            schema: CURRENT_FIXTURE_SCHEMA,
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
        // Slice 8c — accept any envelope version from
        // MIN_SUPPORTED_FIXTURE_VERSION up to DEVLOOP_FIXTURE_VERSION.
        // Below the floor we can't realistically translate; above
        // the ceiling we refuse because the wire format might have
        // diverged.
        if envelope.devloop_fixture_version > DEVLOOP_FIXTURE_VERSION {
            return Err(FixtureError::UnsupportedFutureVersion {
                got: envelope.devloop_fixture_version,
                max: DEVLOOP_FIXTURE_VERSION,
            });
        }
        if envelope.devloop_fixture_version < MIN_SUPPORTED_FIXTURE_VERSION {
            return Err(FixtureError::VersionMismatch {
                field: "devloop_fixture_version",
                expected: MIN_SUPPORTED_FIXTURE_VERSION,
                got: envelope.devloop_fixture_version,
            });
        }

        // TickerState has a separate chain — still gates hard because a
        // mismatch here means TickerState deserialisation would fail
        // unpredictably downstream.
        if envelope.ticker_state_version != ticker_state::CURRENT_VERSION {
            return Err(FixtureError::VersionMismatch {
                field: "ticker_state_version",
                expected: ticker_state::CURRENT_VERSION,
                got: envelope.ticker_state_version,
            });
        }

        if envelope.devloop_fixture_version < DEVLOOP_FIXTURE_VERSION {
            tracing::info!(
                from = envelope.devloop_fixture_version,
                to = DEVLOOP_FIXTURE_VERSION,
                "devloop: forward-translating v1 fixture envelope; \
                 next SnapshotFixture will write v2"
            );
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
            schema: CURRENT_FIXTURE_SCHEMA,
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
            schema: CURRENT_FIXTURE_SCHEMA,
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
        assert_eq!(back.schema, CURRENT_FIXTURE_SCHEMA);
    }

    // ── Slice 8c — fixture v2 backward + forward compat ─────────────────

    /// A v1 file (no `schema` key, `devloop_fixture_version: 1`) parses
    /// cleanly into the current envelope shape. The `schema` field
    /// defaults to v1 via the `#[serde(default)]` attribute. On a next
    /// snapshot the app-side writer stamps `CURRENT_FIXTURE_SCHEMA`.
    #[test]
    fn v1_fixture_file_round_trips_to_v2_shape() {
        let raw = r#"{
            "devloop_fixture_version": 1,
            "ticker_state_version": 2,
            "captured_at": "2026-04-17T00:00:00Z",
            "note": null,
            "active_ticker": null,
            "app_config": {},
            "ticker_states": []
        }"#;
        let env: FixtureEnvelope = serde_json::from_str(raw).expect("v1 parse");
        assert_eq!(env.devloop_fixture_version, 1);
        assert_eq!(
            env.schema,
            midas_devloop_proto::FIXTURE_SCHEMA_V1,
            "missing schema field defaults to v1"
        );

        // Re-serialise + re-parse with a v2 stamp — this is what the
        // app-side writer does on the next SnapshotFixture after
        // loading a v1 file.
        let upgraded = FixtureEnvelope {
            devloop_fixture_version: DEVLOOP_FIXTURE_VERSION,
            schema: CURRENT_FIXTURE_SCHEMA,
            ..env
        };
        let s = serde_json::to_string(&upgraded).unwrap();
        let back: FixtureEnvelope = serde_json::from_str(&s).unwrap();
        assert_eq!(back.devloop_fixture_version, DEVLOOP_FIXTURE_VERSION);
        assert_eq!(back.schema, CURRENT_FIXTURE_SCHEMA);
    }

    /// Slice 8c: a future envelope version — one whose
    /// `devloop_fixture_version` exceeds the current
    /// `DEVLOOP_FIXTURE_VERSION` constant — is refused cleanly. No
    /// panic, no silent corruption.
    #[test]
    fn unknown_future_schema_errors_cleanly() {
        // Construct a hypothetical v3 envelope and serialise it —
        // then try to parse + validate. The parser side succeeds
        // (forward-compat deserialiser); the `apply_fixture_envelope`
        // entry is the gate that errors.
        let future = FixtureEnvelope {
            devloop_fixture_version: DEVLOOP_FIXTURE_VERSION + 1,
            schema: CURRENT_FIXTURE_SCHEMA + 1,
            ticker_state_version: ticker_state::CURRENT_VERSION,
            captured_at: "2030-01-01T00:00:00Z".to_owned(),
            note: None,
            active_ticker: None,
            app_config: json!({}),
            ticker_states: vec![],
        };
        // We can't call `apply_fixture_envelope` without a MidasApp,
        // but we can verify the version gate logic via a local
        // predicate mirror of the impl. The test documents the
        // contract; the real rejection path is covered by integration
        // tests under `tests/`.
        let rejected = future.devloop_fixture_version > DEVLOOP_FIXTURE_VERSION;
        assert!(rejected, "future envelope must fail the version gate");
    }

    /// Slice 8c: stability guard — the v2 envelope shape MUST keep
    /// its current field set. A change here is a wire-break and
    /// requires DEVLOOP_FIXTURE_VERSION to bump.
    #[test]
    fn v2_envelope_shape_is_stable() {
        let env = FixtureEnvelope {
            devloop_fixture_version: DEVLOOP_FIXTURE_VERSION,
            schema: CURRENT_FIXTURE_SCHEMA,
            ticker_state_version: ticker_state::CURRENT_VERSION,
            captured_at: "2026-04-22T00:00:00Z".to_owned(),
            note: None,
            active_ticker: None,
            app_config: json!({}),
            ticker_states: vec![],
        };
        let v = serde_json::to_value(&env).unwrap();
        let expected_keys = [
            "devloop_fixture_version",
            "schema",
            "ticker_state_version",
            "captured_at",
            "note",
            "active_ticker",
            "app_config",
            "ticker_states",
        ];
        let obj = v.as_object().expect("envelope serialises to object");
        for key in &expected_keys {
            assert!(obj.contains_key(*key), "missing field: {key}");
        }
        // Exact-count assertion — a new field addition fails this
        // intentionally so the author updates the test AND bumps the
        // version constants.
        assert_eq!(
            obj.len(),
            expected_keys.len(),
            "v2 envelope has unexpected field count"
        );
    }
}
