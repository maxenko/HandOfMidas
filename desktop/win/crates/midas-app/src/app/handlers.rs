//! Domain-grouped message handlers for [`MidasApp::update`].
//!
//! Each handler method receives a [`Message`] and returns `Task<Message>`.
//! The top-level `update()` in `app.rs` dispatches to these handlers based
//! on the message variant.

use super::*;

// ── Symbol / Data Loading ────────────────────────────────────────────

impl MidasApp {
    /// Handle symbol input, submission, timeframe selection, data load
    /// results, and provider selection messages.
    pub(crate) fn handle_symbol_data_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PanelSymbolInputChanged(chart_id, value) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.symbol_input = value;
                }
                Task::none()
            }

            Message::PanelSymbolSubmitted(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.gatr_hover = false;
                }
                let symbol = if let Some(chart) = self.charts.get(&chart_id) {
                    chart.symbol_input.trim().to_uppercase()
                } else {
                    return Task::none();
                };
                if symbol.is_empty() {
                    return Task::none();
                }
                // Tap the MRU at the user-action seam — before any Task
                // dispatch. `propagate_symbol_change` spreads the change
                // to linked charts and must not re-push per hop.
                self.push_recent_symbol(&symbol);
                let task = self.load_symbol_for_chart(chart_id, &symbol);
                self.mark_config_dirty();
                let propagate = self.propagate_symbol_change(chart_id, &symbol);
                Task::batch([task, propagate])
            }

            Message::PanelTimeframeSelected(chart_id, tf) => {
                self.focus_chart(chart_id);
                // Get the symbol before mutating, then regenerate data at new tf.
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();

                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.timeframe = tf;
                    chart.gatr_hover = false;
                    chart.chart_state.dirty.mark_camera();
                }

                let mut tasks: Vec<Task<Message>> = Vec::new();
                if !symbol.is_empty() {
                    if let Some(chart) = self.charts.get_mut(&chart_id) {
                        chart.load_state = LoadState::Loading;
                        chart.chart_state.dirty.mark_data();
                    }
                    tasks.push(self.load_chart_async(chart_id, &symbol, tf));
                }

                tasks.push(self.propagate_timeframe_change(chart_id, tf));
                self.mark_config_dirty();
                Task::batch(tasks)
            }

            Message::DataLoaded(chart_id, requested_symbol, gen, result) => {
                // Fool-proof stale-load guard: check BOTH the symbol name
                // AND the generation counter. If the chart has switched to
                // a different ticker since this load started, discard.
                if let Some(chart) = self.charts.get(&chart_id) {
                    let current = chart.symbol.to_uppercase();
                    if current != requested_symbol || chart.load_generation != gen {
                        tracing::debug!(
                            "discarding stale DataLoaded for {requested_symbol} \
                             (chart now on {current}, gen {gen} vs {})",
                            chart.load_generation
                        );
                        return Task::none();
                    }
                }
                match result {
                    Ok(buffer) => {
                        let mut loaded_symbol: Option<String> = None;
                        // Grab last close before buffer is moved.
                        let last_close = if buffer.is_empty() {
                            None
                        } else {
                            Some(buffer.closes[buffer.len() - 1] as f64)
                        };
                        // Try docked charts first, then floating charts.
                        // Look up the view state for this (symbol, timeframe)
                        // BEFORE the mutable borrow on self.charts.
                        let view_state = self
                            .charts
                            .get(&chart_id)
                            .and_then(|c| self.chart_views.get(&c.symbol, c.timeframe).cloned())
                            .unwrap_or_default();

                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            let sym = chart.symbol.clone();
                            let count = buffer.len();
                            let tf = chart.timeframe;
                            Self::apply_candle_data(chart, buffer, Some(&view_state));
                            self.status_message =
                                format!("{}: {} candles at {}", sym, count, tf.display_name());
                            loaded_symbol = Some(sym);
                        } else if chart_id == ChartId::new(0) {
                            // Floating chart sentinel: apply to the first
                            // floating chart that is in Loading state.
                            let default_view = crate::chart_view::ChartViewState::default();
                            for chart in self.floating_charts.values_mut() {
                                if matches!(chart.load_state, LoadState::Loading) {
                                    loaded_symbol = Some(chart.symbol.clone());
                                    Self::apply_candle_data(chart, buffer, Some(&default_view));
                                    break;
                                }
                            }
                        }

                        // Update zero-price Draft brackets for order panels
                        // on this symbol. When bracket_active is set but data
                        // wasn't loaded yet, entry.price will be 0.0.
                        if let (Some(ref sym), Some(price)) = (&loaded_symbol, last_close) {
                            self.update_zero_price_brackets(sym, price);
                        }

                        // Ensure D1 market snapshot exists for G.ATR display.
                        if let Some(sym) = loaded_symbol {
                            let key = crate::annotation_store::SymbolKey::new(&sym);
                            if self.market_cache.get(&key).is_none() {
                                return self.load_market_snapshot(&sym);
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.load_state = LoadState::Error(e.clone());
                            chart.data = None;
                            chart.chart_state.dirty.mark_data();
                        }
                        tracing::warn!(chart = %chart_id, error = %e, "data load failed");
                        self.status_message = format!("Load error: {e}");
                    }
                }
                Task::none()
            }

            Message::DataRestoredFromStartup(chart_id, requested_symbol, gen, result) => {
                // Same fool-proof guard as DataLoaded.
                if let Some(chart) = self.charts.get(&chart_id) {
                    let current = chart.symbol.to_uppercase();
                    if current != requested_symbol || chart.load_generation != gen {
                        tracing::debug!(
                            "discarding stale DataRestoredFromStartup for {requested_symbol} \
                             (chart now on {current})"
                        );
                        return Task::none();
                    }
                }
                match result {
                    Ok(buffer) => {
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            // Startup restore: load data without resetting camera
                            // (config already has the saved camera from last session).
                            Self::apply_candle_data(chart, buffer, None);
                        }
                        // Bind the chart through the single mutation
                        // point. This fires EnsureDraftBracket so any
                        // linked panel gets a bracket matching its
                        // current `(side, entry_type)`. THIS is the fix
                        // for the "no bracket on initial load" bug —
                        // `bind_chart_to_symbol` lazy-creates the
                        // TickerState and fires the bracket lifecycle.
                        let sym = self
                            .charts
                            .get(&chart_id)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_default();
                        if !sym.is_empty() {
                            let key = crate::annotation_store::SymbolKey::new(&sym);
                            self.bind_chart_to_symbol(chart_id, key);
                        }
                    }
                    Err(e) => {
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.load_state = LoadState::Error(e.clone());
                            chart.chart_state.dirty.mark_data();
                        }
                        tracing::warn!(
                            chart = %chart_id, error = %e,
                            "startup data restore failed"
                        );
                    }
                }
                Task::none()
            }

            Message::DataProviderSelected(name) => {
                if let Some(idx) = self.providers.find_data_provider_index(&name) {
                    if self.providers.set_active_data(idx) {
                        tracing::info!(provider = %name, "switched data provider");
                        self.mark_config_dirty();
                        let chart_task = self.reload_all_charts();
                        let market_task = self.load_all_watchlist_snapshots();
                        return Task::batch([chart_task, market_task]);
                    }
                }
                Task::none()
            }

            Message::OrderBrokerSelected(name) => {
                let idx = if name == "None" {
                    None
                } else {
                    self.providers.find_broker_index(&name)
                };
                if self.providers.set_active_broker(idx) {
                    tracing::info!(broker = %name, "switched order broker");
                    self.mark_config_dirty();
                }
                Task::none()
            }

            _ => unreachable!(),
        }
    }
}

// ── Chart Management ─────────────────────────────────────────────────

impl MidasApp {
    /// Handle adding, closing, activating charts, and layout presets.
    pub(crate) fn handle_chart_management_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddChart => {
                if let Some(focused) = self.workspace.focus {
                    if let Some((new_id, _new_pane)) =
                        self.workspace.split(pane_grid::Axis::Vertical, focused)
                    {
                        self.charts.insert(new_id, Self::make_empty_panel());
                        self.status_message = format!("Added {new_id}");
                        self.mark_config_dirty();
                    }
                }
                Task::none()
            }

            Message::CloseChart(id) => {
                if let Some(pane) = self.workspace.find_pane(id) {
                    if let Some(PanelContent::Chart(closed_id)) = self.workspace.close(pane) {
                        self.charts.remove(&closed_id);
                        if matches!(self.link_picker_open, Some((PickerTarget::Docked(pid), _)) if pid == closed_id)
                        {
                            self.link_picker_open = None;
                        }
                        self.status_message = format!("Closed {closed_id}");
                        return self.flush_config();
                    }
                }
                Task::none()
            }

            Message::ActivateChart(id) => {
                if let Some(pane) = self.workspace.find_pane(id) {
                    self.workspace.set_focus(pane);
                }
                // Bind through the single mutation point. Fires
                // EnsureDraftBracket so the linked panel's bracket
                // matches its current `(side, entry_type)`.
                let sym = self
                    .charts
                    .get(&id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();
                if !sym.is_empty() {
                    let key = crate::annotation_store::SymbolKey::new(&sym);
                    self.bind_chart_to_symbol(id, key);
                }
                // Slice 2 hydration — idempotent when the reducer
                // already re-hydrated.
                self.hydrate_order_panel_for_chart(id);
                Task::none()
            }

            Message::LayoutPreset(preset) => {
                self.link_picker_open = None;
                let new_ids = self.workspace.apply_preset(&preset);
                for id in &new_ids {
                    self.charts
                        .entry(*id)
                        .or_insert_with(Self::make_empty_panel);
                }
                let active_ids: std::collections::HashSet<ChartId> =
                    self.workspace.chart_ids().into_iter().collect();
                self.charts.retain(|id, _| active_ids.contains(id));
                // Clean up orphaned watchlist panels (presets create chart-only layouts).
                let active_wl_ids: std::collections::HashSet<WatchlistId> = self
                    .workspace
                    .panes
                    .panes
                    .values()
                    .filter_map(|s| match &s.content {
                        PanelContent::Watchlist(id) => Some(*id),
                        _ => None,
                    })
                    .collect();
                self.watchlists.retain(|id, _| active_wl_ids.contains(id));
                // Clean up orphaned order panels (presets create chart-only layouts).
                self.order_panels.clear();
                self.mark_config_dirty();
                Task::none()
            }

            _ => unreachable!(),
        }
    }
}

// ── Pane Grid ────────────────────────────────────────────────────────

impl MidasApp {
    /// Handle pane focus, resize, drag, split, and close messages.
    pub(crate) fn handle_pane_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PaneFocused(pane) => {
                self.workspace.set_focus(pane);
                // Single-source-of-truth refactor: the focus change
                // routes through the reducer's `MaybeSnapToGatr`
                // handler. The reducer corrects both surfaces (panel
                // `EntryMemory` and chart bracket annotation) in
                // lockstep by the same `plan.delta`. The existing
                // `maybe_emit_snap_for_active_chart` below still fires
                // the one-shot-per-session toast path.
                if let Some(sym) = self.active_chart_symbol() {
                    let key = crate::annotation_store::SymbolKey::new(&sym);
                    let snap = self.market_cache.get(&key);
                    if let Some(price) = snap.as_ref().and_then(|s| s.last_price) {
                        let gatr_abs = snap.as_ref().and_then(|s| s.gatr_abs);
                        let _ = self.update(Message::Ticker(
                            key,
                            crate::ticker_state::TickerMsg::MaybeSnap {
                                current_price: price,
                                gatr_abs,
                            },
                        ));
                    }
                }
                self.maybe_emit_snap_for_active_chart()
            }

            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                self.workspace.panes.resize(split, ratio);
                self.mark_config_dirty();
                Task::none()
            }

            Message::PaneDragged(pane_grid::DragEvent::Picked { pane }) => {
                self.workspace.set_focus(pane);
                Task::none()
            }

            Message::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                self.pending_drag = None;
                self.dragging_ticker = None;
                self.workspace.panes.drop(pane, target);
                self.mark_config_dirty();
                Task::none()
            }

            Message::PaneDragged(_) => {
                self.pending_drag = None;
                self.dragging_ticker = None;
                Task::none()
            }

            Message::PaneSplit(axis, pane) => {
                if let Some((new_id, _new_pane)) = self.workspace.split(axis, pane) {
                    self.charts.insert(new_id, Self::make_empty_panel());
                    self.status_message = format!("Split pane, added {new_id}");
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::PaneClose(pane) => {
                match self.workspace.close(pane) {
                    Some(PanelContent::Chart(closed_id)) => {
                        self.charts.remove(&closed_id);
                        if matches!(self.link_picker_open, Some((PickerTarget::Docked(pid), _)) if pid == closed_id)
                        {
                            self.link_picker_open = None;
                        }
                        self.status_message = format!("Closed {closed_id}");
                    }
                    Some(PanelContent::Watchlist(wl_id)) => {
                        self.watchlists.remove(&wl_id);
                        self.status_message = format!("Closed {wl_id}");
                    }
                    Some(PanelContent::Order(order_id)) => {
                        self.order_panels.remove(&order_id);
                        if matches!(self.link_picker_open, Some((PickerTarget::Order(pid), _)) if pid == order_id)
                        {
                            self.link_picker_open = None;
                        }
                        self.status_message = format!("Closed {order_id}");
                    }
                    Some(PanelContent::OrderBlotter(_blotter_id)) => {
                        // Legacy — live panes are `Account` after migration.
                        // If this fires it means a persisted layout from an
                        // older build rehydrated an OrderBlotter slot that
                        // slipped past migration. Nothing to clean up since
                        // we never populate `order_blotters` in this build.
                        self.status_message = "Closed legacy Orders pane".to_string();
                    }
                    Some(PanelContent::Account(account_id)) => {
                        self.account_panels.remove(&account_id);
                        if matches!(self.link_picker_open, Some((PickerTarget::Account(pid), _)) if pid == account_id)
                        {
                            self.link_picker_open = None;
                        }
                        self.status_message = format!("Closed {account_id}");
                    }
                    None => return Task::none(),
                }
                self.flush_config()
            }

            _ => unreachable!(),
        }
    }
}

// ── Chart Interaction ────────────────────────────────────────────────

