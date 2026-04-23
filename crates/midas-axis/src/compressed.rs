//! [`CompressedAxis`] — session-compressed time axis.
//!
//! The axis maps the UNION of session active-ranges uniformly to the
//! viewport, with an optional pixel gap between consecutive sessions.
//! Closed time between sessions collapses to zero (or `gap_px`) regardless
//! of wall-clock duration.
//!
//! # Projection math
//!
//! Given N sessions with durations `d_i` (wall-clock seconds each), a
//! viewport `width_px`, and `gap_px` between each pair:
//!
//! - `total_gap_px = gap_px * (N - 1)` (N=0 → 0).
//! - `session_pixels = width_px - total_gap_px`.
//! - `px_per_sec = session_pixels / Σ d_i`.
//! - Session `i` occupies
//!   `[offset_i, offset_i + d_i * px_per_sec)` in x,
//!   where `offset_i = Σ_{k<i}(d_k * px_per_sec + gap_px)`.
//!
//! Precomputed `session_x_starts` and `session_x_ends` keep `to_x` and
//! `from_x` at binary-search cost. Session-close pixel equals
//! next-session-open pixel only when `gap_px == 0.0`; otherwise a gap
//! exists between them.

use smallvec::SmallVec;

use midas_calendar::{Session, SessionBuf, SessionKind, TimeAxisPolicy, Timestamp};

use crate::ticks::{enumerate_ticks, pick_step};
use crate::{AxisError, Importance, SnapDirection, TickDensity, TickLabel, TimeAxis, TimeTick};

/// Session-compressed time axis.
///
/// `gap_px` may be `0.0` (the default chosen by the `for_calendar`
/// builder) or positive; negative / non-finite gaps are rejected.
#[derive(Clone, Debug)]
pub struct CompressedAxis {
    sessions: SessionBuf,
    /// Parallel to `sessions`. Left edge in pixels of each session.
    session_x_starts: SmallVec<[f32; 16]>,
    /// Parallel to `sessions`. Right edge (exclusive) in pixels.
    session_x_ends: SmallVec<[f32; 16]>,
    width: f32,
    gap_px: f32,
}

impl CompressedAxis {
    /// Build a compressed axis. `sessions` must be sorted by `open` and
    /// non-overlapping (`s[i].close() <= s[i+1].open()`). `width_px`
    /// finite-positive; `gap_px` finite-nonnegative.
    ///
    /// Sessions with zero duration are tolerated (both edges collapse to
    /// the same x). Empty iterator produces a degenerate axis that maps
    /// everything to `0.0`.
    pub fn new(
        sessions: impl IntoIterator<Item = Session>,
        width_px: f32,
        gap_px: f32,
    ) -> Result<Self, AxisError> {
        if !width_px.is_finite() || width_px <= 0.0 {
            return Err(AxisError::InvalidWidth(width_px));
        }
        if !gap_px.is_finite() || gap_px < 0.0 {
            return Err(AxisError::InvalidWidth(gap_px));
        }
        let sessions: SessionBuf = sessions.into_iter().collect();

        // Validate sorted + non-overlapping.
        for w in sessions.windows(2) {
            if w[0].close() > w[1].open() || w[0].open() > w[1].open() {
                return Err(AxisError::UnsortedSessions);
            }
        }

        let n = sessions.len();
        let mut session_x_starts: SmallVec<[f32; 16]> = SmallVec::with_capacity(n);
        let mut session_x_ends: SmallVec<[f32; 16]> = SmallVec::with_capacity(n);

        if n == 0 {
            return Ok(Self {
                sessions,
                session_x_starts,
                session_x_ends,
                width: width_px,
                gap_px,
            });
        }

        // Total gap pixels sits between sessions; (n-1) gaps.
        let total_gap = gap_px * (n as f32 - 1.0);
        let session_pixels = (width_px - total_gap).max(0.0);

        // Sum of session durations (seconds as f64 for precision).
        let mut total_secs: f64 = 0.0;
        let mut durations: SmallVec<[f64; 16]> = SmallVec::with_capacity(n);
        for s in &sessions {
            let d = (s.close() - s.open()).num_seconds().max(0) as f64;
            durations.push(d);
            total_secs += d;
        }

        let px_per_sec = if total_secs > 0.0 {
            session_pixels as f64 / total_secs
        } else {
            0.0
        };

        let mut cursor: f64 = 0.0;
        for (i, d) in durations.iter().enumerate() {
            let start_x = cursor as f32;
            let end_x = (cursor + d * px_per_sec) as f32;
            session_x_starts.push(start_x);
            session_x_ends.push(end_x);
            cursor += d * px_per_sec;
            if i + 1 < n {
                cursor += gap_px as f64;
            }
        }
        // Snap the last session's right edge to `width_px` to absorb any
        // accumulated rounding from f64 → f32 conversion. Guarantees
        // `to_x(last.close()) == width_px` exactly.
        if let Some(last) = session_x_ends.last_mut() {
            *last = width_px;
        }

        Ok(Self {
            sessions,
            session_x_starts,
            session_x_ends,
            width: width_px,
            gap_px,
        })
    }

