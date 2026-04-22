//! Ticker-state wiring helpers for [`super::MidasApp`].
//!
//! Extracted from `app.rs` to reduce that file's surface area.
//! Contains:
//! - symbol binding (`bind_chart_to_symbol`, `bind_panel_to_symbol`)
//! - panel ↔ ticker reconciliation (`sync_panel_to_intent`,
//!   `sync_drag_to_intent`, `panel_display_for_chart`,
//!   `hydrate_order_panel_for_chart`)
//! - snap helper (`maybe_emit_snap_for_active_chart`)
//! - `TickerState` lazy-factory (`ticker_mut`)
//! - effect handler (`handle_ticker_effects`)

use iced::Task;

use midas_chart::AnnotationId;
use midas_core::{ChartId, LinkMode, OrderPanelId};

use super::{Message, MidasApp};
use crate::annotation_store::SymbolKey;

impl MidasApp {
    // ── Symbol binding ──────────────────────────────────────────────

    /// Bind a docked chart to a symbol.
    ///
    /// This is the **single mutation point** for setting a chart's active
    /// symbol. It:
    /// 1. Sets `bound_symbol` and the backward-compat `symbol` field.
    /// 2. Lazy-creates a [`crate::ticker_state::TickerState`] for the
    ///    symbol.
    /// 3. Seeds the ticker state with cached market data (if available).
    /// 4. Fires `EnsureDraftBracket` so a linked order panel gets a
    ///    bracket immediately.
    ///
    /// Every user-facing path that changes a chart's symbol must route
    /// through this helper.
    pub(crate) fn bind_chart_to_symbol(&mut self, chart_id: ChartId, symbol: SymbolKey) {
        // 1. Set bound_symbol + backward-compat fields.
        if let Some(chart) = self.charts.get_mut(&chart_id) {
            chart.bound_symbol = Some(symbol.clone());
            chart.symbol = symbol.as_str().to_string();
            chart.symbol_input = symbol.as_str().to_string();
        }

        // S7e: per-chart subscriptions spawn on the next iced
        // re-diff via `chart_subscriptions`; no eager reconcile
        // needed.

        // 2. Lazy-create TickerState.
        self.tickers
            .entry(symbol.clone())
            .or_insert_with(|| crate::ticker_state::TickerState::new(symbol.clone()));

        // 3. Seed market data from cache.
        let (price, gatr) = self
            .market_cache
            .get(&symbol)
            .map(|s| (s.last_price, s.gatr_abs))
            .unwrap_or((None, None));
        if let Some(p) = price {
            if let Some(ts) = self.tickers.get_mut(&symbol) {
                ts.set_last_price(Some(p));
                ts.set_gatr_abs(gatr);
            }
        }

        // 4. Camera: always use the default "last 200 candles" reset
        // on interactive ticker switches. The saved camera is per-symbol
        // (not per symbol+timeframe), so restoring it here would apply
        // a D1 camera to a 5m chart and vice versa. Per-ticker camera
        // persistence needs per-(symbol, timeframe) storage — future work.

        // 5. Bind linked order panels to the same symbol FIRST — so
        //    panel_display_for_chart can find them by the new symbol.
        let source_link = self
            .charts
            .get(&chart_id)
            .map(|c| c.symbol_link)
            .unwrap_or(LinkMode::Unlinked);
        let order_targets: Vec<OrderPanelId> = crate::link::find_link_targets(
            source_link,
            self.order_panels.iter().map(|(id, p)| (*id, p.symbol_link)),
        );
        for op_id in order_targets {
            self.bind_panel_to_symbol(op_id, symbol.clone());
        }

        // 6. NOW resolve linked panel display state and fire EnsureDraftBracket.
        //    Panels are already bound to the new symbol, so the lookup works.
        //    Only create a bracket if bracket_mode is active (not X).
        let bracket_mode = self.tickers.get(&symbol).and_then(|ts| ts.bracket_mode());
        if bracket_mode.is_some() {
            let panel_info = self.panel_display_for_chart(chart_id);
            if let Some((side, entry_type)) = panel_info {
                let _ = self.update(Message::Ticker(
                    symbol.clone(),
                    crate::ticker_state::TickerMsg::EnsureDraftBracket { side, entry_type },
                ));
            }
        }
    }