impl MidasApp {
    /// Dispatch a `ChartAction` wrapped in `Message::Chart(id, action)`.
    ///
    /// Audit P2 #4: this is the SOLE site that knows how every
    /// `ChartAction` variant maps to MidasApp state mutations. The
    /// legacy `Message::Chart*` variants were deleted; this method
    /// owns the bodies that used to live in `handle_chart_interaction_msg`
    /// and `handle_bracket_msg` for chart-action-shaped variants.
    pub(crate) fn dispatch_chart_action(
        &mut self,
        chart_id: ChartId,
        action: midas_chart::ChartAction,
    ) -> Task<Message> {
        use midas_chart::ChartAction;
        match action {
            ChartAction::Pan { dx, dy } => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.apply_action(&ChartAction::Pan { dx, dy });
                    chart.camera_restored_pending = false;
                }
                self.save_camera_for_chart(chart_id);
                self.mark_config_dirty();
                Task::none()
            }
            ChartAction::Zoom { center_x, factor } => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let cam = &mut chart.chart_state.camera;
                    let pivot_time = cam.x_to_time(center_x);
                    let left_dt = pivot_time - cam.time_start;
                    let right_dt = cam.time_end - pivot_time;
                    cam.time_start = pivot_time - left_dt / factor;
                    cam.time_end = pivot_time + right_dt / factor;
                    chart.chart_state.dirty.mark_camera();
                }
                self.save_camera_for_chart(chart_id);
                self.mark_config_dirty();
                Task::none()
            }
            ChartAction::ZoomY { center_y, factor } => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let cam = &mut chart.chart_state.camera;
                    let pivot_price = cam.y_to_price(center_y);
                    let up_dp = cam.price_high - pivot_price;
                    let down_dp = pivot_price - cam.price_low;
                    cam.price_high = pivot_price + up_dp / factor;
                    cam.price_low = pivot_price - down_dp / factor;
                    chart.chart_state.dirty.mark_camera();
                }
                self.save_camera_for_chart(chart_id);
                self.mark_config_dirty();
                Task::none()
            }
            ChartAction::SetCrosshair { x, y } => self.apply_crosshair(chart_id, Some((x, y))),
            ChartAction::ClearCrosshair => self.apply_crosshair(chart_id, None),
            ChartAction::CreateLevel { price } => {
                self.focus_chart(chart_id);
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    let level_id = self.annotation_store.alloc_level_id();
                    self.annotation_store.add_level(
                        &ticker,
                        crate::annotation_store::StoredLevel {
                            level: midas_chart::levels::HorizontalLevel {
                                id: level_id,
                                line: midas_chart::widget::price_line::PriceLine {
                                    price,
                                    extent: midas_chart::widget::price_line::LineExtent::default(),
                                    stroke: midas_chart::widget::price_line::LineStroke {
                                        color: [0.85, 0.85, 0.85, 0.8],
                                        width: 1.0,
                                        style: midas_chart::widget::LineStyle::default(),
                                    },
                                },
                                label: None,
                                icon: midas_chart::LevelIcon::None,
                            },
                            locked: false,
                        },
                    );
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.level_placing = false;
                    self.placing_preview = None;
                    self.mark_config_dirty();
                }
                Task::none()
            }
            ChartAction::SetTimelineBorderRatio { ratio } => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.timeline_border_ratio = ratio as f32;
                    chart.chart_state.dirty.mark_data();
                }
                self.mark_config_dirty();
                Task::none()
            }
            ChartAction::SetVolumeScale { scale } => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.volume_scale = scale as f32;
                    chart.chart_state.dirty.mark_data();
                }
                self.mark_config_dirty();
                Task::none()
            }
            ChartAction::RightClickLevel { id, x, y } => {
                self.focus_chart(chart_id);
                let price_str = self
                    .annotation_store
                    .find_level(id.0)
                    .map(|(_, l)| midas_chart::format_price(l.line.price));
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.editing_level_id = Some(id.0);
                    chart.editing_level_screen_pos = Some((x, y));
                    if let Some(ps) = price_str {
                        chart.level_editor_price_input = ps;
                    }
                    chart.chart_state.selected_level = Some(id);
                    chart.chart_state.dirty.mark_levels();
                }
                Task::none()
            }
            ChartAction::DragLevel { id, new_price } => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.annotation_store
                        .update_level(&ticker, id.0, |level, _locked| {
                            level.line.price = new_price;
                        });
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                self.dragging_annotation = Some(id);
                Task::none()
            }
            ChartAction::SelectLevel { id } => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.selected_level = Some(id);
                    chart.chart_state.dirty.mark_levels();
                }
                Task::none()
            }
            ChartAction::DeselectLevel => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.selected_level = None;
                    chart.chart_state.dirty.mark_levels();
                }
                Task::none()
            }
            ChartAction::DeleteSelectedLevel => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let (Some(ticker), Some(chart)) = (ticker, self.charts.get_mut(&chart_id)) {
                    if let Some(sel_id) = chart.chart_state.selected_level {
                        let is_locked = self
                            .annotation_store
                            .levels_for(&ticker)
                            .iter()
                            .any(|l| l.id == sel_id.0 && l.locked);
                        if !is_locked {
                            chart.chart_state.selected_level = None;
                            self.annotation_store.remove_level(&ticker, sel_id.0);
                            self.mark_levels_dirty_for_ticker(&ticker);
                            self.mark_config_dirty();
                        }
                    }
                }
                Task::none()
            }
            ChartAction::CancelPlacing => {
                self.level_placing = false;
                self.placing_preview = None;
                Task::none()
            }
            ChartAction::PlacingPreview { price } => {
                if let Some(ticker) = self.chart_ticker(chart_id) {
                    self.placing_preview = Some((chart_id, ticker.to_owned(), price));
                }
                Task::none()
            }
            ChartAction::DragBracketLeg {
                annotation_id,
                leg,
                new_price,
            } => self.handle_chart_bracket_drag_leg(chart_id, annotation_id, leg, new_price),
            ChartAction::RightClickBracketLeg {
                annotation_id,
                leg,
                x,
                y,
            } => self.handle_chart_bracket_context_menu(chart_id, annotation_id, leg, x, y),
            ChartAction::CreateBracket {
                entry,
                tp,
                sl,
                side,
            } => self.handle_chart_create_bracket(chart_id, entry, tp, sl, side),
            ChartAction::DecoratorClick {
                annotation_id,
                group_id: _,
                item_path: _,
                action,
            } => {
                use midas_chart::widget::decorator::DecoratorAction;
                match action {
                    DecoratorAction::CloseAnnotation => {
                        self.handle_chart_bracket_cancel(chart_id, annotation_id)
                    }
                    DecoratorAction::RemoveStopLoss => {
                        self.handle_chart_bracket_cancel_sl(chart_id, annotation_id)
                    }
                    DecoratorAction::CreateStopLoss => {
                        self.handle_chart_bracket_toggle_sl(chart_id, annotation_id)
                    }
                    DecoratorAction::Submit => {
                        self.handle_chart_bracket_submit(chart_id, annotation_id)
                    }
                    DecoratorAction::Save => self.handle_chart_bracket_save(annotation_id),
                    DecoratorAction::TogglePin => self.handle_chart_bracket_toggle_pin(chart_id),
                    DecoratorAction::CreateTakeProfit
                    | DecoratorAction::CycleEntryType
                    | DecoratorAction::EditQuantity
                    | DecoratorAction::EditPrice
                    | DecoratorAction::ToggleLocked
                    | DecoratorAction::Custom(_) => Task::none(),
                }
            }
            // Variants the chart core can emit but the app layer
            // doesn't currently consume.
            ChartAction::AutoScaleY { .. }
            | ChartAction::StartMomentum { .. }
            | ChartAction::ApplyMomentum { .. }
            | ChartAction::StopMomentum
            | ChartAction::JumpToEnd
            | ChartAction::JumpToStart
            | ChartAction::Redraw => Task::none(),
        }
    }

    /// Apply a crosshair set/clear. Body shared between
    /// `ChartAction::SetCrosshair` and `ChartAction::ClearCrosshair`
    /// because both fold into a single `Option<(f32,f32)>` mutation
    /// + the cross-chart sync update.
    fn apply_crosshair(&mut self, chart_id: ChartId, pos: Option<(f32, f32)>) -> Task<Message> {
        if pos.is_some() {
            self.focus_chart(chart_id);
        }
        if let Some(chart) = self.charts.get_mut(&chart_id) {
            match pos {
                Some((x, y)) => chart.chart_state.crosshair.set_pos(x, y),
                None => chart.chart_state.crosshair.force_hide(),
            }
            #[allow(deprecated)]
            {
                chart.chart_state.crosshair_pos = pos;
            }
            chart.chart_state.dirty.mark_crosshair();
        }
        match pos {
            Some((x, y)) => {
                if let Some(chart) = self.charts.get(&chart_id) {
                    if let Some(ref data) = chart.data {
                        let cam = &chart.chart_state.camera;
                        let ts = if chart.chart_state.collapse_gaps {
                            let idx_f = cam.x_to_time(x);
                            let idx =
                                (idx_f.round().max(0.0) as usize).min(data.len().saturating_sub(1));
                            data.timestamps[idx]
                        } else {
                            let cursor_time = cam.x_to_time(x);
                            let idx = data.find_index_by_time(cursor_time as i64);
                            data.timestamps[idx]
                        };
                        let price = cam.y_to_price(y);
                        self.crosshair_sync = Some((chart_id, ts, price, chart.symbol.clone()));
                    }
                }
            }
            None => {
                if self
                    .crosshair_sync
                    .as_ref()
                    .is_some_and(|(src, _, _, _)| *src == chart_id)
                {
                    self.crosshair_sync = None;
                }
            }
        }
        Task::none()
    }

    /// Handle chart viewport, pan, zoom, crosshair, levels, toggles,
    /// level editor, and batch messages.
    pub(crate) fn handle_chart_interaction_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ChartViewportChanged(chart_id, old_w, old_h, new_w, new_h) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let cam = &mut chart.chart_state.camera;
                    if old_w > 0 && old_h > 0 {
                        let w_ratio = new_w as f64 / old_w as f64;
                        let h_ratio = new_h as f64 / old_h as f64;

                        // Horizontal: anchor right edge, expand/contract left.
                        let time_range = cam.time_end - cam.time_start;
                        cam.time_start = cam.time_end - time_range * w_ratio;

                        // Vertical: anchor center, expand/contract both edges.
                        let price_center = (cam.price_high + cam.price_low) / 2.0;
                        let half_range = (cam.price_high - cam.price_low) / 2.0 * h_ratio;
                        cam.price_high = price_center + half_range;
                        cam.price_low = price_center - half_range;
                    }
                    // Update canonical viewport so the snapshot matches
                    // actual bounds on the next frame.
                    cam.viewport_width = new_w;
                    cam.viewport_height = new_h;
                    // Clear crosshair during resize so it doesn't linger.
                    chart.chart_state.crosshair.force_hide();
                    #[allow(deprecated)]
                    {
                        chart.chart_state.crosshair_pos = None;
                    }
                    chart.chart_state.dirty.mark_camera();
                    chart.chart_state.dirty.mark_crosshair();
                }
                Task::none()
            }

            // ChartDragLevelEnd has no ChartAction equivalent — emitted
            // by the widget directly (not via action_to_message), so it
            // stays as a top-level Message variant.
            Message::ChartDragLevelEnd(_chart_id) => {
                self.dragging_annotation = None;
                Task::none()
            }

            Message::ChartClearAllLevels(chart_id) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.annotation_store.clear_levels(&ticker);
                    self.mark_levels_dirty_for_ticker(&ticker);
                    // Clear selection and editor on all charts for this ticker.
                    for chart in self.charts.values_mut() {
                        if chart.symbol == ticker {
                            chart.chart_state.selected_level = None;
                            chart.editing_level_id = None;
                            chart.editing_level_screen_pos = None;
                        }
                    }
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::ChartCloseLevelEditor(chart_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.editing_level_id = None;
                    chart.editing_level_screen_pos = None;
                }
                Task::none()
            }

            Message::ChartDeleteLevel(chart_id, level_id) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    let is_locked = self
                        .annotation_store
                        .levels_for(&ticker)
                        .iter()
                        .any(|l| l.id == level_id && l.locked);
                    if !is_locked {
                        self.annotation_store.remove_level(&ticker, level_id);
                        self.mark_levels_dirty_for_ticker(&ticker);
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.editing_level_id = None;
                            chart.editing_level_screen_pos = None;
                        }
                        self.mark_config_dirty();
                    }
                }
                Task::none()
            }

            Message::LevelEditorPriceChanged(chart_id, level_id, text) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.level_editor_price_input = text.clone();
                }
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Ok(price) = text.parse::<f64>() {
                        self.annotation_store
                            .update_level(&ticker, level_id, |level, _| {
                                level.line.price = price;
                            });
                        self.mark_levels_dirty_for_ticker(&ticker);
                        self.mark_config_dirty();
                    }
                }
                Task::none()
            }

            Message::LevelEditorPriceStep(chart_id, level_id, delta) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    let mut new_price: Option<f64> = None;
                    self.annotation_store
                        .update_level(&ticker, level_id, |level, _| {
                            level.line.price += delta;
                            new_price = Some(level.line.price);
                        });
                    if let Some(price) = new_price {
                        let price_str = midas_chart::format_price(price);
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.level_editor_price_input = price_str;
                        }
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorLabelChanged(chart_id, level_id, label_text) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.annotation_store
                        .update_level(&ticker, level_id, |level, _| {
                            level.label = if label_text.is_empty() {
                                None
                            } else {
                                Some(label_text.clone())
                            };
                        });
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorColorChanged(chart_id, level_id, color) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.annotation_store
                        .update_level(&ticker, level_id, |level, _| {
                            level.line.stroke.color = color;
                        });
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorThicknessChanged(chart_id, level_id, thickness) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.annotation_store
                        .update_level(&ticker, level_id, |level, _| {
                            level.line.stroke.width = thickness;
                        });
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorIconChanged(chart_id, level_id, icon) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.annotation_store
                        .update_level(&ticker, level_id, |level, _| {
                            level.icon = icon;
                        });
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorToggleLock(chart_id, level_id) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.annotation_store
                        .update_level(&ticker, level_id, |_, locked| {
                            *locked = !*locked;
                        });
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::DrawingPanelCreateLevel(chart_id) => {
                self.focus_chart(chart_id);
                self.level_placing = !self.level_placing;
                if !self.level_placing {
                    self.placing_preview = None;
                }
                Task::none()
            }

            Message::ToggleCollapseGaps(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let was_collapsed = chart.chart_state.collapse_gaps;
                    chart.chart_state.collapse_gaps = !was_collapsed;

                    if let Some(ref data) = chart.data {
                        let len = data.len();
                        if len > 0 {
                            let cam = &mut chart.chart_state.camera;
                            if !was_collapsed {
                                // Switching ON: convert camera from time-space to
                                // index-space so pan/zoom operate uniformly.
                                let start_idx =
                                    data.find_index_by_time(cam.time_start as i64) as f64;
                                let end_idx =
                                    data.find_index_by_time(cam.time_end as i64) as f64 + 1.0;
                                cam.time_start = start_idx;
                                cam.time_end = end_idx;
                                chart.chart_state.data_time_start = 0.0;
                                chart.chart_state.data_time_end = len as f64;
                            } else {
                                // Switching OFF: convert camera from index-space
                                // back to time-space.
                                let si =
                                    (cam.time_start.round() as usize).min(len.saturating_sub(1));
                                let ei = (cam.time_end.round() as usize).min(len.saturating_sub(1));
                                cam.time_start = data.timestamps[si] as f64;
                                cam.time_end = data.timestamps[ei] as f64;
                                chart.chart_state.data_time_start = data.timestamps[0] as f64;
                                chart.chart_state.data_time_end = data.timestamps[len - 1] as f64;
                            }
                        }
                    }
                    chart.chart_state.dirty.mark_camera();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ToggleVolumeProfile(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.show_volume_profile = !chart.chart_state.show_volume_profile;
                    chart.chart_state.dirty.mark_data();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ToggleLevels(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.show_levels = !chart.chart_state.show_levels;
                    chart.chart_state.dirty.mark_levels();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ResetChart(chart_id) => {
                self.focus_chart(chart_id);
                // Reload data at current timeframe to reset camera to default view.
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();
                let tf = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.timeframe)
                    .unwrap_or(midas_core::Timeframe::D1);
                self.mark_config_dirty();
                if !symbol.is_empty() {
                    if let Some(chart) = self.charts.get_mut(&chart_id) {
                        chart.load_state = LoadState::Loading;
                        chart.chart_state.dirty.mark_data();
                    }
                    return self.load_chart_async(chart_id, &symbol, tf);
                }
                Task::none()
            }

            Message::ChartBatch(msgs) => {
                let tasks: Vec<_> = msgs.into_iter().map(|msg| self.update(msg)).collect();
                Task::batch(tasks)
            }

            _ => unreachable!(),
        }
    }
}

// ── Order Panel ──────────────────────────────────────────────────────