    /// Borrow the sessions slice. Callers can inspect session kind /
    /// label for per-axis rendering without re-querying the calendar.
    #[inline]
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    #[inline]
    pub fn gap_px(&self) -> f32 {
        self.gap_px
    }

    #[cfg(test)]
    pub(crate) fn sessions_x_starts_for_tests(&self, idx: usize) -> f32 {
        self.session_x_starts[idx]
    }

    #[cfg(test)]
    pub(crate) fn sessions_x_ends_for_tests(&self, idx: usize) -> f32 {
        self.session_x_ends[idx]
    }

    /// Binary-search for the session containing `ts`. Returns:
    ///
    /// - `Ok(idx)` if `ts` sits inside `sessions[idx]`. Inclusive on
    ///   the close edge: `ts == sessions[idx].close()` belongs to
    ///   `idx`, not to the following gap or session — even when
    ///   adjacent sessions touch (`sessions[idx].close() ==
    ///   sessions[idx+1].open()`).
    /// - `Err(idx)` if `ts` falls in the gap BEFORE `sessions[idx]`, or
    ///   `Err(sessions.len())` if `ts` is past the last close, or
    ///   `Err(0)` if `ts` is before the first open.
    ///
    /// Bug-hunt H4: the previous partition `open() <= ts` returned
    /// session `i+1` for `ts == sessions[i].close() == sessions[i+1].open()`
    /// (touching sessions), causing `to_x(session.close())` to jump
    /// across the visual gap into the next session's x-range. The fix
    /// checks the preceding session's close FIRST — if `ts <=
    /// sessions[lo-1].close()`, the close-edge timestamp owns session
    /// `lo-1`. Otherwise `ts` truly belongs to `sessions[lo]` (or the
    /// gap before it). Non-touching sessions are unchanged.
    fn find_session_for_ts(&self, ts: Timestamp) -> Result<usize, usize> {
        let n = self.sessions.len();
        if n == 0 {
            return Err(0);
        }
        if ts < self.sessions[0].open() {
            return Err(0);
        }
        // Standard `open() <= ts` partition — `lo` is the first
        // session whose open is STRICTLY greater than `ts`, so the
        // candidate session is `lo - 1`.
        let mut lo = 0_usize;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.sessions[mid].open() <= ts {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        // `lo` is now the upper bound. The candidate session is
        // `lo - 1` (`lo == 0` is already handled by the early return
        // above, since `ts >= sessions[0].open()` guarantees the loop
        // sets `lo >= 1`).
        let idx = lo - 1;
        // Touching-boundary fix: if `ts == sessions[idx].open()` AND
        // there's a preceding session whose `close` equals `ts`, the
        // close-edge ts belongs to THAT preceding session. Without
        // this special case, `to_x(sessions[i].close())` would land
        // on `sessions[i+1]`'s x_start rather than staying at
        // `sessions[i]`'s x_end — which jumps across the visual gap
        // when `gap_px > 0`.
        if idx > 0 && self.sessions[idx].open() == ts && self.sessions[idx - 1].close() == ts {
            return Ok(idx - 1);
        }
        let s = &self.sessions[idx];
        // Inclusive close edge on the candidate session. `ts ==
        // s.close()` still belongs to session `idx`.
        if ts <= s.close() {
            Ok(idx)
        } else if idx + 1 < n {
            Err(idx + 1)
        } else {
            Err(n)
        }
    }

    /// Binary-search for the session whose `[x_start, x_end)` contains
    /// `x`. Returns:
    ///
    /// - `Ok(idx)` if `x` sits inside `sessions[idx]` in x-space.
    /// - `Err(idx)` if `x` falls in the gap BEFORE `sessions[idx]`, or
    ///   `Err(sessions.len())` if `x > last.x_end`, or
    ///   `Err(0)` if `x < first.x_start`.
    fn find_session_for_x(&self, x: f32) -> Result<usize, usize> {
        let n = self.sessions.len();
        if n == 0 {
            return Err(0);
        }
        if x < self.session_x_starts[0] {
            return Err(0);
        }
        // Use standard partition_point against x_starts.
        let idx = match self.session_x_starts.partition_point(|&sx| sx <= x) {
            0 => 0,
            i => i - 1,
        };
        let end = self.session_x_ends[idx];
        // Pixel x is INSIDE [x_start, x_end]. Inclusive on the right
        // edge so `to_x(session.close())` round-trips back into this
        // session rather than falling into the gap.
        if x <= end {
            Ok(idx)
        } else if idx + 1 < n {
            Err(idx + 1)
        } else {
            Err(n)
        }
    }

    /// Project `ts` into session `idx`, using that session's own
    /// x-start/x-end and wall-clock duration.
    fn project_in_session(&self, idx: usize, ts: Timestamp) -> f32 {
        let s = &self.sessions[idx];
        let open_ns = s.open().timestamp_nanos_opt().unwrap_or(0);
        let close_ns = s.close().timestamp_nanos_opt().unwrap_or(open_ns + 1);
        let dur_ns = (close_ns - open_ns).max(1) as f64;
        let ts_ns = ts.timestamp_nanos_opt().unwrap_or(open_ns);
        let frac = ((ts_ns - open_ns) as f64 / dur_ns).clamp(0.0, 1.0);
        let x_start = self.session_x_starts[idx];
        let x_end = self.session_x_ends[idx];
        x_start + (x_end - x_start) * frac as f32
    }

    /// Reconstruct the timestamp for a pixel known to lie in session
    /// `idx`.
    fn unproject_in_session(&self, idx: usize, x: f32) -> Timestamp {
        let s = &self.sessions[idx];
        let x_start = self.session_x_starts[idx];
        let x_end = self.session_x_ends[idx];
        let width = (x_end - x_start).max(f32::EPSILON);
        let frac = ((x - x_start) / width).clamp(0.0, 1.0) as f64;
        let open_ns = s.open().timestamp_nanos_opt().unwrap_or(0);
        let close_ns = s.close().timestamp_nanos_opt().unwrap_or(open_ns + 1);
        let dur_ns = (close_ns - open_ns).max(1) as f64;
        let offset_ns = (frac * dur_ns).round() as i64;
        s.open() + chrono::Duration::nanoseconds(offset_ns)
    }
}

impl TimeAxis for CompressedAxis {
    fn to_x(&self, ts: Timestamp) -> f32 {
        if self.sessions.is_empty() {
            return 0.0;
        }
        match self.find_session_for_ts(ts) {
            Ok(idx) => self.project_in_session(idx, ts),
            Err(0) => 0.0,
            Err(n) if n == self.sessions.len() => self.width,
            Err(next_idx) => {
                // Mid-gap: clamp to preceding session's close pixel.
                self.session_x_ends[next_idx - 1]
            }
        }
    }