    /// Bind an order panel to a symbol.
    ///
    /// Sets `bound_symbol` and the backward-compat `state.symbol` field,
    /// then hydrates the panel from `TickerState` if one exists.
    pub(crate) fn bind_panel_to_symbol(&mut self, panel_id: OrderPanelId, symbol: SymbolKey) {
        if let Some(panel) = self.order_panels.get_mut(&panel_id) {
            panel.bound_symbol = Some(symbol.clone());
            panel.state.symbol = symbol.as_str().to_string();
        }
        // Hydrate panel from TickerState if available.
        if let Some(ts) = self.tickers.get(&symbol) {
            if let Some(panel) = self.order_panels.get_mut(&panel_id) {
                // Sync bracket_mode → bracket_active (TickerState is truth).
                panel.state.bracket_active = ts.bracket_mode();
                // Sync the live bracket annotation ID.
                panel.state.bracket_annotation_id = ts.live_annotation_id();
                // Sync panel fields from bracket if present.
                if let Some(bracket) = ts.live_bracket() {
                    crate::order_panel::sync_panel_from_bracket(&mut panel.state, bracket);
                }
            }
        }
    }

    // ── Panel ↔ ticker reconciliation ───────────────────────────────

    /// Sync a panel edit to the ticker state machine.
    ///
    /// Called from the panel-action handler after any edit that sets
    /// `panel.state.dirty = true`. Builds an `EntryMemory` from the
    /// panel and sends it through `TickerState::apply()` via
    /// `Message::Ticker(... CommitEdit ...)`. The effect handler
    /// persists the updated state.
    pub(in crate::app) fn sync_panel_to_intent(&mut self, panel_id: midas_core::OrderPanelId) {
        let Some(panel) = self.order_panels.get(&panel_id) else {
            return;
        };
        let symbol_str = panel.state.symbol.clone();
        if symbol_str.is_empty() {
            return;
        }
        let key = SymbolKey::new(&symbol_str);
        let side = panel.state.side;
        let entry_type = panel.state.entry_type;

        // Update side/entry_type on the TickerState and persist.
        if let Some(ts) = self.tickers.get_mut(&key) {
            ts.apply(crate::ticker_state::TickerMsg::SetSide(side));
            ts.apply(crate::ticker_state::TickerMsg::SetEntryType(entry_type));
            self.ticker_persist.upsert(key.clone(), ts.clone());
        }
    }

    /// Sync a bracket drag to the ticker state machine.
    ///
    /// Called from the `ChartDragBracketLeg` handler after the
    /// annotation has been mutated in-place by the drag. Marks the
    /// ticker state as dirty for persistence.
    pub(in crate::app) fn sync_drag_to_intent(
        &mut self,
        symbol_at_drag_start: String,
        _annotation_id: AnnotationId,
    ) {
        let key = SymbolKey::new(&symbol_at_drag_start);
        // Mark ticker state dirty for persistence after drag.
        if let Some(ts) = self.tickers.get(&key) {
            self.ticker_persist.upsert(key, ts.clone());
        }
    }