impl MidasApp {
    /// Handle order panel add, messages, and symbol link messages.
    pub(crate) fn handle_order_panel_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddOrderPanel => {
                if let Some(focused) = self.workspace.focus {
                    let op_id = self.workspace.next_order_panel_id();
                    if let Some((chart_id, new_pane)) =
                        self.workspace.split(pane_grid::Axis::Vertical, focused)
                    {
                        // split() always creates a chart pane — replace it with an order panel.
                        if let Some(state) = self.workspace.panes.get_mut(new_pane) {
                            state.content = PanelContent::Order(op_id);
                        }
                        // Remove the chart entry that split() created.
                        self.charts.remove(&chart_id);
                        let symbol = self
                            .active_chart_id()
                            .and_then(|id| self.charts.get(&id))
                            .map(|p| p.symbol.clone())
                            .unwrap_or_default();
                        self.order_panels
                            .insert(op_id, crate::order_panel::OrderPanel::new(op_id, symbol));
                        self.status_message = format!("Added {op_id}");
                        return self.flush_config();
                    }
                }
                Task::none()
            }

            Message::OrderPanelMsg(panel_id, action) => {
                use crate::order_panel::OrderPanelAction;

                // SetBracketMode routes through TickerState as the
                // single source of truth.
                if let OrderPanelAction::SetBracketMode(mode) = action {
                    let symbol = self
                        .order_panels
                        .get(&panel_id)
                        .map(|p| p.state.symbol.clone())
                        .unwrap_or_default();
                    if symbol.is_empty() {
                        return Task::none();
                    }
                    let sym_key = crate::annotation_store::SymbolKey::new(&symbol);

                    // Seed the ticker state with market data so apply() can
                    // compute sensible default prices.
                    {
                        let (mc_price, mc_gatr) = self
                            .market_cache
                            .get(&sym_key)
                            .map(|s| (s.last_price, s.gatr_abs))
                            .unwrap_or((None, None));
                        let ts = self.ticker_mut(&sym_key);
                        ts.set_last_price(mc_price);
                        ts.set_gatr_abs(mc_gatr);
                    }

                    // Update the panel side when activating so the
                    // EnsureDraftBracket inside SetBracketMode uses it.
                    if let Some(side) = mode {
                        if let Some(p) = self.order_panels.get_mut(&panel_id) {
                            p.state.side = side;
                        }
                    }

                    return self.update(Message::Ticker(
                        sym_key,
                        crate::ticker_state::TickerMsg::SetBracketMode(mode),
                    ));
                }

                // SetEntryType routes through TickerState.
                if let OrderPanelAction::SetEntryType(new_type) = action {
                    let symbol = self
                        .order_panels
                        .get(&panel_id)
                        .map(|p| p.state.symbol.clone())
                        .unwrap_or_default();
                    if symbol.is_empty() {
                        return Task::none();
                    }
                    let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
                    {
                        let (mc_price, mc_gatr) = self
                            .market_cache
                            .get(&sym_key)
                            .map(|s| (s.last_price, s.gatr_abs))
                            .unwrap_or((None, None));
                        let ts = self.ticker_mut(&sym_key);
                        ts.set_last_price(mc_price);
                        ts.set_gatr_abs(mc_gatr);
                    }
                    if let Some(p) = self.order_panels.get_mut(&panel_id) {
                        p.state.entry_type = new_type;
                    }
                    // First set the entry type on the existing bracket (if any).
                    let _ = self.update(Message::Ticker(
                        sym_key.clone(),
                        crate::ticker_state::TickerMsg::SetEntryType(new_type),
                    ));
                    // If no bracket exists after SetEntryType (e.g., switching
                    // from Market to Stop Limit with no prior bracket), create
                    // one. EnsureDraftBracket is idempotent — if a bracket
                    // already exists, it returns early.
                    let side = self
                        .order_panels
                        .get(&panel_id)
                        .map(|p| p.state.side)
                        .unwrap_or(crate::order_panel::OrderSide::Buy);
                    return self.update(Message::Ticker(
                        sym_key,
                        crate::ticker_state::TickerMsg::EnsureDraftBracket {
                            side,
                            entry_type: new_type,
                        },
                    ));
                }

                // ConfirmYes needs broader access to self (broker_bridge, market_cache),
                // so handle it outside the panel borrow.
                if matches!(action, OrderPanelAction::ConfirmYes) {
                    let panel = match self.order_panels.get(&panel_id) {
                        Some(p) => p,
                        None => return Task::none(),
                    };
                    let state = &panel.state;

                    // Get last_price from market_cache (authoritative source).
                    let last_price = self
                        .market_cache
                        .get(&crate::annotation_store::SymbolKey::new(&state.symbol))
                        .and_then(|snap| snap.last_price);

                    let last_price = match last_price {
                        Some(p) => p,
                        None => {
                            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                                p.state.errors =
                                    vec![("price".into(), "Market data not loaded".into())];
                                p.state.showing_confirmation = false;
                            }
                            return Task::none();
                        }
                    };

                    // Resolve TP/SL prices from panel inputs.
                    let tp_price = if state.tp_enabled {
                        state.tp_value.parse::<f64>().ok().map(|val| {
                            crate::order_panel::resolve_price(
                                state.tp_mode,
                                val,
                                last_price,
                                state.side,
                                true,
                            )
                        })
                    } else {
                        None
                    };
                    let sl_price = if state.sl_enabled {
                        state.sl_value.parse::<f64>().ok().map(|val| {
                            crate::order_panel::resolve_price(
                                state.sl_mode,
                                val,
                                last_price,
                                state.side,
                                false,
                            )
                        })
                    } else {
                        None
                    };

                    let action = match state.side {
                        OrderSide::Buy => midas_chart::widget::order_bracket::BracketSide::Long,
                        OrderSide::Sell => midas_chart::widget::order_bracket::BracketSide::Short,
                    };
                    let quantity: f64 = state.quantity.parse().unwrap_or(100.0);
                    let symbol = state.symbol.clone();
                    let side = state.side;

                    tracing::info!(
                        "Order confirmed: {} {} {} (TP: {}, SL: {})",
                        match side {
                            OrderSide::Buy => "BUY",
                            OrderSide::Sell => "SELL",
                        },
                        quantity,
                        symbol,
                        tp_price.is_some(),
                        sl_price.is_some(),
                    );

                    // Send to broker engine.
                    if let Some(ref bridge) = self.broker_bridge {
                        let broker_params = midas_broker::BracketParams {
                            symbol: symbol.clone(),
                            con_id: None,
                            sec_type: midas_broker::SecurityType::Stock,
                            exchange: "SMART".to_string(),
                            currency: "USD".to_string(),
                            action: match side {
                                OrderSide::Buy => midas_broker::OrderAction::Buy,
                                OrderSide::Sell => midas_broker::OrderAction::Sell,
                            },
                            quantity,
                            outside_rth: false,
                            take_profit: tp_price.map(|p| midas_broker::TakeProfitParams {
                                price: p,
                                tif: None,
                            }),
                            stop_loss: sl_price.map(|p| midas_broker::StopLossParams {
                                stop_price: p,
                                limit_price: None,
                                tif: None,
                            }),
                            reference_price: Some(last_price),
                            strategy: None,
                            tags: Vec::new(),
                            entry_kind: midas_broker::OrderKind::Market,
                            entry_price: None,
                            entry_stop_price: None,
                        };
                        match bridge.create_bracket(broker_params) {
                            Ok(()) => {
                                tracing::info!(
                                    "CreateBracket sent to broker engine for {}",
                                    symbol
                                );
                            }
                            Err(e) => {
                                tracing::error!("Failed to send bracket to broker: {e}");
                                self.show_toast(format!("Broker error: {e}"));
                            }
                        }
                    } else {
                        tracing::warn!("No broker bridge: CreateBracket for {} not sent", symbol,);
                    }

                    self.status_message = format!(
                        "Order submitted: {} {} {}",
                        match side {
                            OrderSide::Buy => "BUY",
                            OrderSide::Sell => "SELL",
                        },
                        quantity,
                        symbol,
                    );

                    // Clear confirmation on the panel.
                    if let Some(p) = self.order_panels.get_mut(&panel_id) {
                        p.state.showing_confirmation = false;
                    }

                    // Create chart annotation via self-message so the bracket is
                    // visible on all charts displaying this symbol.
                    return self.update(Message::BrokerBracketCreated {
                        parent_id: uuid::Uuid::now_v7(),
                        take_profit_id: tp_price.map(|_| uuid::Uuid::now_v7()),
                        stop_loss_id: sl_price.map(|_| uuid::Uuid::now_v7()),
                        symbol,
                        action,
                        quantity,
                        entry_price: Some(last_price),
                        tp_price,
                        sl_price,
                    });
                }

                // Capture data for panel→chart annotation sync (applied
                // after the panel borrow drops).
                // Fields: (annotation_id, symbol, field_name, parsed_price)
                let mut annotation_sync: Option<(AnnotationId, String, &str, f64)> = None;

                // Slice 4 (panel-input extension): when the user flips
                // side (Buy ⇄ Sell), the compound-key rehydrate above
                // may have just swapped in a previously-stored absolute
                // price from the opposite-side bucket. Capture the
                // symbol so we can run the panel-input snap against
                // the current price after the panel borrow drops.
                let mut side_change_symbol: Option<String> = None;

                // Structural annotation changes that go beyond a single f64 price.
                // Applied after the panel borrow drops (below annotation_sync).
                enum StructuralSync {
                    None,
                    Side(AnnotationId, String, crate::order_panel::OrderSide),
                    Quantity(AnnotationId, String, String),
                    ToggleTp(AnnotationId, String, bool, f64),
                    ToggleSl(AnnotationId, String, bool, f64),
                }
                let mut structural_sync = StructuralSync::None;

                // Snapshot the ticker state up-front so SetSide can
                // soft-rehydrate the panel from the new compound-key
                // bucket without re-borrowing `self`. Panels that are
                // not linked to a ticker state get `None` and fall back
                // to the existing mutation path.
                let rehydrate_state = self
                    .order_panels
                    .get(&panel_id)
                    .map(|p| crate::annotation_store::SymbolKey::new(&p.state.symbol))
                    .and_then(|key| self.tickers.get(&key).cloned());

                if let Some(panel) = self.order_panels.get_mut(&panel_id) {
                    match action {
                        OrderPanelAction::SetSide(side) => {
                            // Rehydrate from the new compound
                            // bucket *without* bumping `dirty` — side
                            // toggles are soft reloads, not typed input.
                            if let Some(ref ts) = rehydrate_state {
                                panel.state.rehydrate_for_compound(
                                    ts,
                                    side,
                                    panel.state.entry_type,
                                );
                            } else {
                                panel.state.side = side;
                            }
                            if let Some(ann_id) = panel.state.bracket_annotation_id {
                                structural_sync =
                                    StructuralSync::Side(ann_id, panel.state.symbol.clone(), side);
                            }
                            side_change_symbol = Some(panel.state.symbol.clone());
                        }
                        OrderPanelAction::SetQuantity(qty) => {
                            if let Some(ann_id) = panel.state.bracket_annotation_id {
                                structural_sync = StructuralSync::Quantity(
                                    ann_id,
                                    panel.state.symbol.clone(),
                                    qty.clone(),
                                );
                            }
                            panel.state.quantity = qty;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::ToggleTp(enabled) => {
                            panel.state.tp_enabled = enabled;
                            panel.state.dirty = true;
                            if let Some(ann_id) = panel.state.bracket_annotation_id {
                                let last = panel.state.last_price.unwrap_or(0.0);
                                structural_sync = StructuralSync::ToggleTp(
                                    ann_id,
                                    panel.state.symbol.clone(),
                                    enabled,
                                    last,
                                );
                            }
                        }
                        OrderPanelAction::SetTpMode(mode) => {
                            panel.state.tp_mode = mode;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::SetTpValue(val) => {
                            if let (Some(ann_id), Ok(price)) =
                                (panel.state.bracket_annotation_id, val.parse::<f64>())
                            {
                                annotation_sync =
                                    Some((ann_id, panel.state.symbol.clone(), "tp", price));
                            }
                            panel.state.tp_value = val;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::ToggleSl(enabled) => {
                            panel.state.sl_enabled = enabled;
                            panel.state.dirty = true;
                            if let Some(ann_id) = panel.state.bracket_annotation_id {
                                let last = panel.state.last_price.unwrap_or(0.0);
                                structural_sync = StructuralSync::ToggleSl(
                                    ann_id,
                                    panel.state.symbol.clone(),
                                    enabled,
                                    last,
                                );
                            }
                        }
                        OrderPanelAction::SetSlMode(mode) => {
                            panel.state.sl_mode = mode;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::SetSlValue(val) => {
                            if let (Some(ann_id), Ok(price)) =
                                (panel.state.bracket_annotation_id, val.parse::<f64>())
                            {
                                annotation_sync =
                                    Some((ann_id, panel.state.symbol.clone(), "sl", price));
                            }
                            panel.state.sl_value = val;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::SetSlType(sl_type) => {
                            panel.state.sl_type = sl_type;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::SetSlLimit(val) => {
                            panel.state.sl_limit_value = val;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::Submit => {
                            // Sync last_price from market_cache so validate_panel
                            // can check TP/SL direction against current price.
                            panel.state.last_price = self
                                .market_cache
                                .get(&crate::annotation_store::SymbolKey::new(
                                    &panel.state.symbol,
                                ))
                                .and_then(|snap| snap.last_price);
                            let errors = crate::order_panel::validate_panel(&panel.state);
                            let valid = errors.is_empty();
                            panel.state.errors = errors;
                            if valid {
                                panel.state.showing_confirmation = true;
                                // Successful validate → treat as end of the
                                // editing session; hydration from the same
                                // ticker is allowed again.
                                panel.state.dirty = false;
                            }
                        }
                        OrderPanelAction::ConfirmNo => {
                            panel.state.showing_confirmation = false;
                            panel.state.dirty = false;
                        }
                        OrderPanelAction::Dismiss => {
                            panel.state.showing_confirmation = false;
                            panel.state.dirty = false;
                        }
                        OrderPanelAction::SetLimitPrice(val) => {
                            if let (Some(ann_id), Ok(price)) =
                                (panel.state.bracket_annotation_id, val.parse::<f64>())
                            {
                                annotation_sync =
                                    Some((ann_id, panel.state.symbol.clone(), "limit", price));
                            }
                            panel.state.limit_price = val;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::SetStopPrice(val) => {
                            if let (Some(ann_id), Ok(price)) =
                                (panel.state.bracket_annotation_id, val.parse::<f64>())
                            {
                                annotation_sync =
                                    Some((ann_id, panel.state.symbol.clone(), "stop", price));
                            }
                            panel.state.stop_price = val;
                            panel.state.dirty = true;
                        }
                        OrderPanelAction::StepPrice { field, delta } => {
                            use crate::order_panel::PriceField;
                            let (current_str, field_tag) = match field {
                                PriceField::Tp => (&panel.state.tp_value, "tp"),
                                PriceField::Sl => (&panel.state.sl_value, "sl"),
                                PriceField::LimitPrice => (&panel.state.limit_price, "limit"),
                                PriceField::StopPrice => (&panel.state.stop_price, "stop"),
                            };
                            // Don't step from invalid input — leave the field as-is
                            // so the user can finish editing before scrolling.
                            if let Ok(current) = current_str.parse::<f64>() {
                                let new_price = (current + delta).max(0.0);
                                let new_str = format!("{:.2}", new_price);
                                if let Some(ann_id) = panel.state.bracket_annotation_id {
                                    annotation_sync = Some((
                                        ann_id,
                                        panel.state.symbol.clone(),
                                        field_tag,
                                        new_price,
                                    ));
                                }
                                match field {
                                    PriceField::Tp => panel.state.tp_value = new_str,
                                    PriceField::Sl => panel.state.sl_value = new_str,
                                    PriceField::LimitPrice => panel.state.limit_price = new_str,
                                    PriceField::StopPrice => panel.state.stop_price = new_str,
                                }
                                panel.state.dirty = true;
                            }
                        }
                        OrderPanelAction::ConfirmYes
                        | OrderPanelAction::SetBracketMode(_)
                        | OrderPanelAction::SetEntryType(_) => {
                            // Handled above (outside the panel borrow).
                            unreachable!();
                        }
                    }
                }

                // Structural sync (side, quantity, TP/SL toggle) via TickerState.
                match structural_sync {
                    StructuralSync::Side(_ann_id, ref symbol, side) => {
                        let sym_key = crate::annotation_store::SymbolKey::new(symbol);
                        let _ = self.update(Message::Ticker(
                            sym_key,
                            crate::ticker_state::TickerMsg::SetSide(side),
                        ));
                    }
                    StructuralSync::Quantity(_ann_id, ref symbol, ref qty_str) => {
                        if let Ok(qty) = qty_str.parse::<f64>() {
                            let sym_key = crate::annotation_store::SymbolKey::new(symbol);
                            let _ = self.update(Message::Ticker(
                                sym_key,
                                crate::ticker_state::TickerMsg::SetQuantity(qty),
                            ));
                        }
                    }
                    StructuralSync::ToggleTp(_ann_id, ref symbol, enabled, _last_price) => {
                        let sym_key = crate::annotation_store::SymbolKey::new(symbol);
                        let _ = self.update(Message::Ticker(
                            sym_key,
                            crate::ticker_state::TickerMsg::SetTpEnabled(enabled),
                        ));
                    }
                    StructuralSync::ToggleSl(_ann_id, ref symbol, enabled, _last_price) => {
                        let sym_key = crate::annotation_store::SymbolKey::new(symbol);
                        let _ = self.update(Message::Ticker(
                            sym_key,
                            crate::ticker_state::TickerMsg::SetSlEnabled(enabled),
                        ));
                    }
                    StructuralSync::None => {}
                }

                // Panel → Chart annotation sync (price fields) via TickerState.
                if let Some((_ann_id, symbol, field, price)) = annotation_sync {
                    let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
                    let role = match field {
                        "tp" => Some(midas_chart::widget::order_bracket::LegRole::TakeProfit),
                        "sl" => Some(midas_chart::widget::order_bracket::LegRole::StopLoss),
                        "limit" => Some(midas_chart::widget::order_bracket::LegRole::Entry),
                        "stop" => Some(midas_chart::widget::order_bracket::LegRole::StopTrigger),
                        _ => None,
                    };
                    if let Some(role) = role {
                        let _ = self.update(Message::Ticker(
                            sym_key,
                            crate::ticker_state::TickerMsg::SetLegPrice { role, price },
                        ));
                    }
                }

                // Single-source-of-truth refactor: a side flip may
                // have surfaced a stale absolute price from the
                // opposite compound bucket. Route through the
                // reducer's `MaybeSnapToGatr` handler so the intent
                // is corrected and the panel is re-hydrated from the
                // corrected memory before the sync below persists it.
                if let Some(sym) = side_change_symbol {
                    let key = crate::annotation_store::SymbolKey::new(&sym);
                    let snap = self.market_cache.get(&key);
                    if let Some(price) = snap.as_ref().and_then(|s| s.last_price) {
                        let gatr_abs = snap.as_ref().and_then(|s| s.gatr_abs);
                        let _ = self.update(Message::Ticker(
                            key.clone(),
                            crate::ticker_state::TickerMsg::MaybeSnap {
                                current_price: price,
                                gatr_abs,
                            },
                        ));
                    }

                    // Reconcile the live bracket so a Buy→Sell (or vice
                    // versa) toggle replaces any stale opposite-side
                    // bracket. The panel's display is canonical.
                    if let Some((panel_side, panel_entry_type)) = self
                        .order_panels
                        .get(&panel_id)
                        .map(|p| (p.state.side, p.state.entry_type))
                    {
                        let _ = self.update(Message::Ticker(
                            key.clone(),
                            crate::ticker_state::TickerMsg::EnsureDraftBracket {
                                side: panel_side,
                                entry_type: panel_entry_type,
                            },
                        ));
                        if let Some(new_ann_id) = self
                            .tickers
                            .get(&key)
                            .and_then(|ts| ts.live_annotation_id())
                        {
                            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                                p.state.bracket_annotation_id = Some(new_ann_id);
                            }
                        }
                    }
                }

                // Slice 3: route the post-edit panel state through the
                // ticker-intent reducer so the per-compound-key memory
                // is updated and any linked annotation is synchronized.
                // The reducer is idempotent for no-op edits.
                self.sync_panel_to_intent(panel_id);

                Task::none()
            }

            Message::OrderPanelSetSymbolLink(op_id, mode) => {
                self.link_picker_open = None;
                if let Some(panel) = self.order_panels.get_mut(&op_id) {
                    panel.symbol_link = mode;
                }
                // Adopt group symbol when joining a link group.
                let group_symbol = match mode {
                    LinkMode::Color(color) => self
                        .charts
                        .values()
                        .chain(self.floating_charts.values())
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(c) if c == color)
                                && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone()),
                    LinkMode::ListenAll => self
                        .charts
                        .values()
                        .chain(self.floating_charts.values())
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(_)) && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone()),
                    LinkMode::Unlinked => None,
                };
                if let Some(symbol) = group_symbol {
                    let key = crate::annotation_store::SymbolKey::new(&symbol);
                    self.bind_panel_to_symbol(op_id, key);
                }
                self.flush_config()
            }

            _ => unreachable!(),
        }
    }
}

