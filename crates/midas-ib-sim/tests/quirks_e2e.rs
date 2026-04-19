//! Stage 05 — quirk modelling end-to-end integration tests.
//!
//! These tests exercise the *public quirks API* through the
//! [`CompositeQuirkGuard`] — the same surface the engine actor uses. They do
//! not spin up a full TCP server (that lives in `handshake_e2e`); instead
//! they prove the central invariants the plan calls out:
//!
//! * 51 messages in one second trip error 100 with `DisconnectAfterError`.
//! * 101 L1 subscriptions trip 10197 with `RejectRequest` (session survives).
//! * Each of the three historical-pacing regimes emits the canonical 162
//!   violation with a distinct human-readable tag.
//! * Farm-status bulletins are the canonical 2104/2106/2158 triplet and fire
//!   at the configured delay.
//! * `QuirksConfig::default()` loads cleanly from YAML round-trip.

use std::sync::Arc;
use std::time::Duration;

use midas_broker_core::{ContractSpec, SymbolKey};
use midas_ib_sim::engine::clock::{Clock, VirtualClock, VirtualInstant};
use midas_ib_sim::engine::types::{
    HistoricalReq, QuirkViolation, ReqId, SessionId, ViolationAction,
};
use midas_ib_sim::quirks::config::{MarketDataTypeKind, QuirksConfig};
use midas_ib_sim::quirks::error_codes;
use midas_ib_sim::quirks::farm_status::{ConnEvent, FarmStatusEmitter};
use midas_ib_sim::quirks::{CompositeQuirkGuard, QuirkCheckCtx, QuirkCheckKind, QuirkGuard};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn mk_guard_default() -> (Arc<VirtualClock>, CompositeQuirkGuard) {
    let clock = Arc::new(VirtualClock::new());
    let g =
        CompositeQuirkGuard::from_config(clock.clone() as Arc<dyn Clock>, &QuirksConfig::default());
    (clock, g)
}

fn mk_sym(i: i32) -> SymbolKey {
    SymbolKey {
        contract_id: i,
        symbol: format!("SYM{i}"),
    }
}

fn mk_hist(symbol: &str, what_to_show: &str, bar_size: &str) -> HistoricalReq {
    HistoricalReq {
        contract: ContractSpec::Stock {
            symbol: symbol.into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        },
        end_date_time: "".into(),
        duration: "1 D".into(),
        bar_size: bar_size.into(),
        what_to_show: what_to_show.into(),
        use_rth: true,
        format_date: 1,
        keep_up_to_date: false,
    }
}

// ---------------------------------------------------------------------------
// T1 quirk 1 — 50 msg/sec → error 100 → disconnect.
// ---------------------------------------------------------------------------

#[test]
fn spam_51_msgs_in_one_second_trips_error_100_and_disconnect_action() {
    let (_clock, mut g) = mk_guard_default();
    // Full bucket admits 50 messages.
    for i in 0..50 {
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: None,
            kind: QuirkCheckKind::MsgRate,
        })
        .unwrap_or_else(|e| panic!("msg {i} unexpectedly rejected: {e:?}"));
    }
    let err = g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: None,
            kind: QuirkCheckKind::MsgRate,
        })
        .expect_err("51st message must trip rate limit");
    match err {
        QuirkViolation::RateLimit {
            code,
            action,
            message,
        } => {
            assert_eq!(code, error_codes::MSG_RATE_EXCEEDED, "code is 100");
            assert_eq!(action, ViolationAction::DisconnectAfterError);
            assert!(
                message.contains("Max rate"),
                "message should be canonical: {message}"
            );
        }
        other => panic!("expected RateLimit, got {other:?}"),
    }
}

#[test]
fn msg_rate_bucket_refills_after_a_full_second() {
    let (clock, mut g) = mk_guard_default();
    for _ in 0..50 {
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: None,
            kind: QuirkCheckKind::MsgRate,
        })
        .unwrap();
    }
    assert!(g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: None,
            kind: QuirkCheckKind::MsgRate,
        })
        .is_err());
    clock.advance(VirtualInstant::from_secs(1));
    // Fresh bucket again.
    for _ in 0..50 {
        assert!(g
            .check(QuirkCheckCtx {
                session: SessionId(1),
                req_id: None,
                kind: QuirkCheckKind::MsgRate,
            })
            .is_ok());
    }
}