    /// Look up the `(side, entry_type)` currently displayed by the
    /// order panel linked to `chart_id`.
    ///
    /// Preference order:
    ///
    /// 1. The first panel whose `source_chart` matches `chart_id`.
    /// 2. The first panel whose `symbol` matches the chart's symbol.
    ///
    /// Returns `None` when no panel is linked to the chart at all —
    /// the caller then skips reconciliation, because there is no UI
    /// surface the reducer should mirror.
    pub(in crate::app) fn panel_display_for_chart(
        &self,
        chart_id: ChartId,
    ) -> Option<(
        crate::order_panel::OrderSide,
        midas_chart::widget::order_bracket::EntryType,
    )> {
        let chart_symbol = self.charts.get(&chart_id).map(|c| c.symbol.clone());
        // (1) Direct `source_chart` link.
        if let Some(panel) = self
            .order_panels
            .values()
            .find(|p| p.state.source_chart == Some(chart_id))
        {
            return Some((panel.state.side, panel.state.entry_type));
        }
        // (2) Fallback: symbol match (case-insensitive).
        if let Some(sym) = chart_symbol.as_deref().filter(|s| !s.is_empty()) {
            if let Some(panel) = self
                .order_panels
                .values()
                .find(|p| p.state.symbol.eq_ignore_ascii_case(sym))
            {
                return Some((panel.state.side, panel.state.entry_type));
            }
        }
        None
    }

    /// Hydrate the order panel linked to `chart_id` from the intent
    /// store, if any. Called from `ActivateChart` so switching charts
    /// lands the panel on the last-used side/type/prices for that
    /// symbol. The `dirty` guard in [`crate::order_panel::OrderPanelState::hydrate_from_intent`]
    /// prevents clobbering an in-progress edit on the *same* symbol.
    pub(in crate::app) fn hydrate_order_panel_for_chart(&mut self, chart_id: ChartId) {
        let Some(chart) = self.charts.get(&chart_id) else {
            return;
        };
        let symbol = chart.symbol.clone();
        if symbol.is_empty() {
            return;
        }
        let key = SymbolKey::new(&symbol);
        let Some(ticker_state) = self.tickers.get(&key) else {
            return;
        };

        let last_price = self.market_cache.get(&key).and_then(|s| s.last_price);

        // Find panels whose `source_chart` links to this chart.
        let ticker_state = ticker_state.clone();
        for panel in self.order_panels.values_mut() {
            if panel.state.source_chart == Some(chart_id) {
                panel.state.hydrate_from_intent(&ticker_state, last_price);
            }
        }
    }

    // ── Snap + factory ──────────────────────────────────────────────

    /// Fire a one-shot GATR snap check for the active chart's symbol.
    ///
    /// Called from `ActivateChart` so that stale brackets get
    /// repositioned when the user switches back to a chart they haven't
    /// looked at in a while.
    pub(in crate::app) fn maybe_emit_snap_for_active_chart(&mut self) -> Task<Message> {
        let Some(sym) = self.active_chart_symbol() else {
            return Task::none();
        };
        let key = SymbolKey::new(&sym);
        if self
            .tickers
            .get(&key)
            .is_some_and(|ts| ts.is_snapped_this_session())
        {
            return Task::none();
        }
        let _ = self.update(Message::Ticker(
            key.clone(),
            crate::ticker_state::TickerMsg::MarkSnappedThisSession,
        ));
        let snap = self.market_cache.get(&key);
        let current_price = snap.as_ref().and_then(|s| s.last_price);
        let gatr_abs = snap.as_ref().and_then(|s| s.gatr_abs);
        if let Some(price) = current_price {
            self.update(Message::Ticker(
                key,
                crate::ticker_state::TickerMsg::MaybeSnap {
                    current_price: price,
                    gatr_abs,
                },
            ))
        } else {
            Task::none()
        }
    }

    /// Save the current camera viewport of a chart to its bound
    /// `TickerState`.
    ///
    /// Computes `was_at_live_edge` by checking whether the latest
    /// candle's timestamp falls within the visible time window.
    /// Called from `ChartPan`, `ChartZoom`, and `ChartZoomY` handlers.
    pub(in crate::app) fn save_camera_for_chart(&mut self, chart_id: ChartId) {
        let (sym, cam_snapshot, latest) = {
            let Some(chart) = self.charts.get(&chart_id) else {
                return;
            };
            let Some(ref sym) = chart.bound_symbol else {
                return;
            };
            let cam = &chart.chart_state.camera;
            (
                sym.clone(),
                (cam.time_start, cam.time_end, cam.price_low, cam.price_high),
                chart.latest_candle_time().unwrap_or(0.0),
            )
        };
        let (time_start, time_end, price_low, price_high) = cam_snapshot;
        let at_live_edge = latest > 0.0 && latest >= time_start && latest <= time_end;
        let _ = self.update(Message::Ticker(
            sym,
            crate::ticker_state::TickerMsg::SaveCameraState {
                time_start,
                time_end,
                price_low,
                price_high,
                was_at_live_edge: at_live_edge,
            },
        ));
    }