// ── Account Panel ────────────────────────────────────────────────────

impl MidasApp {
    /// Dispatcher for every `Message::Account*` variant.
    pub(crate) fn handle_account_panel_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddAccountPanel => self.handle_add_account_panel(),

            Message::Account(account_id, acct_msg) => {
                use crate::account_panel::AccountMsg;
                use midas_core::config::AccountTab;
                match acct_msg {
                    AccountMsg::TabSelected(tab) => {
                        if let Some(p) = self.account_panels.get_mut(&account_id) {
                            p.active_tab = tab;
                        }
                        // Activating the Trade History tab on a fresh
                        // run (or after a restart) needs the cached
                        // rows populated before the next `view()` —
                        // broker events before this point may have
                        // bumped the blotter generation without any
                        // History tab being open to rebuild its cache.
                        if tab == AccountTab::TradeHistory {
                            self.rebuild_account_history_caches();
                        }
                        // Same logic for Positions — batches may have
                        // arrived before the user opened this tab.
                        if tab == AccountTab::Positions {
                            self.rebuild_account_positions_caches();
                        }
                        self.flush_config()
                    }
                    AccountMsg::Orders(gm) => self.handle_account_orders_grid(account_id, gm),
                    AccountMsg::PositionsBatchApplied(batch) => {
                        // Per-panel variant kept for symmetry with
                        // Orders/History messages. The store itself is
                        // app-wide, so all open panels see the result.
                        self.positions.apply_batch(&batch);
                        self.rebuild_account_positions_caches();
                        Task::none()
                    }
                    AccountMsg::Positions(pm) => self.handle_account_positions_msg(account_id, pm),
                    AccountMsg::DisconnectBannerDismissed => {
                        if let Some(p) = self.account_panels.get_mut(&account_id) {
                            p.disconnect_banner_ack = true;
                        }
                        Task::none()
                    }
                    AccountMsg::RecentClicked(symbol) => self.handle_account_recent_clicked(symbol),
                }
            }

            Message::AccountPositionsBatch(batch) => {
                // App-wide variant emitted by `positions_subscription`.
                // Identical effect to the per-panel
                // `PositionsBatchApplied` path — both paths land in the
                // same store so last-write-wins is idempotent.
                self.positions.apply_batch(&batch);
                self.rebuild_account_positions_caches();
                Task::none()
            }

            Message::AccountOrdersRowSelected(account_id, order_id, symbol) => {
                // Selection write happens FIRST — an unlinked panel still
                // needs its highlight updated locally, so the broadcast
                // short-circuit below must not skip the mutation.
                let Some(panel) = self.account_panels.get_mut(&account_id) else {
                    return Task::none();
                };
                panel.orders.selected_row = Some(order_id);
                let link = panel.orders.symbol_link;
                if matches!(link, LinkMode::Unlinked) {
                    return Task::none();
                }
                self.broadcast_symbol_to_link_group(link, &symbol)
            }

            Message::AccountOrdersSetSymbolLink(account_id, mode) => {
                self.link_picker_open = None;
                if let Some(p) = self.account_panels.get_mut(&account_id) {
                    p.orders.symbol_link = mode;
                }
                self.flush_config()
            }

            Message::AccountOrdersColumnResizeStart(account_id, col_idx) => {
                let ids = crate::order_blotter::columns::OrderBlotterColumn::ids();
                if col_idx >= ids.len() {
                    return Task::none();
                }
                let width = self
                    .account_panels
                    .get(&account_id)
                    .map(|p| p.orders.grid_state.column_width(ids[col_idx]))
                    .unwrap_or(80.0);
                // start_x is NaN until the first cursor-move lands.
                self.resizing_account_column = Some((account_id, col_idx, f32::NAN, width));
                Task::none()
            }

            Message::AccountOrdersColumnResizing(cursor_x) => {
                let Some((account_id, col, ref mut start_x, start_w)) =
                    self.resizing_account_column
                else {
                    return Task::none();
                };
                if start_x.is_nan() {
                    self.resizing_account_column = Some((account_id, col, cursor_x, start_w));
                    return Task::none();
                }
                let dx = cursor_x - *start_x;
                let new_w = (start_w + dx).max(24.0);
                let ids = crate::order_blotter::columns::OrderBlotterColumn::ids();
                if col < ids.len() {
                    if let Some(p) = self.account_panels.get_mut(&account_id) {
                        p.orders
                            .grid_state
                            .set_column_width(ids[col], new_w, 24.0, None);
                    }
                }
                Task::none()
            }

            Message::AccountOrdersColumnResizeEnd => {
                self.resizing_account_column = None;
                self.flush_config()
            }

            Message::AccountHistoryColumnResizeStart(account_id, col_idx) => {
                let ids = crate::account_panel::history_columns::HistoryColumn::ids();
                if col_idx >= ids.len() {
                    return Task::none();
                }
                let width = self
                    .account_panels
                    .get(&account_id)
                    .map(|p| p.history.grid_state.column_width(ids[col_idx]))
                    .unwrap_or(80.0);
                self.resizing_account_history_column = Some((account_id, col_idx, f32::NAN, width));
                Task::none()
            }

            Message::AccountHistoryColumnResizing(cursor_x) => {
                let Some((account_id, col, ref mut start_x, start_w)) =
                    self.resizing_account_history_column
                else {
                    return Task::none();
                };
                if start_x.is_nan() {
                    self.resizing_account_history_column =
                        Some((account_id, col, cursor_x, start_w));
                    return Task::none();
                }
                let dx = cursor_x - *start_x;
                let new_w = (start_w + dx).max(24.0);
                let ids = crate::account_panel::history_columns::HistoryColumn::ids();
                if col < ids.len() {
                    if let Some(p) = self.account_panels.get_mut(&account_id) {
                        p.history
                            .grid_state
                            .set_column_width(ids[col], new_w, 24.0, None);
                    }
                }
                Task::none()
            }

            Message::AccountHistoryColumnResizeEnd => {
                // Widths are runtime-only in v1 (not persisted); nothing
                // to flush. Parity with the Orders-tab shape keeps the
                // drag state lifecycle identical.
                self.resizing_account_history_column = None;
                Task::none()
            }

            Message::AccountRecentsColumnResizeStart(account_id, col_idx) => {
                let ids = crate::account_panel::recents_tab::column_ids();
                if col_idx >= ids.len() {
                    return Task::none();
                }
                let width = self
                    .account_panels
                    .get(&account_id)
                    .map(|p| p.recents.grid_state.column_width(ids[col_idx]))
                    .unwrap_or(80.0);
                self.resizing_account_recents_column = Some((account_id, col_idx, f32::NAN, width));
                Task::none()
            }

            Message::AccountRecentsColumnResizing(cursor_x) => {
                let Some((account_id, col, ref mut start_x, start_w)) =
                    self.resizing_account_recents_column
                else {
                    return Task::none();
                };
                if start_x.is_nan() {
                    self.resizing_account_recents_column =
                        Some((account_id, col, cursor_x, start_w));
                    return Task::none();
                }
                let dx = cursor_x - *start_x;
                let new_w = (start_w + dx).max(24.0);
                let ids = crate::account_panel::recents_tab::column_ids();
                if col < ids.len() {
                    if let Some(p) = self.account_panels.get_mut(&account_id) {
                        p.recents
                            .grid_state
                            .set_column_width(ids[col], new_w, 24.0, None);
                    }
                }
                Task::none()
            }

            Message::AccountRecentsColumnResizeEnd => {
                self.resizing_account_recents_column = None;
                Task::none()
            }

            Message::AccountOrdersOpenColumnSelector(account_id) => {
                // Close any other popups first.
                self.link_picker_open = None;
                self.account_column_selector_open = Some(account_id);
                Task::none()
            }

            Message::AccountOrdersDismissColumnSelector => {
                self.account_column_selector_open = None;
                Task::none()
            }

            Message::AccountOrdersToggleColumn(account_id, col_id) => {
                if let Some(p) = self.account_panels.get_mut(&account_id) {
                    if p.orders.hidden_columns.contains(&col_id) {
                        p.orders.hidden_columns.remove(&col_id);
                    } else {
                        p.orders.hidden_columns.insert(col_id);
                    }
                }
                self.flush_config()
            }