// ---------------------------------------------------------------------------
// T1 quirk 2 — 100 L1 line cap → 10197 → reject, session survives.
// ---------------------------------------------------------------------------

#[test]
fn subscribing_101_l1_tickers_trips_10197_without_disconnecting() {
    // 101 subscribes would also exhaust the 50 msg/sec bucket. Advance 1s
    // every ten subscriptions to keep the msg-rate quirk out of the way —
    // this test is about the L1 cap, not msg-rate interaction.
    let (clock, mut g) = mk_guard_default();
    for i in 0..100 {
        if i > 0 && i % 10 == 0 {
            clock.advance(VirtualInstant::from_millis((i as u64 / 10) * 1_000));
        }
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(i)),
            kind: QuirkCheckKind::L1Subscribe { symbol: &mk_sym(i) },
        })
        .unwrap_or_else(|e| panic!("ticker {i} rejected: {e:?}"));
    }
    // Keep msg-rate fresh for the 101st.
    clock.advance(VirtualInstant::from_secs(20));
    let err = g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(100)),
            kind: QuirkCheckKind::L1Subscribe {
                symbol: &mk_sym(100),
            },
        })
        .expect_err("101st subscription must trip line cap");
    match err {
        QuirkViolation::LineLimit { code, action, .. } => {
            assert_eq!(code, error_codes::LINE_CAP_OVERFLOW);
            // Critically — we reject the request, *not* tear down the session.
            assert_eq!(action, ViolationAction::RejectRequest);
        }
        other => panic!("expected LineLimit, got {other:?}"),
    }
    // Session-level state (msg_rate bucket) should still work.
    for _ in 0..40 {
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: None,
            kind: QuirkCheckKind::MsgRate,
        })
        .unwrap();
    }
}

#[test]
fn releasing_an_l1_line_frees_the_slot() {
    let (clock, mut g) = mk_guard_default();
    for i in 0..100 {
        if i > 0 && i % 10 == 0 {
            clock.advance(VirtualInstant::from_millis((i as u64 / 10) * 1_000));
        }
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(i)),
            kind: QuirkCheckKind::L1Subscribe { symbol: &mk_sym(i) },
        })
        .unwrap();
    }
    clock.advance(VirtualInstant::from_secs(20));
    // Unsubscribe via the QuirkGuard surface.
    g.check(QuirkCheckCtx {
        session: SessionId(1),
        req_id: Some(ReqId(0)),
        kind: QuirkCheckKind::L1Unsubscribe,
    })
    .unwrap();
    // A new subscription can now land.
    g.check(QuirkCheckCtx {
        session: SessionId(1),
        req_id: Some(ReqId(200)),
        kind: QuirkCheckKind::L1Subscribe {
            symbol: &mk_sym(200),
        },
    })
    .unwrap();
}

// ---------------------------------------------------------------------------
// T1 quirk 3 — historical pacing, three regimes.
// ---------------------------------------------------------------------------

#[test]
fn historical_pacing_window_regime_trips_at_61st_distinct_request() {
    let (clock, mut g) = mk_guard_default();
    for i in 0..60 {
        clock.advance(VirtualInstant::from_millis(i * 1_000));
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(i as i32)),
            kind: QuirkCheckKind::HistoricalRequest {
                req: &mk_hist(&format!("SYM{i}"), "TRADES", "1 min"),
            },
        })
        .unwrap();
    }
    clock.advance(VirtualInstant::from_millis(70_000));
    let err = g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(60)),
            kind: QuirkCheckKind::HistoricalRequest {
                req: &mk_hist("SYMX", "TRADES", "1 min"),
            },
        })
        .expect_err("61st request in window must trip 162");
    match err {
        QuirkViolation::HistoricalPacing { code, message, .. } => {
            assert_eq!(code, error_codes::HISTORICAL_PACING);
            assert!(
                message.contains("window"),
                "expected 'window' tag: {message}"
            );
        }
        other => panic!("expected HistoricalPacing, got {other:?}"),
    }
}