    fn from_x(&self, x: f32) -> Option<Timestamp> {
        if !x.is_finite() || !(0.0..=self.width).contains(&x) {
            return None;
        }
        if self.sessions.is_empty() {
            return None;
        }
        match self.find_session_for_x(x) {
            Ok(idx) => Some(self.unproject_in_session(idx, x)),
            Err(_) => None,
        }
    }

    fn from_x_snapped(&self, x: f32, dir: SnapDirection) -> (Timestamp, bool) {
        if self.sessions.is_empty() {
            // Degenerate — no snap target. Return UNIX epoch-ish; this
            // path is only reachable with an empty viewport.
            return (chrono::DateTime::<chrono::Utc>::UNIX_EPOCH, true);
        }
        let clamped = if x.is_finite() {
            x.clamp(0.0, self.width)
        } else {
            0.0
        };

        match self.find_session_for_x(clamped) {
            Ok(idx) => {
                let was_snapped = !x.is_finite() || !(0.0..=self.width).contains(&x);
                (self.unproject_in_session(idx, clamped), was_snapped)
            }
            Err(next_idx) => {
                // clamped is in a gap / off the end.
                let n = self.sessions.len();
                // prev_idx: most recent session ending at or before clamped.
                //   If Err(0), there is no preceding session; force Forward.
                //   If Err(n), there is no next session; force Backward.
                let prev_idx = if next_idx == 0 {
                    None
                } else {
                    Some(next_idx - 1)
                };
                let next_idx_opt = if next_idx >= n { None } else { Some(next_idx) };

                match (dir, prev_idx, next_idx_opt) {
                    (SnapDirection::Forward, _, Some(ni)) => (self.sessions[ni].open(), true),
                    (SnapDirection::Forward, Some(pi), None) => (self.sessions[pi].close(), true),
                    (SnapDirection::Backward, Some(pi), _) => (self.sessions[pi].close(), true),
                    (SnapDirection::Backward, None, Some(ni)) => (self.sessions[ni].open(), true),
                    (SnapDirection::Nearest, Some(pi), Some(ni)) => {
                        let prev_x = self.session_x_ends[pi];
                        let next_x = self.session_x_starts[ni];
                        if (clamped - prev_x).abs() <= (next_x - clamped).abs() {
                            (self.sessions[pi].close(), true)
                        } else {
                            (self.sessions[ni].open(), true)
                        }
                    }
                    (SnapDirection::Nearest, Some(pi), None) => (self.sessions[pi].close(), true),
                    (SnapDirection::Nearest, None, Some(ni)) => (self.sessions[ni].open(), true),
                    // Only reachable on an empty session list, handled above.
                    (_, None, None) => (chrono::DateTime::<chrono::Utc>::UNIX_EPOCH, true),
                }
            }
        }
    }