            _ => unreachable!(),
        }
    }

    /// Refresh every Account-panel's Trade-History display-row cache
    /// against the current [`crate::order_blotter::OrderBlotter`]
    /// generation. Safe to call at any time — individual tabs
    /// early-return when their `last_seen_generation` already matches
    /// the blotter. Invoked after broker events mutate the blotter and
    /// on `TabSelected(TradeHistory)` so iced's `view()` is always
    /// free of mutation.
    pub(crate) fn rebuild_account_history_caches(&mut self) {
        let blotter = &self.order_blotter;
        for panel in self.account_panels.values_mut() {
            panel.history.rebuild_rows_if_stale(blotter);
        }
    }

    /// Refresh every Account-panel's Positions-tab display-row cache
    /// against the current [`crate::account_panel::PositionStore`]
    /// generation. Symmetrical with
    /// [`Self::rebuild_account_history_caches`]. Called from:
    ///
    /// - `Message::AccountPositionsBatch` (coalesced subscription path)
    /// - `Message::Account(_, AccountMsg::PositionsBatchApplied)` (per-panel
    ///   variant — currently identical effect; kept for symmetry)
    /// - Single-event `BrokerEvent::PositionUpdate` path (reconnect
    ///   backfill)
    /// - `AccountMsg::TabSelected(Positions)` (user-visible trigger)
    /// - `MidasApp::new` tail (so a restored Positions tab renders on
    ///   first frame without waiting for the subscription)
    ///
    /// Individual tabs early-return when their `last_seen_generation`
    /// already matches the store, so repeated calls are cheap.
    pub(crate) fn rebuild_account_positions_caches(&mut self) {
        let store = &self.positions;
        for panel in self.account_panels.values_mut() {
            panel.positions.rebuild_rows_if_stale(store);
        }
    }

    /// Handle a Positions-tab-originating message.
    ///
    /// - `CloseRequested(symbol)`: **stub only in v1.** Guards on
    ///   broker connectivity at the handler level (UI disable is not
    ///   sufficient per CLAUDE.md rule #3 — dev-harness can inject the
    ///   message directly). Never constructs a `BrokerCommand`. A unit
    ///   test pins that guarantee.
    /// - `Grid(msg)`: forwarded to the tab's grid state for row
    ///   selection / future resize. No effect on the store.
    pub(crate) fn handle_account_positions_msg(
        &mut self,
        account_id: midas_core::AccountPanelId,
        msg: crate::account_panel::PositionsMsg,
    ) -> Task<Message> {
        use crate::account_panel::PositionsMsg;
        match msg {
            PositionsMsg::CloseRequested(symbol) => self.handle_account_close_requested(symbol),
            PositionsMsg::Grid(gm) => {
                // Grid events on the Positions tab are row-select
                // only in v1 (headers are non-sortable, columns
                // non-resizable). Forward the handful of variants
                // the plan supports; everything else is an inert
                // passthrough so a future slice wiring up sort /
                // resize doesn't need to re-plumb the dispatcher.
                if let Some(p) = self.account_panels.get_mut(&account_id) {
                    match gm {
                        midas_grid::GridMessage::RowSelected(idx) => {
                            if let Some(row) = p.positions.cached_rows().get(idx) {
                                p.positions.selected_row = Some(row.symbol.clone());
                            }
                            p.positions.grid_state.selection.select(idx);
                        }
                        midas_grid::GridMessage::SortToggled(_) => {
                            // v1: Positions sort is fixed-ascending by
                            // symbol. Header clicks are inert.
                        }
                    }
                }
                Task::none()
            }
        }
    }

    /// Close-position request handler.
    ///
    /// **Stub in v1.** Guards on broker connectivity and writes an
    /// intent status message. Never constructs a `BrokerCommand`;
    /// the plan's non-goals explicitly list "Close-position wired to
    /// broker (stub only v1)". The decision function
    /// [`crate::account_panel::positions_msg::CloseDecision`] is
    /// unit-tested separately to pin the guarantee so a future
    /// refactor can't accidentally wire it up.
    pub(crate) fn handle_account_close_requested(&mut self, symbol: String) -> Task<Message> {
        use crate::account_panel::positions_msg::CloseDecision;
        let connected = self
            .broker_bridge
            .as_ref()
            .is_some_and(|b| b.is_engine_connected());
        let decision = CloseDecision::compute(connected, &symbol);
        // `may_emit_command()` is `false` in v1 for both decision
        // variants — the assertion below documents the intent at the
        // call site. Removing it in a future slice that wires the
        // action to the broker is where the guard transitions live.
        debug_assert!(
            !decision.may_emit_command(),
            "close-position stub must not authorize a broker command in v1"
        );
        match &decision {
            CloseDecision::RefusedDisconnected => {
                tracing::warn!(
                    "close position refused: broker disconnected (symbol={})",
                    symbol
                );
            }
            CloseDecision::Logged(sym) => {
                tracing::info!("close position intent: symbol={}", sym);
            }
        }
        self.status_message = decision.status_message();
        Task::none()
        // NO BrokerCommand emitted. Stub only.
    }

    /// Re-select the clicked Recent Instrument on the focused chart.
    ///
    /// No-ops when no chart is focused (e.g. the workspace contains
    /// only watchlists / Account panels). Reuses the exact same seam
    /// `handle_panel_symbol_submitted` uses — load for the focused
    /// chart + propagate through the link group — so the behaviour
    /// matches a manual symbol entry.
    fn handle_account_recent_clicked(&mut self, symbol: String) -> Task<Message> {
        let trimmed = symbol.trim().to_uppercase();
        if trimmed.is_empty() {
            return Task::none();
        }
        let Some(chart_id) = self.workspace.focused_chart_id() else {
            tracing::debug!("RecentClicked({symbol}) ignored — no focused chart in workspace");
            return Task::none();
        };
        self.focus_chart(chart_id);
        if let Some(chart) = self.charts.get_mut(&chart_id) {
            chart.symbol_input = trimmed.clone();
            chart.gatr_hover = false;
        }
        // Refreshing `last_seen` also moves the clicked symbol back to
        // the front of the MRU — matching what the user just did.
        self.push_recent_symbol(&trimmed);
        let load = self.load_symbol_for_chart(chart_id, &trimmed);
        self.mark_config_dirty();
        let propagate = self.propagate_symbol_change(chart_id, &trimmed);
        Task::batch([load, propagate])
    }

    /// Open a new Account pane in the workspace.
    ///
    /// Mirrors `handle_add_watchlist`: allocate a fresh ID, split off
    /// a new pane from the focused pane, drop the auto-created chart,
    /// install the Account panel, flush config.
    fn handle_add_account_panel(&mut self) -> Task<Message> {
        let Some(focused) = self.workspace.focus else {
            return Task::none();
        };
        let account_id = self.workspace.next_account_panel_id();
        if let Some((chart_id, new_pane)) = self.workspace.split(pane_grid::Axis::Vertical, focused)
        {
            // split() always creates a chart pane — replace with Account.
            if let Some(state) = self.workspace.panes.get_mut(new_pane) {
                state.content = PanelContent::Account(account_id);
            }
            self.charts.remove(&chart_id);
            self.account_panels.insert(
                account_id,
                crate::account_panel::AccountPanel::new(account_id, "Account"),
            );
            self.status_message = format!("Added {account_id}");
            return self.flush_config();
        }
        Task::none()
    }

    /// Route `GridMessage`s (sort toggles, row clicks) to an Account
    /// panel's Orders tab grid state.
    fn handle_account_orders_grid(
        &mut self,
        account_id: midas_core::AccountPanelId,
        message: midas_grid::GridMessage,
    ) -> Task<Message> {
        let Some(panel) = self.account_panels.get_mut(&account_id) else {
            return Task::none();
        };
        match message {
            midas_grid::GridMessage::SortToggled(col_id) => {
                use crate::order_blotter::columns::*;
                // Numeric / price / time columns default to descending
                // (most recent / largest first) to match trading-app UX.
                let default_dir = match col_id {
                    COL_QTY | COL_AVG_FILL | COL_LIMIT | COL_STOP | COL_TP | COL_SL
                    | COL_LAST_UPDATE | COL_ORDER_ID => midas_grid::SortDirection::Descending,
                    _ => midas_grid::SortDirection::Ascending,
                };
                panel.orders.grid_state.toggle_sort(col_id, default_dir);
            }
            midas_grid::GridMessage::RowSelected(_) => {
                // Row selection reserved for future per-row actions
                // (cancel / modify). v1 rows are read-only.
            }
        }
        Task::none()
    }
}

// ── Watchlist ────────────────────────────────────────────────────────

impl MidasApp {
    /// Handle watchlist add, ticker input, add/remove tickers, favorites,
    /// drag-and-drop, ticker selection, linking, column resize, and grid
    /// messages.
    pub(crate) fn handle_watchlist_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddWatchlist => {
                if let Some(focused) = self.workspace.focus {
                    let wl_id = self.workspace.next_watchlist_id();
                    if let Some((chart_id, new_pane)) =
                        self.workspace.split(pane_grid::Axis::Vertical, focused)
                    {
                        // split() always creates a chart pane — replace it with a watchlist.
                        if let Some(state) = self.workspace.panes.get_mut(new_pane) {
                            state.content = PanelContent::Watchlist(wl_id);
                        }
                        // Remove the chart entry that split() created.
                        self.charts.remove(&chart_id);
                        self.watchlists
                            .insert(wl_id, WatchlistPanel::new(wl_id, "Watchlist".into()));
                        self.status_message = format!("Added {wl_id}");
                        return self.flush_config();
                    }
                }
                Task::none()
            }

            Message::WatchlistTickerInputChanged(wl_id, value) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.add_ticker_input = value;
                }
                Task::none()
            }

            Message::WatchlistAddTicker(wl_id) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    let input = wl.add_ticker_input.clone();
                    if wl.add_ticker(&input) {
                        wl.add_ticker_input.clear();
                        // Always load fresh data — don't rely on potentially stale cache.
                        let symbol = input.trim().to_uppercase();
                        // Record the symbol in Recents as well so the MRU
                        // reflects every user-initiated ticker entry, not
                        // just chart selections.
                        self.push_recent_symbol(&symbol);
                        let task = self.load_market_snapshot(&symbol);
                        return Task::batch([self.flush_config(), task]);
                    }
                }
                Task::none()
            }

            Message::WatchlistRemoveTicker(wl_id, symbol) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    if wl.selected_symbol.as_deref() == Some(symbol.as_str()) {
                        wl.selected_symbol = None;
                    }
                    wl.remove_ticker(&symbol);
                    // Remove from cache if no watchlist still has this symbol.
                    let symbol_upper = symbol.to_uppercase();
                    let still_used = self
                        .watchlists
                        .values()
                        .any(|wl| wl.has_ticker(&symbol_upper));
                    if !still_used {
                        // Evict ticker state for this symbol from both
                        // the in-memory map and the persistence store.
                        let sym_key = crate::annotation_store::SymbolKey::new(&symbol_upper);
                        self.market_cache.remove(&sym_key);
                        self.tickers.remove(&sym_key);
                        self.ticker_persist.forget(&sym_key);
                    }
                    return self.flush_config();
                }
                Task::none()
            }

            Message::WatchlistAdjustFavorite(wl_id, symbol, delta) => {
                if delta == 0 {
                    return Task::none();
                }
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.adjust_favorite(&symbol, delta);
                    return self.flush_config();
                }
                Task::none()
            }

            Message::WatchlistTickerPressed(wl_id, symbol) => {
                self.pending_drag = Some(PendingDragState {
                    symbol: symbol.clone(),
                    wl_id,
                });
                // Fire confirmation after 250ms hold.
                Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    },
                    move |_| Message::WatchlistDragConfirm(symbol),
                )
            }

            Message::WatchlistDragConfirm(symbol) => {
                // Only promote if the pending drag matches (hasn't been cancelled).
                if self.pending_drag.as_ref().map(|p| &p.symbol) == Some(&symbol) {
                    self.pending_drag = None;
                    self.dragging_ticker = Some(DragTickerState {
                        symbol,
                        cursor_pos: self.cursor_position,
                    });
                }
                Task::none()
            }

            Message::WatchlistDragCancel => {
                self.pending_drag = None;
                self.dragging_ticker = None;
                Task::none()
            }

            Message::DragCursorMoved(pos) => {
                self.cursor_position = pos;
                if let Some(ref mut drag) = self.dragging_ticker {
                    drag.cursor_pos = pos;
                }
                Task::none()
            }

            Message::DragMouseUp => {
                // If still in pending state (released before 250ms), treat as
                // a regular ticker click — select the ticker in the watchlist.
                if let Some(pending) = self.pending_drag.take() {
                    return self.update(Message::WatchlistTickerSelected(
                        pending.wl_id,
                        pending.symbol,
                    ));
                }

                let drag = match self.dragging_ticker.take() {
                    Some(d) => d,
                    None => return Task::none(),
                };

                // Hit-test: find the chart pane under the cursor.
                // The pane grid sits below the toolbar (~32px) and above the
                // status bar (~26px). We compute pane regions relative to the
                // pane grid origin, then offset to window coordinates.
                const TOOLBAR_H: f32 = 32.0;
                const STATUS_H: f32 = 26.0;
                let (win_w, win_h) = self.window.size();
                let grid_w = win_w as f32;
                let grid_h = (win_h as f32 - TOOLBAR_H - STATUS_H).max(1.0);

                let regions = self.workspace.panes.layout().pane_regions(
                    1.0, // spacing
                    0.0, // min_size
                    iced::Size::new(grid_w, grid_h),
                );

                let cursor = drag.cursor_pos;
                // Translate cursor from window-space to pane-grid-space.
                let local_x = cursor.x;
                let local_y = cursor.y - TOOLBAR_H;

                for (pane, rect) in &regions {
                    if local_x >= rect.x
                        && local_x <= rect.x + rect.width
                        && local_y >= rect.y
                        && local_y <= rect.y + rect.height
                    {
                        if let Some(ps) = self.workspace.panes.get(*pane) {
                            if let Some(chart_id) = ps.chart_id() {
                                self.workspace.set_focus(*pane);
                                // Drag-drop of a watchlist ticker onto a
                                // chart is a user-visible symbol switch;
                                // record it in the MRU.
                                self.push_recent_symbol(&drag.symbol);
                                let load = self.load_symbol_for_chart(chart_id, &drag.symbol);
                                let propagate =
                                    self.propagate_symbol_change(chart_id, &drag.symbol);
                                self.mark_config_dirty();
                                return Task::batch([load, propagate]);
                            }
                        }
                    }
                }

                // Mouse-up was not on a chart pane — cancel drag.
                Task::none()
            }

            Message::WatchlistTickerSelected(wl_id, symbol) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.selected_symbol = Some(symbol.clone());
                }
                let wl_link = self
                    .watchlists
                    .get(&wl_id)
                    .map(|wl| wl.symbol_link)
                    .unwrap_or(LinkMode::Unlinked);
                self.broadcast_symbol_to_link_group(wl_link, &symbol)
            }

            Message::WatchlistSetSymbolLink(wl_id, mode) => {
                self.link_picker_open = None;
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.symbol_link = mode;
                }
                self.flush_config()
            }

            Message::WatchlistColumnResizeStart(wl_id, col, _) => {
                let ids = crate::watchlist::WATCHLIST_COLUMN_ORDER;
                if col >= ids.len() {
                    return Task::none();
                }
                let width = self
                    .watchlists
                    .get(&wl_id)
                    .map(|wl| wl.grid_state.column_width(ids[col]))
                    .unwrap_or(70.0);
                // start_x is NaN until the first on_move event provides cursor position.
                self.resizing_column = Some((wl_id, col, f32::NAN, width));
                Task::none()
            }

            Message::WatchlistColumnResizing(current_x) => {
                if let Some((wl_id, col, ref mut start_x, orig_w)) = self.resizing_column {
                    if start_x.is_nan() {
                        *start_x = current_x;
                    }
                    let delta = current_x - *start_x;
                    let new_w = (orig_w + delta).max(20.0);
                    let ids = crate::watchlist::WATCHLIST_COLUMN_ORDER;
                    if col < ids.len() {
                        if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                            wl.grid_state.set_column_width(ids[col], new_w, 20.0, None);
                        }
                    }
                }
                Task::none()
            }

            Message::WatchlistColumnResizeEnd => {
                self.resizing_column = None;
                self.flush_config()
            }

            Message::WatchlistGrid(wl_id, grid_msg) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    match grid_msg {
                        midas_grid::GridMessage::SortToggled(col_id) => {
                            // Per-column default direction: numeric columns start Descending.
                            let default_dir = match col_id {
                                crate::watchlist::COL_PRICE
                                | crate::watchlist::COL_CHANGE
                                | crate::watchlist::COL_GATR => {
                                    midas_grid::SortDirection::Descending
                                }
                                _ => midas_grid::SortDirection::Ascending,
                            };
                            wl.grid_state.toggle_sort(col_id, default_dir);
                        }
                        midas_grid::GridMessage::RowSelected(_) => {
                            // Row clicks emit WatchlistTickerSelected directly from
                            // the view (with the correct symbol from sorted order).
                            // This arm is reserved for Phase 2 keyboard navigation.
                        }
                    }
                }
                Task::none()
            }

            _ => unreachable!(),
        }
    }
}

// ── Market Data Cache ────────────────────────────────────────────────