#[test]
fn historical_pacing_identical_cooldown_regime_trips_on_repeat() {
    let (clock, mut g) = mk_guard_default();
    let req = mk_hist("AAPL", "TRADES", "1 min");
    g.check(QuirkCheckCtx {
        session: SessionId(1),
        req_id: Some(ReqId(0)),
        kind: QuirkCheckKind::HistoricalRequest { req: &req },
    })
    .unwrap();
    clock.advance(VirtualInstant::from_secs(5)); // 10s shy of the 15s cooldown
    let err = g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(1)),
            kind: QuirkCheckKind::HistoricalRequest { req: &req },
        })
        .expect_err("identical repeat within 15s must trip 162");
    match err {
        QuirkViolation::HistoricalPacing { code, message, .. } => {
            assert_eq!(code, error_codes::HISTORICAL_PACING);
            assert!(
                message.contains("cooldown"),
                "expected 'cooldown' tag: {message}"
            );
        }
        other => panic!("expected HistoricalPacing, got {other:?}"),
    }
}

#[test]
fn historical_pacing_burst_regime_requires_cooldown_disabled() {
    // Burst regime is normally overshadowed by the 15s cooldown for identical
    // requests. To test the 6-in-2s regime in isolation we build a custom
    // guard with cooldown = 0 — that's what the unit test does, so we verify
    // here that the tag still renders through the composite guard's surface.
    let clock = Arc::new(VirtualClock::new());
    let mut cfg = QuirksConfig::default();
    cfg.historical_pacing.identical_cooldown_sec = 0;
    let mut g = CompositeQuirkGuard::from_config(clock.clone() as Arc<dyn Clock>, &cfg);
    let req = mk_hist("AAPL", "TRADES", "1 min");
    for _ in 0..6 {
        clock.advance(VirtualInstant::from_millis(
            clock.now().as_duration().as_millis() as u64 + 1,
        ));
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(0)),
            kind: QuirkCheckKind::HistoricalRequest { req: &req },
        })
        .unwrap();
    }
    clock.advance(VirtualInstant::from_millis(
        clock.now().as_duration().as_millis() as u64 + 1,
    ));
    let err = g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(0)),
            kind: QuirkCheckKind::HistoricalRequest { req: &req },
        })
        .expect_err("7th identical within burst window must trip");
    match err {
        QuirkViolation::HistoricalPacing { code, message, .. } => {
            assert_eq!(code, error_codes::HISTORICAL_PACING);
            assert!(message.contains("burst"), "expected 'burst' tag: {message}");
        }
        other => panic!("expected HistoricalPacing, got {other:?}"),
    }
}

#[test]
fn bidask_historical_costs_double() {
    let (clock, mut g) = mk_guard_default();
    // 30 BID_ASK requests at 1/sec — cost 60 exhausts the 60-unit budget.
    // The 31st must trip even though it's only the 31st request.
    for i in 0..30 {
        clock.advance(VirtualInstant::from_millis(i * 1_000));
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(i as i32)),
            kind: QuirkCheckKind::HistoricalRequest {
                req: &mk_hist(&format!("B{i}"), "BID_ASK", "1 min"),
            },
        })
        .unwrap();
    }
    clock.advance(VirtualInstant::from_secs(35));
    let err = g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(30)),
            kind: QuirkCheckKind::HistoricalRequest {
                req: &mk_hist("BX", "BID_ASK", "1 min"),
            },
        })
        .expect_err("31st BID_ASK request at cost 2x must exhaust the 60-unit budget");
    assert!(matches!(err, QuirkViolation::HistoricalPacing { .. }));
}

// ---------------------------------------------------------------------------
// T1 quirk 5 — farm-status bulletin emission timing + ordering.
// ---------------------------------------------------------------------------