    fn ticks(&self, density: TickDensity) -> Vec<TimeTick> {
        let mut out = Vec::new();
        if self.sessions.is_empty() {
            return out;
        }

        // Per-session tick generation plus session-boundary ticks.
        for (i, s) in self.sessions.iter().enumerate() {
            // Skip decorating zero-length sessions.
            if self.session_x_ends[i] <= self.session_x_starts[i] {
                continue;
            }
            // Visible width of this session decides density.
            let session_width = self.session_x_ends[i] - self.session_x_starts[i];
            let span_secs = (s.close() - s.open()).num_seconds().max(1);
            let step = pick_step(span_secs, session_width.max(1.0), density);
            let mut in_session = enumerate_ticks(s.open(), s.close(), step, |ts| {
                // `ts` may equal the session close — that's fine; project
                // will emit `x_end` which is still inside the session by
                // our half-open convention (close maps to next-session-start
                // when gap=0, otherwise stays at this session's right edge).
                if ts < s.open() || ts > s.close() {
                    None
                } else {
                    Some(self.project_in_session(i, ts))
                }
            });
            out.append(&mut in_session);

            // Session-boundary major tick between session i and i+1.
            if i + 1 < self.sessions.len() {
                let boundary_x = self.session_x_ends[i];
                out.push(TimeTick {
                    x: boundary_x,
                    ts: s.close(),
                    label: TickLabel::Primary(std::borrow::Cow::Borrowed("|")),
                    importance: Importance::Major,
                });
            }
        }

        // Sort by x so downstream rendering has a monotonic sequence.
        out.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    fn width_px(&self) -> f32 {
        self.width
    }

    fn policy(&self) -> TimeAxisPolicy {
        TimeAxisPolicy::CompressedSessionBoundaries
    }
}

// A small test helper to construct a Session from outside the
// midas-calendar crate — the real `Session::new` is crate-private. We
// reach in via the `xnys` singleton's `classify` for tests that need
// exchange-real sessions, and via `sessions_between` for multi-session
// fixtures. Unit tests use the latter.
#[allow(dead_code)]
fn _unused_dummy(kind: SessionKind) -> SessionKind {
    kind
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_calendar::xnys;

    use super::*;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    /// Pull XNYS regular sessions for two consecutive weekdays. Returns a
    /// SmallVec of exactly two Regular sessions (filters out PreMarket /
    /// PostMarket which the calendar also reports).
    fn two_regular_sessions() -> SessionBuf {
        // 2024-01-17 (Wed) and 2024-01-18 (Thu). Both regular trading
        // days, 09:30-16:00 ET (= 14:30-21:00 UTC).
        let cal = xnys();
        let from = ts(2024, 1, 17, 0, 0, 0);
        let to = ts(2024, 1, 19, 0, 0, 0);
        let mut buf: SessionBuf = SessionBuf::new();
        cal.sessions_between(from, to, &mut buf);
        buf.retain(|s| matches!(s.kind(), SessionKind::Regular));
        assert_eq!(buf.len(), 2, "expected exactly 2 regular sessions");
        buf
    }

    #[test]
    fn two_sessions_map_to_halves_when_no_gap() {
        let sessions = two_regular_sessions();
        let s0_open = sessions[0].open();
        let s0_close = sessions[0].close();
        let s1_open = sessions[1].open();
        let s1_close = sessions[1].close();

        let axis = CompressedAxis::new(sessions.clone(), 1000.0, 0.0).unwrap();

        assert!((axis.to_x(s0_open) - 0.0).abs() < 1e-3, "s0_open");
        // Two identical-duration sessions → s0_close sits at 500.
        let x_close0 = axis.to_x(s0_close);
        assert!((x_close0 - 500.0).abs() < 0.5, "s0_close: {x_close0}");
        // With gap = 0, s1_open also lands at 500.
        let x_open1 = axis.to_x(s1_open);
        assert!((x_open1 - 500.0).abs() < 0.5, "s1_open: {x_open1}");
        let x_close1 = axis.to_x(s1_close);
        assert!((x_close1 - 1000.0).abs() < 0.5, "s1_close: {x_close1}");
    }

    #[test]
    fn from_x_returns_none_in_the_gap() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions, 1000.0, 50.0).unwrap();
        // With gap=50, session pixels = 950 → each session width = 475.
        // s0 occupies [0, 475]; gap [475, 525]; s1 [525, 1000].
        assert!(axis.from_x(475.0).is_some());
        assert!(axis.from_x(500.0).is_none());
        assert!(axis.from_x(525.0).is_some());
    }