impl MidasApp {
    /// Handle market snapshot load results and refresh timer.
    pub(crate) fn handle_market_data_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            // -- Market data cache --
            Message::MarketSnapshotLoaded(symbol, Ok(buffer)) => {
                // Insert if any watchlist or chart still references this symbol.
                let in_watchlist = self.watchlists.values().any(|wl| wl.has_ticker(&symbol));
                let in_chart = self.charts.values().any(|c| c.symbol == symbol)
                    || self.floating_charts.values().any(|c| c.symbol == symbol);
                let key = crate::annotation_store::SymbolKey::new(&symbol);
                if in_watchlist || in_chart {
                    let snapshot = crate::market_cache::snapshot_from_candles(&buffer);
                    self.market_cache.insert(key.clone(), snapshot);
                }

                // Sync draft bracket entry prices for Market-type brackets via TickerState.
                if let Some(new_price) = self.market_cache.get(&key).and_then(|s| s.last_price) {
                    // Check if the TickerState's live bracket needs a price update.
                    let needs_update = self
                        .tickers
                        .get(&key)
                        .and_then(|ts| ts.live_bracket())
                        .map(|b| {
                            b.entry_type == midas_chart::widget::order_bracket::EntryType::Market
                                && (new_price - b.entry.line.price).abs() >= 0.01
                        })
                        .unwrap_or(false);
                    if needs_update {
                        let _ = self.update(Message::Ticker(
                            key.clone(),
                            crate::ticker_state::TickerMsg::SetLegPrice {
                                role: midas_chart::widget::order_bracket::LegRole::Entry,
                                price: new_price,
                            },
                        ));
                    }
                }

                // Slice 4: first time we see fresh market data for a
                // symbol in this session, evaluate the GATR snap rule.
                // The reducer's guards drop the vast majority of calls
                // so this is cheap. This is the path that fixes the
                // "$112 stop on a $14 chart" bug — a stale bracket is
                // repositioned to the current price the moment its
                // market data lands.

                // Fresh price data just landed. Update the TickerState
                // with the new market data, then ensure a draft bracket
                // exists for each linked panel. This handles the "no
                // bracket on initial app load" case: bind_chart_to_symbol
                // ran at startup before market data was available, so
                // EnsureDraftBracket had no price to work with. Now that
                // we have real prices, re-fire it through TickerState.
                let (cp, gatr) = self
                    .market_cache
                    .get(&key)
                    .map(|s| (s.last_price, s.gatr_abs))
                    .unwrap_or((None, None));
                if let Some(new_price) = cp {
                    let gatr_val = gatr.unwrap_or(new_price * 0.005);
                    let _ = self.update(Message::Ticker(
                        key.clone(),
                        crate::ticker_state::TickerMsg::UpdateMarketData {
                            last_price: new_price,
                            gatr_abs: Some(gatr_val),
                        },
                    ));
                }
                // Walk every panel linked to this symbol. If TickerState
                // has no live bracket yet AND bracket_mode is active,
                // fire EnsureDraftBracket with the panel's current
                // (side, entry_type). When bracket_mode is None (X),
                // skip — no brackets should appear.
                let ts_info = self
                    .tickers
                    .get(&key)
                    .map(|ts| (ts.live_bracket().is_none(), ts.bracket_mode().is_some()));
                let (no_bracket, mode_active) = ts_info.unwrap_or((true, false));
                if no_bracket && mode_active {
                    let targets: Vec<(
                        crate::order_panel::OrderSide,
                        midas_chart::widget::order_bracket::EntryType,
                    )> = self
                        .order_panels
                        .values()
                        .filter(|p| p.state.symbol.eq_ignore_ascii_case(key.as_str()))
                        .map(|p| (p.state.side, p.state.entry_type))
                        .collect();
                    for (panel_side, panel_entry_type) in targets {
                        let _ = self.update(Message::Ticker(
                            key.clone(),
                            crate::ticker_state::TickerMsg::EnsureDraftBracket {
                                side: panel_side,
                                entry_type: panel_entry_type,
                            },
                        ));
                    }
                }

                let already_snapped = self
                    .tickers
                    .get(&key)
                    .is_some_and(|ts| ts.is_snapped_this_session());
                if !already_snapped {
                    // Mark the flag up front; the ensuing MaybeSnap dispatch
                    // runs regardless of whether the market cache has a
                    // price yet, matching the pre-refactor semantics.
                    let _ = self.update(Message::Ticker(
                        key.clone(),
                        crate::ticker_state::TickerMsg::MarkSnappedThisSession,
                    ));
                    let snap = self.market_cache.get(&key);
                    if let Some(price) = snap.as_ref().and_then(|s| s.last_price) {
                        let gatr_abs = snap.as_ref().and_then(|s| s.gatr_abs);
                        return self.update(Message::Ticker(
                            key,
                            crate::ticker_state::TickerMsg::MaybeSnap {
                                current_price: price,
                                gatr_abs,
                            },
                        ));
                    }
                }
                Task::none()
            }
            Message::MarketSnapshotLoaded(_symbol, Err(e)) => {
                tracing::warn!("Failed to load market snapshot: {e}");
                Task::none()
            }
            Message::RefreshMarketData => {
                // Refresh all watchlist symbols, not just cached ones.
                // This retries any symbols whose initial load failed.
                let mut seen = std::collections::HashSet::new();
                let mut tasks = Vec::new();
                for wl in self.watchlists.values() {
                    for ticker in &wl.tickers {
                        if seen.insert(ticker.symbol.clone()) {
                            tasks.push(self.load_market_snapshot(&ticker.symbol));
                        }
                    }
                }
                if tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(tasks)
                }
            }

            _ => unreachable!(),
        }
    }
}

// ── Chart Linking ────────────────────────────────────────────────────

impl MidasApp {
    /// Handle symbol/timeframe link setting for docked and floating
    /// charts, and the link picker toggle/dismiss.
    pub(crate) fn handle_link_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            // -- Chart linking --
            Message::SetSymbolLink(handle, mode) => {
                self.link_picker_open = None;

                // Write the new mode into the right storage map.
                match handle {
                    crate::app::ChartHandle::Docked(id) => {
                        if let Some(chart) = self.charts.get_mut(&id) {
                            chart.symbol_link = mode;
                        }
                    }
                    crate::app::ChartHandle::Floating(wid) => {
                        if let Some(chart) = self.floating_charts.get_mut(&wid) {
                            chart.symbol_link = mode;
                        }
                    }
                }

                // Adopt group symbol when joining a link group. The
                // scan sees every other chart panel (docked + floating)
                // regardless of which map this panel lives in.
                let group_symbol = match mode {
                    LinkMode::Color(color) => self
                        .all_chart_panels()
                        .filter(|(h, _)| *h != handle)
                        .map(|(_, p)| p)
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(c) if c == color)
                                && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone()),
                    LinkMode::ListenAll => self
                        .all_chart_panels()
                        .filter(|(h, _)| *h != handle)
                        .map(|(_, p)| p)
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(_)) && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone()),
                    LinkMode::Unlinked => None,
                };

                // Docked charts fully re-bind through
                // `load_symbol_for_chart` (which resets load_generation,
                // clears data, rebinds TickerState). Floating charts
                // use the thinner mutation via `apply_symbol_to_panel`
                // and a direct async load.
                let adopt_task = match (handle, group_symbol) {
                    (crate::app::ChartHandle::Docked(id), Some(symbol)) => {
                        self.load_symbol_for_chart(id, &symbol)
                    }
                    (crate::app::ChartHandle::Floating(wid), Some(symbol)) => {
                        let tf = self
                            .floating_charts
                            .get(&wid)
                            .map(|c| c.timeframe)
                            .unwrap_or(Timeframe::D1);
                        let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
                        if let Some(chart) = self.floating_charts.get_mut(&wid) {
                            crate::app::apply_symbol_to_panel(chart, &symbol, sym_key);
                        }
                        self.load_floating_chart_async(wid, &symbol, tf)
                    }
                    (_, None) => Task::none(),
                };

                // Only docked charts persist; floating layout is not
                // saved to disk.
                if matches!(handle, crate::app::ChartHandle::Docked(_)) {
                    self.mark_config_dirty();
                }
                adopt_task
            }

            Message::SetTimeframeLink(handle, mode) => {
                self.link_picker_open = None;

                // Write the new mode into the right storage map.
                match handle {
                    crate::app::ChartHandle::Docked(id) => {
                        if let Some(chart) = self.charts.get_mut(&id) {
                            chart.timeframe_link = mode;
                        }
                    }
                    crate::app::ChartHandle::Floating(wid) => {
                        if let Some(chart) = self.floating_charts.get_mut(&wid) {
                            chart.timeframe_link = mode;
                        }
                    }
                }

                // Adopt group timeframe when joining a link group.
                let group_tf = match mode {
                    LinkMode::Color(color) => self
                        .all_chart_panels()
                        .filter(|(h, _)| *h != handle)
                        .map(|(_, p)| p)
                        .find(|p| matches!(p.timeframe_link, LinkMode::Color(c) if c == color))
                        .map(|p| p.timeframe),
                    LinkMode::ListenAll => self
                        .all_chart_panels()
                        .filter(|(h, _)| *h != handle)
                        .map(|(_, p)| p)
                        .find(|p| matches!(p.timeframe_link, LinkMode::Color(_)))
                        .map(|p| p.timeframe),
                    LinkMode::Unlinked => None,
                };

                if let Some(tf) = group_tf {
                    // Fetch current symbol + apply the new timeframe to
                    // whichever map owns this panel, then fire the
                    // matching async loader.
                    let symbol = match handle {
                        crate::app::ChartHandle::Docked(id) => self
                            .charts
                            .get(&id)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_default(),
                        crate::app::ChartHandle::Floating(wid) => self
                            .floating_charts
                            .get(&wid)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_default(),
                    };
                    match handle {
                        crate::app::ChartHandle::Docked(id) => {
                            if let Some(chart) = self.charts.get_mut(&id) {
                                chart.timeframe = tf;
                            }
                            if !symbol.is_empty() {
                                if let Some(chart) = self.charts.get_mut(&id) {
                                    chart.load_state = LoadState::Loading;
                                    chart.chart_state.dirty.mark_data();
                                }
                                self.mark_config_dirty();
                                return self.load_chart_async(id, &symbol, tf);
                            }
                        }
                        crate::app::ChartHandle::Floating(wid) => {
                            if let Some(chart) = self.floating_charts.get_mut(&wid) {
                                chart.timeframe = tf;
                            }
                            if !symbol.is_empty() {
                                if let Some(chart) = self.floating_charts.get_mut(&wid) {
                                    chart.load_state = LoadState::Loading;
                                    chart.chart_state.dirty.mark_data();
                                }
                                return self.load_floating_chart_async(wid, &symbol, tf);
                            }
                        }
                    }
                }

                if matches!(handle, crate::app::ChartHandle::Docked(_)) {
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::ToggleLinkPicker(target, dim) => {
                if self.link_picker_open == Some((target, dim)) {
                    self.link_picker_open = None;
                } else {
                    self.link_picker_open = Some((target, dim));
                }
                Task::none()
            }

            Message::DismissLinkPicker => {
                self.link_picker_open = None;
                Task::none()
            }

            _ => unreachable!(),
        }
    }
}

// ── Window / Config / Floating ───────────────────────────────────────

impl MidasApp {
    /// Handle config save results, window close, pop-out, window
    /// geometry, main window opened, and floating window closed.
    pub(crate) fn handle_window_config_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ConfigSaved(result) => {
                match result {
                    Ok(()) => {}
                    Err(ref e) => {
                        tracing::warn!("Config save failed: {e}");
                        self.status_message = format!("Config save failed: {e}");
                        // Re-mark dirty so the next tick retries the save.
                        self.config_dirty = true;
                    }
                }
                Task::none()
            }

            Message::WindowCloseRequested => {
                if let Some(ref bridge) = self.broker_bridge {
                    let _ = bridge.shutdown();
                }
                // Blocking shutdown of the ticker-state persistence
                // layer. Signals the flush thread to perform a final
                // `Immediate` commit and blocks until it exits.
                self.ticker_persist.shutdown_blocking();
                // Same for the order-history store.
                self.order_history_persist.shutdown_blocking();
                self.flush_config()
            }

            Message::PopOut(pane) => {
                if let Some(pane_state) = self.workspace.panes.get(pane) {
                    if let Some(chart_id) = pane_state.chart_id() {
                        if let Some(chart) = self.charts.get(&chart_id) {
                            let floating_chart = chart.clone();
                            let title = if floating_chart.symbol.is_empty() {
                                "Hand of Midas".to_string()
                            } else {
                                format!(
                                    "{} - {}",
                                    floating_chart.symbol,
                                    floating_chart.timeframe.display_name()
                                )
                            };
                            let (win_id, open_task) = window::open(window::Settings {
                                size: iced::Size::new(800.0, 500.0),
                                ..window::Settings::default()
                            });
                            self.floating_charts.insert(win_id, floating_chart);
                            self.status_message = format!("Popped out {title} to new window");
                            self.mark_config_dirty();
                            return open_task.map(|_id| Message::Tick);
                        }
                    }
                }
                Task::none()
            }

            Message::FloatingWindowClosed(id) => {
                if matches!(self.link_picker_open, Some((PickerTarget::Floating(wid), _)) if wid == id)
                {
                    self.link_picker_open = None;
                }
                if let Some(chart) = self.floating_charts.remove(&id) {
                    tracing::info!("Floating window closed for {}", chart.symbol);
                }
                // If the main window was closed, exit the application.
                if self.window.main_window() == Some(id) {
                    return self.flush_config().chain(iced::exit());
                }
                Task::none()
            }

            _ => unreachable!(),
        }
    }

    /// Route a `WindowGeometryMsg` to the controller and translate
    /// any returned effects back into a parent `Task<Message>`.
    ///
    /// The single bridge between the window-geometry controller's
    /// local message + effect vocabulary and the parent's god-`Message`.
    /// Same shape as `dispatch_toast` (slice 0).
    pub(crate) fn dispatch_window(
        &mut self,
        msg: crate::window_geometry::WindowGeometryMsg,
    ) -> Task<Message> {
        if matches!(
            msg,
            crate::window_geometry::WindowGeometryMsg::MainWindowOpened(_)
        ) {
            tracing::info!("Main window opened (routed via WindowGeometry)");
        }
        let effects = self.window.update(msg);
        let mut tasks: Vec<Task<Message>> = Vec::new();
        for eff in effects {
            match eff {
                crate::window_geometry::Effect::MarkConfigDirty => self.mark_config_dirty(),
                crate::window_geometry::Effect::QueryMonitor(id) => {
                    tasks.push(window::monitor_size(id).map(|size| {
                        Message::Window(
                            crate::window_geometry::WindowGeometryMsg::MonitorSizeResult(size),
                        )
                    }));
                }
            }
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }
}

// ── G.ATR Hover ──────────────────────────────────────────────────────

impl MidasApp {
    /// Handle G.ATR hover enter/leave messages (candle dimming).
    pub(crate) fn handle_gatr_hover_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            // -- G.ATR hover highlight --
            Message::GatrHoverEnter(chart_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.gatr_hover = true;
                    chart.chart_state.dirty.mark_candles();
                } else {
                    // Floating windows use ChartId(0) which isn't in self.charts.
                    for fc in self.floating_charts.values_mut() {
                        fc.gatr_hover = true;
                        fc.chart_state.dirty.mark_candles();
                    }
                }
                Task::none()
            }
            Message::GatrHoverLeave(chart_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.gatr_hover = false;
                    chart.chart_state.dirty.mark_candles();
                } else {
                    for fc in self.floating_charts.values_mut() {
                        fc.gatr_hover = false;
                        fc.chart_state.dirty.mark_candles();
                    }
                }
                Task::none()
            }

            _ => unreachable!(),
        }
    }
}

// ── Bracket (drawing tool, drag, action buttons, context menu) ───────