    /// Lazy factory access to a per-symbol `TickerState`. Creates a
    /// fresh default state if the symbol is not yet in the map.
    #[allow(dead_code)] // used by Slices 1+
    pub fn ticker_mut(&mut self, symbol: &SymbolKey) -> &mut crate::ticker_state::TickerState {
        self.tickers
            .entry(symbol.clone())
            .or_insert_with(|| crate::ticker_state::TickerState::new(symbol.clone()))
    }

    // ── Effect handler ──────────────────────────────────────────────

    /// Interpret the effects produced by `TickerState::apply()`.
    ///
    /// Called by the `Message::Ticker` arm of `update()`. Each effect
    /// is wired mechanically to annotation store mutations, panel syncs,
    /// broker submissions, or persistence marks.
    pub(in crate::app) fn handle_ticker_effects(
        &mut self,
        sym: &SymbolKey,
        effects: Vec<crate::ticker_state::TickerEffect>,
    ) {
        for effect in effects {
            match effect {
                crate::ticker_state::TickerEffect::ProjectBracket(ref bracket) => {
                    let ann_id_opt = self.tickers.get(sym).and_then(|s| s.live_annotation_id());
                    // Check if the annotation actually exists in the store.
                    // IDs are session-local; a stale ID from a prior session
                    // (loaded from redb) would silently fail the update.
                    let ann_exists = ann_id_opt
                        .map(|id| {
                            self.annotation_store
                                .get(sym.as_str())
                                .iter()
                                .any(|a| a.id == id)
                        })
                        .unwrap_or(false);
                    if let (Some(ann_id), true) = (ann_id_opt, ann_exists) {
                        // Update existing annotation.
                        self.annotation_store.update(sym.as_str(), ann_id, |ann| {
                            ann.kind = midas_chart::widget::AnnotationKind::OrderBracket(Box::new(
                                bracket.clone(),
                            ));
                            ann.presence = midas_chart::widget::Presence::Active;
                        });
                    } else {
                        // Stale or missing ID — clear and create fresh.
                        if let Some(s) = self.tickers.get_mut(sym) {
                            s.set_live_annotation_id(None);
                        }
                        // Add new annotation.
                        let new_id = self.annotation_store.add(
                            sym.as_str(),
                            midas_chart::widget::AnnotationKind::OrderBracket(Box::new(
                                bracket.clone(),
                            )),
                        );
                        if let Some(s) = self.tickers.get_mut(sym) {
                            s.set_live_annotation_id(Some(new_id));
                        }
                        // Sync the annotation ID to any panel tracking this symbol.
                        for panel in self.order_panels.values_mut() {
                            if panel.state.symbol.eq_ignore_ascii_case(sym.as_str()) {
                                panel.state.bracket_annotation_id = Some(new_id);
                            }
                        }
                    }
                    // Sync panel inputs from the bracket.
                    for panel in self.order_panels.values_mut() {
                        if panel.state.symbol.eq_ignore_ascii_case(sym.as_str()) {
                            crate::order_panel::sync_panel_from_bracket(&mut panel.state, bracket);
                        }
                    }
                    self.mark_levels_dirty_for_ticker(sym.as_str());
                }
                crate::ticker_state::TickerEffect::RemoveBracket(id) => {
                    self.annotation_store.remove(sym.as_str(), id);
                    // Clear the annotation ID on panels tracking this symbol.
                    for panel in self.order_panels.values_mut() {
                        if panel.state.symbol.eq_ignore_ascii_case(sym.as_str())
                            && panel.state.bracket_annotation_id == Some(id)
                        {
                            panel.state.bracket_annotation_id = None;
                        }
                    }
                    self.mark_levels_dirty_for_ticker(sym.as_str());
                }
                crate::ticker_state::TickerEffect::Toast { message, action } => {
                    // Route through the controller so all toast
                    // mutations land in one place (effects ignored —
                    // a synchronous Show produces no parent-visible
                    // effects).
                    let _ = self
                        .toasts
                        .update(crate::toast::ToastMsg::Show { message, action });
                }
                crate::ticker_state::TickerEffect::PersistDirty => {
                    if let Some(state) = self.tickers.get(sym) {
                        self.ticker_persist.upsert(sym.clone(), state.clone());
                    }
                }
                crate::ticker_state::TickerEffect::SubmitToBroker { ref bracket } => {
                    // Convert bracket to broker params and send.
                    if let Some(ref bridge) = self.broker_bridge {
                        let action = match bracket.side {
                            midas_chart::widget::order_bracket::BracketSide::Long => {
                                midas_broker::OrderAction::Buy
                            }
                            midas_chart::widget::order_bracket::BracketSide::Short => {
                                midas_broker::OrderAction::Sell
                            }
                        };
                        let quantity = bracket.quantity.unwrap_or(0.0);
                        let (entry_kind, entry_price, entry_stop_price) = match bracket.entry_type {
                            midas_chart::widget::order_bracket::EntryType::Market => {
                                (midas_broker::OrderKind::Market, None, None)
                            }
                            midas_chart::widget::order_bracket::EntryType::Limit => (
                                midas_broker::OrderKind::Limit,
                                Some(bracket.entry.line.price),
                                None,
                            ),
                            midas_chart::widget::order_bracket::EntryType::Stop => (
                                midas_broker::OrderKind::Stop,
                                None,
                                Some(bracket.entry.line.price),
                            ),
                            midas_chart::widget::order_bracket::EntryType::StopLimit => (
                                midas_broker::OrderKind::StopLimit,
                                Some(bracket.entry.line.price),
                                bracket.entry_stop_price,
                            ),
                        };
                        let broker_params = midas_broker::BracketParams {
                            symbol: sym.as_str().to_owned(),
                            con_id: None,
                            sec_type: midas_broker::SecurityType::Stock,
                            exchange: "SMART".to_string(),
                            currency: "USD".to_string(),
                            action,
                            quantity,
                            outside_rth: false,
                            take_profit: bracket.take_profit.as_ref().map(|tp| {
                                midas_broker::TakeProfitParams {
                                    price: tp.line.price,
                                    tif: None,
                                }
                            }),
                            stop_loss: bracket.stop_loss.as_ref().map(|sl| {
                                midas_broker::StopLossParams {
                                    stop_price: sl.line.price,
                                    limit_price: None,
                                    tif: None,
                                }
                            }),
                            reference_price: Some(bracket.entry.line.price),
                            strategy: None,
                            tags: Vec::new(),
                            entry_kind,
                            entry_price,
                            entry_stop_price,
                        };
                        if let Err(e) = bridge.create_bracket(broker_params) {
                            tracing::error!("Failed to submit bracket to broker: {e}");
                        }
                    }
                }
                crate::ticker_state::TickerEffect::ProjectLevel { .. } => {
                    self.mark_levels_dirty_for_ticker(sym.as_str());
                }
                crate::ticker_state::TickerEffect::RemoveLevel { .. } => {
                    self.mark_levels_dirty_for_ticker(sym.as_str());
                }
            }
        }

        // Sync bracket_mode from TickerState to all linked panels.
        // TickerState is the single source of truth; panel.bracket_active
        // is derived.
        if let Some(ts) = self.tickers.get(sym) {
            let mode = ts.bracket_mode();
            for panel in self.order_panels.values_mut() {
                if panel.state.symbol.eq_ignore_ascii_case(sym.as_str()) {
                    panel.state.bracket_active = mode;
                }
            }
        }
    }
}