    #[test]
    fn gap_px_creates_visible_gap() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions.clone(), 1000.0, 4.0).unwrap();
        // After session 0 ends, the next session starts 4px later.
        let x_close0 = axis.to_x(sessions[0].close());
        let x_open1 = axis.to_x(sessions[1].open());
        assert!(
            (x_open1 - x_close0 - 4.0).abs() < 0.5,
            "gap: x_close0={x_close0}, x_open1={x_open1}"
        );
    }

    #[test]
    fn snap_nearest_picks_closer_boundary() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions.clone(), 1000.0, 100.0).unwrap();
        // With gap=100, session pixels = 900 → each width = 450.
        // s0 ends at 450; s1 starts at 550.
        // Mid-gap at 490 is closer to 450 than to 550 → snap to
        // s0.close.
        let (ts_snapped, was) = axis.from_x_snapped(490.0, SnapDirection::Nearest);
        assert!(was);
        assert_eq!(ts_snapped, sessions[0].close());
        // 540 is closer to 550 → snap to s1.open.
        let (ts_snapped, was) = axis.from_x_snapped(540.0, SnapDirection::Nearest);
        assert!(was);
        assert_eq!(ts_snapped, sessions[1].open());
    }

    #[test]
    fn snap_forward_picks_next_session_open() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions.clone(), 1000.0, 100.0).unwrap();
        // Any x in the gap → s1.open.
        let (ts_snapped, _) = axis.from_x_snapped(500.0, SnapDirection::Forward);
        assert_eq!(ts_snapped, sessions[1].open());
    }

    #[test]
    fn snap_backward_picks_prev_session_close() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions.clone(), 1000.0, 100.0).unwrap();
        let (ts_snapped, _) = axis.from_x_snapped(500.0, SnapDirection::Backward);
        assert_eq!(ts_snapped, sessions[0].close());
    }

    #[test]
    fn snap_inside_session_is_not_snapped() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions, 1000.0, 100.0).unwrap();
        let (_, was) = axis.from_x_snapped(200.0, SnapDirection::Nearest);
        assert!(!was);
    }

    #[test]
    fn ticks_include_session_boundary_major() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions.clone(), 1200.0, 0.0).unwrap();
        let ticks = axis.ticks(TickDensity::Normal);
        // The session-boundary tick uses `|` as primary label and is
        // major.
        let boundary_tick = ticks
            .iter()
            .find(|t| t.label.primary() == "|" && t.importance == Importance::Major);
        assert!(boundary_tick.is_some(), "missing session-boundary tick");
    }

    #[test]
    fn policy_is_compressed() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions, 1000.0, 0.0).unwrap();
        assert_eq!(axis.policy(), TimeAxisPolicy::CompressedSessionBoundaries);
    }

    #[test]
    fn rejects_unsorted_sessions() {
        // Reverse the two regular sessions.
        let mut sessions = two_regular_sessions();
        sessions.reverse();
        let err = CompressedAxis::new(sessions, 1000.0, 0.0);
        assert!(matches!(err, Err(AxisError::UnsortedSessions)));
    }

    #[test]
    fn rejects_bad_width_or_gap() {
        let sessions = two_regular_sessions();
        assert!(matches!(
            CompressedAxis::new(sessions.clone(), -1.0, 0.0),
            Err(AxisError::InvalidWidth(_))
        ));
        assert!(matches!(
            CompressedAxis::new(sessions, 1000.0, -1.0),
            Err(AxisError::InvalidWidth(_))
        ));
    }

    #[test]
    fn to_x_before_first_open_clamps_to_zero() {
        let sessions = two_regular_sessions();
        let s0_open = sessions[0].open();
        let axis = CompressedAxis::new(sessions, 1000.0, 0.0).unwrap();
        assert_eq!(axis.to_x(s0_open - chrono::Duration::hours(10)), 0.0);
    }

    #[test]
    fn to_x_after_last_close_clamps_to_width() {
        let sessions = two_regular_sessions();
        let s_last_close = sessions.last().unwrap().close();
        let axis = CompressedAxis::new(sessions, 1000.0, 0.0).unwrap();
        assert_eq!(
            axis.to_x(s_last_close + chrono::Duration::hours(10)),
            1000.0
        );
    }

    #[test]
    fn from_x_outside_viewport_is_none() {
        let sessions = two_regular_sessions();
        let axis = CompressedAxis::new(sessions, 1000.0, 0.0).unwrap();
        assert!(axis.from_x(-1.0).is_none());
        assert!(axis.from_x(1001.0).is_none());
        assert!(axis.from_x(f32::NAN).is_none());
    }

    /// Three consecutive XNYS sessions for 2024-01-17 (pre / regular /
    /// post) — these TOUCH exactly (pre.close == regular.open ==
    /// 14:30 UTC; regular.close == post.open == 21:00 UTC). Bug-hunt
    /// H4: `to_x(session_i.close())` must land inside session i's
    /// x-range, not jump across the visual boundary into session i+1.
    ///
    /// Query window is 09:00 UTC → 01:00 UTC next day to cover the
    /// full ET trading day (04:00 ET → 20:00 ET). Queries aligned to
    /// UTC midnight would pick up the previous day's post-market,
    /// which is not what this test needs.
    fn three_touching_xnys_sessions_one_day() -> SessionBuf {
        let cal = xnys();
        let from = ts(2024, 1, 17, 9, 0, 0); // 04:00 ET
        let to = ts(2024, 1, 18, 1, 0, 0); // 20:00 ET
        let mut buf: SessionBuf = SessionBuf::new();
        cal.sessions_between(from, to, &mut buf);
        // Keep only the three sessions for 2024-01-17.
        let mut out: SessionBuf = SessionBuf::new();
        for s in buf {
            if !matches!(
                s.kind(),
                SessionKind::PreMarket | SessionKind::Regular | SessionKind::PostMarket
            ) {
                continue;
            }
            out.push(s);
            if out.len() == 3 {
                break;
            }
        }
        assert_eq!(
            out.len(),
            3,
            "expected 3 touching XNYS sessions on 2024-01-17"
        );
        out
    }

    #[test]
    fn close_edge_timestamp_projects_to_own_session_end() {
        // Regression: bug-hunt H4. For touching sessions (pre.close ==
        // regular.open), `to_x(pre.close())` used to return regular's
        // x_start pixel (jumping across the visual gap). After the
        // fix, `to_x(pre.close())` stays at pre's x_end. With gap=0,
        // pre.x_end numerically equals regular.x_start but the fix
        // ensures the SESSION owning the close-edge ts is the same
        // one whose x_end we project into — which matters the moment
        // `gap_px > 0`.
        let sessions = three_touching_xnys_sessions_one_day();
        let pre_close = sessions[0].close();
        let reg_open = sessions[1].open();
        assert_eq!(pre_close, reg_open, "sessions must be strictly touching");

        // gap=0 case: the pixel is numerically the same either way,
        // but the session ownership now differs.
        let axis = CompressedAxis::new(sessions.clone(), 1000.0, 0.0).unwrap();
        let x_at_close = axis.to_x(pre_close);
        assert!(
            (x_at_close - axis.sessions_x_ends_for_tests(0)).abs() < 1e-3,
            "pre.close should project into pre's x_end, not regular's x_start; \
             got {x_at_close}, pre_x_end={}, reg_x_start={}",
            axis.sessions_x_ends_for_tests(0),
            axis.sessions_x_starts_for_tests(1),
        );

        // gap>0 case: pre.x_end and regular.x_start are visually
        // separated. The close-edge ts must land at pre.x_end, not
        // regular.x_start.
        let axis = CompressedAxis::new(sessions, 1000.0, 20.0).unwrap();
        let x_at_close = axis.to_x(pre_close);
        let pre_x_end = axis.sessions_x_ends_for_tests(0);
        let reg_x_start = axis.sessions_x_starts_for_tests(1);
        assert!(
            (reg_x_start - pre_x_end - 20.0).abs() < 0.5,
            "sanity: gap=20 applied"
        );
        assert!(
            (x_at_close - pre_x_end).abs() < 1e-3,
            "close-edge ts owns preceding session: got x={x_at_close}, pre_x_end={pre_x_end}, reg_x_start={reg_x_start}"
        );
        assert!(
            (x_at_close - reg_x_start).abs() > 1.0,
            "close-edge ts must NOT project into next session's x_start"
        );
    }

    #[test]
    fn to_x_from_x_round_trip_within_session() {
        let sessions = two_regular_sessions();
        let s0 = sessions[0].clone();
        let axis = CompressedAxis::new(sessions.clone(), 1000.0, 0.0).unwrap();
        // midpoint of session 0.
        let mid_ts = s0.open() + (s0.close() - s0.open()) / 2;
        let x = axis.to_x(mid_ts);
        let back = axis.from_x(x).expect("in-session");
        // Round-trip should preserve within a nanosecond per half-bar.
        assert!((back - mid_ts).num_milliseconds().abs() <= 1);
    }
}