impl MidasApp {
    /// Handle bracket creation from drawing tool, bracket leg drag,
    /// bracket action buttons (submit, save, cancel, toggle SL/pin),
    /// and bracket context menu messages.
    pub(crate) fn handle_bracket_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            // -- Bracket creation from drawing tool --
            // Bracket arms migrated to dedicated handle_chart_bracket_*
            // methods (audit P2 #4 batch 3) — the Message::ChartBracket*
            // wrapper variants are deleted; dispatch_chart_action calls
            // the methods directly when DecoratorClick / RightClickBracketLeg
            // / CreateBracket / DragBracketLeg actions arrive.

            // -- Bracket context menu (these still re-dispatch elsewhere) --
            Message::BracketContextCancel(parent_id) => {
                self.bracket_context_menu = None;

                // Look up the link but do NOT remove it yet.
                // The link stays alive until the engine confirms cancellation.
                if let Some(link) = self.order_annotation_links.get(&parent_id).cloned() {
                    // Route status change through TickerState.
                    let sym_key = crate::annotation_store::SymbolKey::new(&link.symbol);
                    let _ = self.update(Message::Ticker(
                        sym_key,
                        crate::ticker_state::TickerMsg::OrderCancelled,
                    ));
                    tracing::info!("Bracket {parent_id} cancel requested from context menu");

                    // Send cancellation to broker engine.
                    if let Some(ref bridge) = self.broker_bridge {
                        match bridge.cancel_bracket(parent_id) {
                            Ok(()) => {
                                tracing::info!(
                                    "CancelBracket sent to broker engine for {parent_id}"
                                );
                            }
                            Err(e) => {
                                tracing::error!("Failed to send CancelBracket to broker: {e}");
                            }
                        }
                    } else {
                        // No engine — remove link now since there will be no confirmation.
                        self.order_annotation_links.remove(&parent_id);
                    }
                }
                Task::none()
            }
            Message::BracketContextDismiss => {
                self.bracket_context_menu = None;
                Task::none()
            }

            _ => unreachable!(),
        }
    }

    // ── Bracket action handlers (audit P2 #4 batch 3) ────────────────
    //
    // Each method here used to live as a `Message::ChartBracket*` arm
    // inside `handle_bracket_msg`. The legacy variants are gone;
    // `dispatch_chart_action` calls these methods directly when the
    // matching `ChartAction` variant arrives.

    pub(crate) fn handle_chart_create_bracket(
        &mut self,
        chart_id: ChartId,
        entry: f64,
        tp: f64,
        sl: f64,
        side: midas_chart::widget::order_bracket::BracketSide,
    ) -> Task<Message> {
        let ticker = self.chart_ticker(chart_id).map(str::to_owned);
        if let Some(ticker) = ticker {
            use midas_chart::widget::level::LineStyle;
            use midas_chart::widget::order_bracket::*;

            let make_leg = |price: f64| BracketLeg {
                line: midas_chart::widget::PriceLine {
                    price,
                    extent: midas_chart::widget::LineExtent::FullWidth,
                    stroke: midas_chart::widget::LineStroke {
                        color: [0.0, 0.0, 0.0, 1.0],
                        width: 1.5,
                        style: LineStyle::Solid,
                    },
                },
                role: LegRole::Entry,
                projected_pnl: None,
                projected_pnl_pct: None,
            };
            let bracket = OrderBracket {
                entry: make_leg(entry),
                take_profit: Some(make_leg(tp)),
                stop_loss: Some(make_leg(sl)),
                side,
                status: BracketStatus::Draft,
                quantity: None,
                saved: false,
                filled_qty: None,
                entry_type: midas_chart::widget::order_bracket::EntryType::Market,
                entry_stop_price: None,
                wrong_side_warning: false,
            };
            let annotation_id = self.annotation_store.add(
                &ticker,
                midas_chart::widget::AnnotationKind::OrderBracket(Box::new(bracket)),
            );
            self.mark_levels_dirty_for_ticker(&ticker);
            tracing::info!(
                "Bracket drawn on chart: {annotation_id} for {ticker} \
                 ({side:?} entry={entry:.2} tp={tp:.2} sl={sl:.2})"
            );
            self.status_message =
                format!("Bracket placed on {ticker} ({side:?} E={entry:.2} TP={tp:.2} SL={sl:.2})");
        }
        Task::none()
    }

    pub(crate) fn handle_chart_bracket_drag_leg(
        &mut self,
        chart_id: ChartId,
        annotation_id: AnnotationId,
        leg: midas_chart::widget::order_bracket::LegRole,
        new_price: f64,
    ) -> Task<Message> {
        let ticker = self.chart_ticker(chart_id).map(str::to_owned);
        if let Some(ref ticker) = ticker {
            let sym_key = crate::annotation_store::SymbolKey::new(ticker);
            let _ = self.update(Message::Ticker(
                sym_key,
                crate::ticker_state::TickerMsg::DragLeg {
                    role: leg,
                    new_price,
                },
            ));
            if let Some(ref bridge) = self.broker_bridge {
                let order_id = self
                    .order_annotation_links
                    .values()
                    .find(|link| link.annotation_id == annotation_id.0)
                    .and_then(|link| match leg {
                        midas_chart::widget::order_bracket::LegRole::TakeProfit => link.tp_order_id,
                        midas_chart::widget::order_bracket::LegRole::StopLoss => link.sl_order_id,
                        _ => None,
                    });
                if let Some(order_id) = order_id {
                    if let Err(e) = bridge.modify_bracket_leg(order_id, new_price) {
                        tracing::error!("Failed to send ModifyBracketLeg to broker: {e}");
                    }
                }
            }
        }
        if let Some(symbol_at_drag_start) = ticker {
            self.sync_drag_to_intent(symbol_at_drag_start, annotation_id);
        }
        Task::none()
    }

    pub(crate) fn handle_chart_bracket_context_menu(
        &mut self,
        chart_id: ChartId,
        annotation_id: AnnotationId,
        leg: midas_chart::widget::order_bracket::LegRole,
        x: f32,
        y: f32,
    ) -> Task<Message> {
        self.bracket_context_menu = Some((chart_id, annotation_id.0, leg, x, y));
        Task::none()
    }

    pub(crate) fn handle_chart_bracket_toggle_sl(
        &mut self,
        chart_id: ChartId,
        ann_id: AnnotationId,
    ) -> Task<Message> {
        let symbol = self
            .charts
            .get(&chart_id)
            .map(|c| c.symbol.clone())
            .unwrap_or_default();
        if symbol.is_empty() {
            return Task::none();
        }
        let has_sl = self
            .annotation_store
            .get(&symbol)
            .iter()
            .find(|a| a.id == ann_id)
            .and_then(|a| match &a.kind {
                midas_chart::widget::AnnotationKind::OrderBracket(b) => Some(b.stop_loss.is_some()),
                _ => None,
            })
            .unwrap_or(false);
        let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
        self.update(Message::Ticker(
            sym_key,
            crate::ticker_state::TickerMsg::SetSlEnabled(!has_sl),
        ))
    }

    pub(crate) fn handle_chart_bracket_cancel_sl(
        &mut self,
        chart_id: ChartId,
        _ann_id: AnnotationId,
    ) -> Task<Message> {
        let symbol = self
            .charts
            .get(&chart_id)
            .map(|c| c.symbol.clone())
            .unwrap_or_default();
        if symbol.is_empty() {
            return Task::none();
        }
        let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
        self.update(Message::Ticker(
            sym_key,
            crate::ticker_state::TickerMsg::SetSlEnabled(false),
        ))
    }

    pub(crate) fn handle_chart_bracket_toggle_pin(&mut self, chart_id: ChartId) -> Task<Message> {
        let symbol = self
            .charts
            .get(&chart_id)
            .map(|c| c.symbol.clone())
            .unwrap_or_default();
        if symbol.is_empty() {
            return Task::none();
        }
        let key = crate::annotation_store::SymbolKey::new(&symbol);
        self.update(Message::Ticker(
            key,
            crate::ticker_state::TickerMsg::TogglePin,
        ))
    }

    pub(crate) fn handle_chart_bracket_save(&mut self, ann_id: AnnotationId) -> Task<Message> {
        let symbol = self
            .order_panels
            .iter()
            .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
            .map(|(_id, p)| p.state.symbol.clone());

        if let Some(symbol) = symbol {
            let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
            return self.update(Message::Ticker(
                sym_key,
                crate::ticker_state::TickerMsg::SaveBracket,
            ));
        }
        Task::none()
    }

    pub(crate) fn handle_chart_bracket_submit(
        &mut self,
        chart_id: ChartId,
        ann_id: AnnotationId,
    ) -> Task<Message> {
        let symbol = self
            .charts
            .get(&chart_id)
            .map(|c| c.symbol.clone())
            .unwrap_or_default();
        if symbol.is_empty() {
            return Task::none();
        }

        let panel_id = self
            .order_panels
            .iter()
            .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
            .map(|(id, _)| *id);

        let bracket_data = self
            .annotation_store
            .get(&symbol)
            .iter()
            .find(|a| a.id == ann_id)
            .and_then(|a| match &a.kind {
                midas_chart::widget::AnnotationKind::OrderBracket(b) => Some(b.as_ref().clone()),
                _ => None,
            });

        let Some(bracket) = bracket_data else {
            return Task::none();
        };

        let quantity: f64 = if let Some(q) = bracket.quantity {
            q
        } else if let Some(pid) = panel_id {
            self.order_panels
                .get(&pid)
                .and_then(|p| p.state.quantity.parse::<f64>().ok())
                .unwrap_or(0.0)
        } else {
            0.0
        };

        let errors = crate::order_panel::validate_bracket(&bracket, quantity);
        if !errors.is_empty() {
            if let Some(pid) = panel_id {
                if let Some(p) = self.order_panels.get_mut(&pid) {
                    p.state.errors = errors;
                }
            }
            return Task::none();
        }

        let entry_kind = match bracket.entry_type {
            midas_chart::widget::order_bracket::EntryType::Market => {
                midas_broker::OrderKind::Market
            }
            midas_chart::widget::order_bracket::EntryType::Limit => midas_broker::OrderKind::Limit,
            midas_chart::widget::order_bracket::EntryType::Stop => midas_broker::OrderKind::Stop,
            midas_chart::widget::order_bracket::EntryType::StopLimit => {
                midas_broker::OrderKind::StopLimit
            }
        };

        let (entry_price, entry_stop_price) = match bracket.entry_type {
            midas_chart::widget::order_bracket::EntryType::Market => (None, None),
            midas_chart::widget::order_bracket::EntryType::Limit => {
                (Some(bracket.entry.line.price), None)
            }
            midas_chart::widget::order_bracket::EntryType::Stop => {
                (None, Some(bracket.entry.line.price))
            }
            midas_chart::widget::order_bracket::EntryType::StopLimit => {
                (Some(bracket.entry.line.price), bracket.entry_stop_price)
            }
        };

        let action = match bracket.side {
            midas_chart::widget::order_bracket::BracketSide::Long => midas_broker::OrderAction::Buy,
            midas_chart::widget::order_bracket::BracketSide::Short => {
                midas_broker::OrderAction::Sell
            }
        };

        {
            let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
            let _ = self.update(Message::Ticker(
                sym_key,
                crate::ticker_state::TickerMsg::SubmitOrder,
            ));
        }

        if let Some(ref bridge) = self.broker_bridge {
            let broker_params = midas_broker::BracketParams {
                symbol: symbol.clone(),
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
                stop_loss: bracket
                    .stop_loss
                    .as_ref()
                    .map(|sl| midas_broker::StopLossParams {
                        stop_price: sl.line.price,
                        limit_price: None,
                        tif: None,
                    }),
                reference_price: Some(bracket.entry.line.price),
                strategy: None,
                tags: Vec::new(),
                entry_kind,
                entry_price,
                entry_stop_price,
            };
            match bridge.create_bracket(broker_params) {
                Ok(()) => {
                    tracing::info!(
                        "CreateBracket sent: chart={chart_id:?} ann={ann_id} \
                         symbol={symbol} qty={quantity} type={entry_kind:?}"
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to send bracket to broker: {e}");
                    {
                        let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
                        let _ = self.update(Message::Ticker(
                            sym_key,
                            crate::ticker_state::TickerMsg::OrderRejected {
                                reason: e.to_string(),
                            },
                        ));
                    }
                    if let Some(pid) = panel_id {
                        if let Some(p) = self.order_panels.get_mut(&pid) {
                            p.state.errors = vec![("broker".into(), format!("Broker error: {e}"))];
                        }
                    }
                    self.mark_levels_dirty_for_ticker(&symbol);
                    return Task::none();
                }
            }
        } else {
            tracing::info!(
                "Bracket submitted (no broker bridge): \
                 chart={chart_id:?} ann={ann_id} symbol={symbol} qty={quantity}"
            );
        }

        {
            let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
            let _ = self.update(Message::Ticker(
                sym_key,
                crate::ticker_state::TickerMsg::SetBracketMode(None),
            ));
        }
        if let Some(pid) = panel_id {
            if let Some(p) = self.order_panels.get_mut(&pid) {
                p.state.bracket_annotation_id = None;
            }
        }
        self.mark_levels_dirty_for_ticker(&symbol);
        Task::none()
    }

    pub(crate) fn handle_chart_bracket_cancel(
        &mut self,
        chart_id: ChartId,
        ann_id: AnnotationId,
    ) -> Task<Message> {
        let symbol = self
            .charts
            .get(&chart_id)
            .map(|c| c.symbol.clone())
            .unwrap_or_default();
        if symbol.is_empty() {
            return Task::none();
        }

        let panel_id = self
            .order_panels
            .iter()
            .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
            .map(|(id, _)| *id);

        let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
        let _ = self.update(Message::Ticker(
            sym_key,
            crate::ticker_state::TickerMsg::SetBracketMode(None),
        ));

        if let Some(pid) = panel_id {
            if let Some(p) = self.order_panels.get_mut(&pid) {
                p.state.bracket_annotation_id = None;
            }
        }
        self.mark_levels_dirty_for_ticker(&symbol);
        Task::none()
    }
}

// ── Broker Events ────────────────────────────────────────────────────

impl MidasApp {
    /// Handle broker bracket created/status changed, broker event
    /// received, and broker connection state changes.
    pub(crate) fn handle_broker_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            // -- Broker bracket events --
            Message::BrokerBracketCreated {
                parent_id,
                take_profit_id,
                stop_loss_id,
                symbol,
                action,
                quantity,
                entry_price,
                tp_price,
                sl_price,
            } => {
                let entry = entry_price.unwrap_or(0.0);
                let bracket = crate::order_panel::create_bracket_annotation(
                    action, entry, tp_price, sl_price, quantity,
                );

                // Route through TickerState: set the bracket, then
                // ProjectBracket effect creates the annotation.
                let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
                {
                    let state = self
                        .tickers
                        .entry(sym_key.clone())
                        .or_insert_with(|| crate::ticker_state::TickerState::new(sym_key.clone()));
                    state.set_live_bracket(Some(bracket.clone()));
                }
                // Dispatch a ProjectBracket through the effect handler to
                // create the annotation in the store.
                let _ = self.update(Message::Ticker(
                    sym_key.clone(),
                    crate::ticker_state::TickerMsg::OrderPending {
                        order_id: parent_id,
                    },
                ));

                // Read back the annotation ID that the effect handler set.
                let annotation_id = self
                    .tickers
                    .get(&sym_key)
                    .and_then(|s| s.live_annotation_id())
                    .map(|id| id.0)
                    .unwrap_or(0);

                // Store the mapping from annotation to broker order IDs.
                let link = crate::order_panel::OrderAnnotationLink {
                    annotation_id,
                    parent_order_id: parent_id,
                    tp_order_id: take_profit_id,
                    sl_order_id: stop_loss_id,
                    symbol: symbol.clone(),
                    side: action,
                    quantity,
                    created_at: std::time::Instant::now(),
                };
                self.order_annotation_links
                    .insert(link.parent_order_id, link);

                tracing::info!(
                    "Bracket annotation created: {annotation_id} for {symbol} \
                     (parent={parent_id}, entry={entry:.2})"
                );

                self.status_message =
                    format!("Bracket annotation {annotation_id} created for {symbol}");
                Task::none()
            }