#[test]
fn farm_status_emits_2104_2106_2158_triplet_in_order() {
    let em = FarmStatusEmitter::new();
    let triplet = em.initial_bulletins();
    let codes: Vec<i32> = triplet.iter().map(|b| b.code).collect();
    assert_eq!(
        codes,
        vec![
            error_codes::MD_FARM_OK_USFARM,
            error_codes::HMDS_FARM_OK_USHMDS,
            error_codes::SEC_DEF_FARM_OK,
        ]
    );
    // The plan says emit ~100ms after START_API — verify the default.
    assert_eq!(em.initial_delay(), Duration::from_millis(100));
}

#[test]
fn farm_status_conn_events_produce_canonical_codes() {
    let em = FarmStatusEmitter::new();
    for (evt, expected_code) in [
        (ConnEvent::FarmLost, error_codes::FARM_LOST),
        (
            ConnEvent::FarmRestoredNoData,
            error_codes::FARM_RESTORED_NO_DATA,
        ),
        (ConnEvent::FarmRestoredData, error_codes::FARM_RESTORED_DATA),
        (ConnEvent::DailyRestart, error_codes::TWS_DAILY_RESTART),
    ] {
        let b = em.bulletins_for(evt);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].code, expected_code);
    }
}

// ---------------------------------------------------------------------------
// Quirk 4 — tick-by-tick cap + 15s cooldown.
// ---------------------------------------------------------------------------

#[test]
fn tick_by_tick_cap_is_five_per_session() {
    let (_clock, mut g) = mk_guard_default();
    for i in 0..5 {
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(i)),
            kind: QuirkCheckKind::TickByTickSubscribe { symbol: &mk_sym(i) },
        })
        .unwrap();
    }
    let err = g
        .check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: Some(ReqId(5)),
            kind: QuirkCheckKind::TickByTickSubscribe { symbol: &mk_sym(5) },
        })
        .expect_err("6th TBT subscription must trip");
    assert!(matches!(err, QuirkViolation::TickByTickLimit { .. }));
}

// ---------------------------------------------------------------------------
// Config — YAML round-trip + default = T1 only.
// ---------------------------------------------------------------------------

#[test]
fn config_defaults_disable_all_t2_flags() {
    let c = QuirksConfig::default();
    // T1 defaults.
    assert_eq!(c.msg_rate.limit_per_sec, 50);
    assert_eq!(c.line_limit.max_l1_lines, 100);
    assert_eq!(c.line_limit.max_tbt, 5);
    assert_eq!(c.historical_pacing.window_60_10min, 60);
    assert!(c.historical_pacing.bidask_double_count);

    // T2 defaults off.
    assert!(!c.farm_status.periodic_cycling);
    assert_eq!(c.fills.duplicate_order_status_rate, 0.0);
    assert!(!c.contract_latency_ms.is_enabled());
    assert_eq!(c.market_data_type.default, MarketDataTypeKind::Live);
}

#[test]
fn config_round_trips_through_yaml() {
    let c = QuirksConfig::default();
    let yaml = serde_yaml::to_string(&c).expect("serialize defaults");
    let back = QuirksConfig::from_yaml(&yaml).expect("parse serialized defaults");
    assert_eq!(back, c);
}

#[test]
fn config_custom_limits_apply_to_composite_guard() {
    let clock = Arc::new(VirtualClock::new());
    let mut cfg = QuirksConfig::default();
    cfg.msg_rate.limit_per_sec = 10;
    cfg.line_limit.max_l1_lines = 3;
    let mut g = CompositeQuirkGuard::from_config(clock as Arc<dyn Clock>, &cfg);
    // Msg-rate cap is 10 now, not 50.
    for _ in 0..10 {
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: None,
            kind: QuirkCheckKind::MsgRate,
        })
        .unwrap();
    }
    assert!(matches!(
        g.check(QuirkCheckCtx {
            session: SessionId(1),
            req_id: None,
            kind: QuirkCheckKind::MsgRate,
        })
        .unwrap_err(),
        QuirkViolation::RateLimit { .. }
    ));
}