            Message::BrokerEventReceived(boxed_event) => {
                use midas_broker::BrokerEvent;

                // Feed every broker event into the order blotter first,
                // so the UI row state is in sync regardless of which
                // specific arm below also processes the event. For every
                // touched row, write-through to the history persist so
                // the redb flush thread picks it up on its next tick.
                let touched_ids = self.order_blotter.apply(boxed_event.as_ref());
                // Refresh any Account-panel Trade-History caches whose
                // `last_seen_generation` fell behind. Cheap: it's a
                // u64 compare per panel and an iterator scan only when
                // the generation actually moved. Keeps iced's `view()`
                // strictly read-only.
                self.rebuild_account_history_caches();
                // Collect symbols from the touched rows so we can both
                // (a) persist the rows and (b) pre-warm any thumbnails
                // this event just introduced to the blotter. Unique
                // symbols per tick bound the work.
                let mut touched_symbols: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for oid in touched_ids {
                    if let Some(row) = self.order_blotter.row(oid) {
                        touched_symbols.insert(row.symbol.clone());
                        self.order_history_persist.upsert(oid, row.clone());
                    }
                }
                let mut thumb_tasks: Vec<Task<Message>> = Vec::new();
                for symbol in touched_symbols {
                    let tf = self.thumbnail_store.get(&symbol);
                    thumb_tasks.push(self.spawn_thumbnail_load(symbol, tf));
                }

                // Also log to the devloop event log so `wait_for_event`
                // can match on broker-side variants like "BracketCreated"
                // and "OrderFilled".
                #[cfg(feature = "dev_harness")]
                {
                    if let Some(log) = crate::dev_harness::event_log::try_global() {
                        log.append_broker(boxed_event.as_ref());
                    }
                }

                // Combine the optional thumbnail-load batch with whatever
                // the match below produces so every return path dispatches
                // the prewarm for new blotter symbols.
                let thumbnail_batch = if thumb_tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(thumb_tasks)
                };

                match *boxed_event {
                    BrokerEvent::BracketCreated {
                        parent_id,
                        take_profit_id,
                        stop_loss_id,
                        symbol,
                        action,
                        quantity,
                        tp_price,
                        sl_price,
                        reference_price,
                        ..
                    } => {
                        // Reconcile: find the existing annotation created locally
                        // by matching symbol + side + quantity using cached fields.
                        let side = crate::broker_bridge::translate_action_to_side(&action);

                        let mut candidates: Vec<_> = self
                            .order_annotation_links
                            .iter()
                            .filter(|(_, link)| {
                                link.symbol == symbol
                                    && link.side == side
                                    && (link.quantity - quantity).abs() < 0.01
                            })
                            .collect();
                        candidates.sort_by_key(|(_, link)| link.created_at);

                        let matching_key = candidates.first().map(|(key, _)| **key);

                        if let Some(old_key) = matching_key {
                            if let Some(mut link) = self.order_annotation_links.remove(&old_key) {
                                link.parent_order_id = parent_id;
                                link.tp_order_id = take_profit_id;
                                link.sl_order_id = stop_loss_id;
                                self.order_annotation_links.insert(parent_id, link);
                                tracing::info!(
                                    "Reconciled bracket annotation: provisional \
                                     {old_key} -> engine {parent_id} for {symbol}"
                                );
                            }
                        } else {
                            // No local annotation — create from engine event.
                            let entry_price = reference_price.unwrap_or(0.0);
                            let inner = self.update(Message::BrokerBracketCreated {
                                parent_id,
                                take_profit_id,
                                stop_loss_id,
                                symbol,
                                action: side,
                                quantity,
                                entry_price: Some(entry_price),
                                tp_price,
                                sl_price,
                            });
                            return Task::batch([inner, thumbnail_batch]);
                        }
                    }
                    BrokerEvent::BracketStatusChanged {
                        parent_id,
                        status,
                        entry_fill_price,
                    } => {
                        use midas_chart::widget::order_bracket::BracketStatus;
                        let chart_status = match status {
                            midas_broker::BracketLifecycleStatus::Submitted => {
                                BracketStatus::Pending
                            }
                            midas_broker::BracketLifecycleStatus::EntryFilled => {
                                BracketStatus::Active
                            }
                            midas_broker::BracketLifecycleStatus::TakeProfitHit => {
                                BracketStatus::Closed
                            }
                            midas_broker::BracketLifecycleStatus::StopLossHit => {
                                BracketStatus::Closed
                            }
                            midas_broker::BracketLifecycleStatus::Cancelled => {
                                BracketStatus::Cancelled
                            }
                            midas_broker::BracketLifecycleStatus::Rejected => {
                                BracketStatus::Cancelled
                            }
                            midas_broker::BracketLifecycleStatus::Error => BracketStatus::Cancelled,
                            midas_broker::BracketLifecycleStatus::Closed => BracketStatus::Closed,
                        };
                        let inner = self.update(Message::BrokerBracketStatusChanged {
                            parent_id,
                            status: chart_status,
                            entry_fill_price,
                        });
                        return Task::batch([inner, thumbnail_batch]);
                    }
                    BrokerEvent::OrderFilled {
                        order_id,
                        shares,
                        price,
                        commission,
                        ..
                    } => {
                        tracing::info!(
                            "Order filled: {order_id} {shares} shares @ {price:.2} \
                             (commission: {commission:?})"
                        );
                        let msg = format!(
                            "Filled: {shares} @ ${price:.2}{}",
                            commission
                                .map(|c| format!(" (comm ${c:.2})"))
                                .unwrap_or_default()
                        );
                        self.show_toast(msg);
                    }
                    BrokerEvent::OrderRejected { order_id, reason } => {
                        tracing::warn!("Order rejected: {order_id}: {reason}");
                        self.show_toast(format!("Order rejected: {reason}"));
                    }
                    BrokerEvent::OrderCancelled { order_id, reason } => {
                        tracing::info!("Order cancelled: {order_id}: {reason}");
                    }
                    BrokerEvent::Connected { server_version } => {
                        tracing::info!("Broker connected (server v{server_version})");
                        self.status_message = format!("Broker connected (v{server_version})");
                    }
                    BrokerEvent::Disconnected { reason } => {
                        tracing::warn!("Broker disconnected: {reason}");
                        self.status_message = format!("Broker disconnected: {reason}");
                    }
                    BrokerEvent::OrderValidationFailed { message, code } => {
                        tracing::warn!("Order validation failed [{code}]: {message}");
                        self.show_toast(format!("Validation: {message}"));
                    }
                    // Eager single-event position apply. Fires during
                    // reconnect backfills where events arrive before
                    // the coalesced `positions_subscription` has time
                    // to bucket them. The coalesced path
                    // (`Message::AccountPositionsBatch`) and this path
                    // both write to `self.positions`; last-write-wins
                    // is idempotent, so double-delivery is harmless.
                    BrokerEvent::PositionUpdate {
                        account,
                        symbol,
                        con_id,
                        quantity,
                        avg_cost,
                    } => {
                        let ev = BrokerEvent::PositionUpdate {
                            account,
                            symbol,
                            con_id,
                            quantity,
                            avg_cost,
                        };
                        self.positions.apply(&ev);
                        // Keep every Account-panel Positions cache in
                        // lock-step with the store — iced's `view()`
                        // is pure-`&self`, so the rebuild must happen
                        // here.
                        self.rebuild_account_positions_caches();
                    }
                    other => {
                        tracing::trace!("Unhandled broker event: {other:?}");
                    }
                }
                thumbnail_batch
            }

            Message::BrokerBracketStatusChanged {
                parent_id,
                status,
                entry_fill_price,
            } => {
                // Find the annotation link by parent broker order ID.
                if let Some(link) = self.order_annotation_links.get(&parent_id).cloned() {
                    let sym_key = crate::annotation_store::SymbolKey::new(&link.symbol);

                    // Route status change through TickerState.
                    use midas_chart::widget::order_bracket::BracketStatus;
                    let ticker_msg = match status {
                        BracketStatus::Active => crate::ticker_state::TickerMsg::OrderFilled {
                            filled_qty: link.quantity,
                            avg_price: entry_fill_price.unwrap_or(0.0),
                        },
                        BracketStatus::PartialFill => {
                            crate::ticker_state::TickerMsg::OrderPartialFill {
                                filled_qty: link.quantity,
                            }
                        }
                        BracketStatus::Cancelled => crate::ticker_state::TickerMsg::OrderCancelled,
                        BracketStatus::Pending => crate::ticker_state::TickerMsg::OrderPending {
                            order_id: parent_id,
                        },
                        _ => {
                            // Closed, Draft — handled as OrderCancelled for now.
                            crate::ticker_state::TickerMsg::OrderCancelled
                        }
                    };
                    let _ = self.update(Message::Ticker(sym_key, ticker_msg));

                    tracing::info!("Bracket status -> {status:?} (parent={parent_id})");

                    // Remove link when engine confirms cancellation (S9).
                    if status == BracketStatus::Cancelled {
                        self.order_annotation_links.remove(&parent_id);
                        tracing::info!(
                            "Annotation link removed for cancelled bracket \
                             {parent_id}"
                        );
                    }
                } else {
                    tracing::warn!("No annotation link found for parent_id={parent_id}");
                }
                Task::none()
            }

            Message::BrokerConnectionChanged(state_str) => {
                // Derive a simple connected/disconnected bool from the
                // display string; mirrors the status-bar predicate used
                // in `view_connection_indicator`. "Ready" is the steady
                // state after Connect + server handshake; "Connected"
                // (pre-handshake) is also treated as healthy.
                let now_connected = matches!(state_str.as_str(), "Ready" | "Connected");
                self.broker_connection_display = state_str;
                // Propagate the edge to every open Account pane so the
                // banner-ack flag resets on fresh disconnects.
                for panel in self.account_panels.values_mut() {
                    panel.apply_connection_change(now_connected);
                }
                Task::none()
            }

            _ => unreachable!(),
        }
    }
}

// ── Toast Notifications ──────────────────────────────────────────────

impl MidasApp {
    /// Handle show/dismiss/action toast messages.
    /// Route a `ToastMsg` to the controller and translate any returned
    /// effects back into a parent `Task<Message>`.
    ///
    /// This is the single bridge between the controller's local
    /// message + effect vocabulary and the parent's god-`Message`. The
    /// only mapping site for `Message::Toast` / `.map(Message::Toast)`
    /// in the codebase — keep it that way.
    pub(crate) fn dispatch_toast(&mut self, msg: crate::toast::ToastMsg) -> Task<Message> {
        let effects = self.toasts.update(msg);
        self.consume_toast_effects(effects)
    }

    /// Collapse a [`crate::toast::Effect`] vector into a single
    /// `Task<Message>`. Pulled out so [`Self::dispatch_toast`] and the
    /// `Tick` handler share one interpretation site.
    fn consume_toast_effects(&mut self, effects: Vec<crate::toast::Effect>) -> Task<Message> {
        let mut tasks: Vec<Task<Message>> = Vec::new();
        for eff in effects {
            match eff {
                crate::toast::Effect::FireParentMsg(boxed) => {
                    // Re-dispatch the embedded message synchronously so
                    // the toast-action UX stays single-tick. Bounded:
                    // `ToastController::update(ActionClicked)` takes
                    // state first, so any cycle through Toast hits an
                    // empty-state no-op on the second hop.
                    tasks.push(self.update(*boxed));
                }
            }
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }
}

// ── Tick / Ticker State ──────────────────────────────────────────────

impl MidasApp {
    /// Handle periodic tick (toast auto-dismiss, config save) and
    /// ticker state machine dispatch.
    pub(crate) fn handle_tick_ticker_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                // Auto-dismiss toast after TOAST_TTL_SECS seconds.
                let effects = self.toasts.tick(Instant::now());
                let toast_task = self.consume_toast_effects(effects);
                Task::batch([toast_task, self.maybe_save_config()])
            }

            Message::Ticker(sym, msg) => {
                assert!(!self.ticker_dispatch_active, "re-entrant ticker dispatch");
                self.ticker_dispatch_active = true;
                let state = self
                    .tickers
                    .entry(sym.clone())
                    .or_insert_with(|| crate::ticker_state::TickerState::new(sym.clone()));

                #[cfg(feature = "dev_harness")]
                let msg_for_log = msg.clone();

                let effects = state.apply(msg);

                #[cfg(feature = "dev_harness")]
                {
                    if let Some(log) = crate::dev_harness::event_log::try_global() {
                        log.append_ticker(sym.as_str(), &msg_for_log, &effects);
                    }
                }

                self.handle_ticker_effects(&sym, effects);
                self.ticker_dispatch_active = false;
                iced::Task::none()
            }

            _ => unreachable!(),
        }
    }
}

// ── Shared symbol-link broadcast ─────────────────────────────────────

impl MidasApp {
    /// Propagate a selected symbol to every chart, floating window,
    /// and order panel sharing `source_link`. Called from watchlist
    /// row clicks and blotter row clicks — behaviour is identical.
    ///
    /// `LinkMode::Unlinked` is a no-op. Returns batched data-load
    /// tasks for every chart that now needs to load the symbol.
    pub(crate) fn broadcast_symbol_to_link_group(
        &mut self,
        source_link: LinkMode,
        symbol: &str,
    ) -> Task<Message> {
        if matches!(source_link, LinkMode::Unlinked) {
            return Task::none();
        }

        use crate::link::find_link_targets;
        let mut tasks = Vec::new();

        // Collect every chart panel whose link mode matches, then
        // dispatch per-variant: docked charts go through
        // `load_symbol_for_chart` (full rebind); floating charts use
        // `apply_symbol_to_panel` + `load_floating_chart_async`.
        let chart_targets: Vec<crate::app::ChartHandle> = find_link_targets(
            source_link,
            self.all_chart_panels()
                .map(|(handle, p)| (handle, p.symbol_link)),
        );
        let sym_key = crate::annotation_store::SymbolKey::new(symbol);
        for handle in chart_targets {
            match handle {
                crate::app::ChartHandle::Docked(id) => {
                    tasks.push(self.load_symbol_for_chart(id, symbol));
                }
                crate::app::ChartHandle::Floating(wid) => {
                    let tf = self
                        .floating_charts
                        .get(&wid)
                        .map(|c| c.timeframe)
                        .unwrap_or(Timeframe::D1);
                    if let Some(chart) = self.floating_charts.get_mut(&wid) {
                        crate::app::apply_symbol_to_panel(chart, symbol, sym_key.clone());
                    }
                    tasks.push(self.load_floating_chart_async(wid, symbol, tf));
                }
            }
        }

        let order_targets: Vec<OrderPanelId> = find_link_targets(
            source_link,
            self.order_panels.iter().map(|(id, p)| (*id, p.symbol_link)),
        );
        for op_id in order_targets {
            let old_sym = self
                .order_panels
                .get(&op_id)
                .map(|p| p.state.symbol.clone())
                .unwrap_or_default();
            let recalled = self.handle_order_panel_symbol_change(op_id, &old_sym, symbol);
            self.bind_panel_to_symbol(op_id, sym_key.clone());
            if let Some(panel) = self.order_panels.get_mut(&op_id) {
                if !recalled {
                    panel.state.tp_value.clear();
                    panel.state.sl_value.clear();
                    panel.state.sl_limit_value.clear();
                }
                panel.state.last_price = None;
                panel.state.errors.clear();
            }
        }

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }
}
