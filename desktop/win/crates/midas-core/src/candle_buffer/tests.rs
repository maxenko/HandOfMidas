use super::*;

/// Build a sample CandleBuffer with 5 candles for testing.
fn sample_buffer() -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(5);
    buf.push(1000, 100.0, 105.0, 95.0, 101.0, 1000);
    buf.push(2000, 101.0, 106.0, 96.0, 102.0, 2000);
    buf.push(3000, 102.0, 107.0, 97.0, 103.0, 3000);
    buf.push(4000, 103.0, 108.0, 98.0, 104.0, 4000);
    buf.push(5000, 104.0, 109.0, 99.0, 105.0, 5000);
    buf
}

// ── Construction and basic accessors ───────────────────────────────

#[test]
fn new_buffer_is_empty() {
    let buf = CandleBuffer::new();
    assert!(buf.is_empty());
    assert_eq!(buf.len(), 0);
}

#[test]
fn with_capacity_is_empty() {
    let buf = CandleBuffer::with_capacity(100);
    assert!(buf.is_empty());
    assert_eq!(buf.len(), 0);
}

#[test]
fn default_is_empty() {
    let buf = CandleBuffer::default();
    assert!(buf.is_empty());
    assert_eq!(buf.len(), 0);
}

#[test]
fn push_increases_len() {
    let buf = sample_buffer();
    assert_eq!(buf.len(), 5);
    assert!(!buf.is_empty());
}

#[test]
fn field_access_after_push() {
    let buf = sample_buffer();
    assert_eq!(buf.timestamps[0], 1000);
    assert_eq!(buf.opens[2], 102.0);
    assert_eq!(buf.highs[4], 109.0);
    assert_eq!(buf.lows[0], 95.0);
    assert_eq!(buf.closes[3], 104.0);
    assert_eq!(buf.volumes[1], 2000);
}

// ── price_range ────────────────────────────────────────────────────

#[test]
fn price_range_full() {
    let buf = sample_buffer();
    let (min, max) = buf.price_range(0..5);
    assert_eq!(min, 95.0);
    assert_eq!(max, 109.0);
}

#[test]
fn price_range_subset() {
    let buf = sample_buffer();
    let (min, max) = buf.price_range(1..4);
    // lows[1..4] = [96, 97, 98], min = 96
    // highs[1..4] = [106, 107, 108], max = 108
    assert_eq!(min, 96.0);
    assert_eq!(max, 108.0);
}

#[test]
fn price_range_single_candle() {
    let buf = sample_buffer();
    let (min, max) = buf.price_range(2..3);
    assert_eq!(min, 97.0);
    assert_eq!(max, 107.0);
}

// ── find_index_by_time ─────────────────────────────────────────────

#[test]
fn find_index_exact_match() {
    let buf = sample_buffer();
    assert_eq!(buf.find_index_by_time(3000), 2);
}

#[test]
fn find_index_between_candles() {
    let buf = sample_buffer();
    // 2500 is between 2000 (idx 1) and 3000 (idx 2)
    // partition_point returns 2 (first ts >= 2500)
    assert_eq!(buf.find_index_by_time(2500), 2);
}

#[test]
fn find_index_before_all() {
    let buf = sample_buffer();
    assert_eq!(buf.find_index_by_time(500), 0);
}

#[test]
fn find_index_after_all() {
    let buf = sample_buffer();
    // After all timestamps: clamped to len()-1 = 4
    assert_eq!(buf.find_index_by_time(9999), 4);
}

#[test]
fn find_index_empty_buffer() {
    let buf = CandleBuffer::new();
    assert_eq!(buf.find_index_by_time(1000), 0);
}

// ── find_index_ge / find_index_gt ──────────────────────────────────

#[test]
fn find_index_ge_exact() {
    let buf = sample_buffer();
    assert_eq!(buf.find_index_ge(3000), 2);
}

#[test]
fn find_index_ge_between() {
    let buf = sample_buffer();
    assert_eq!(buf.find_index_ge(2500), 2);
}

#[test]
fn find_index_ge_after_all() {
    let buf = sample_buffer();
    assert_eq!(buf.find_index_ge(9999), 5); // returns len()
}

#[test]
fn find_index_gt_exact() {
    let buf = sample_buffer();
    // First index with timestamp > 3000 is idx 3 (ts=4000)
    assert_eq!(buf.find_index_gt(3000), 3);
}

// ── visible_range ──────────────────────────────────────────────────

#[test]
fn visible_range_subset() {
    let buf = sample_buffer();
    let range = buf.visible_range(2000, 4000);
    // ge(2000) = 1, gt(4000) = 4
    assert_eq!(range, 1..4);
}

#[test]
fn visible_range_all() {
    let buf = sample_buffer();
    let range = buf.visible_range(0, 99999);
    assert_eq!(range, 0..5);
}

// ── update_last ────────────────────────────────────────────────────

#[test]
fn update_last_modifies_values() {
    let mut buf = sample_buffer();
    buf.update_last(5000, 110.0, 115.0, 105.0, 112.0, 9999);
    assert_eq!(buf.timestamps[4], 5000);
    assert_eq!(buf.opens[4], 110.0);
    assert_eq!(buf.highs[4], 115.0);
    assert_eq!(buf.lows[4], 105.0);
    assert_eq!(buf.closes[4], 112.0);
    assert_eq!(buf.volumes[4], 9999);
}

#[test]
fn update_last_on_empty_does_nothing() {
    let mut buf = CandleBuffer::new();
    buf.update_last(1000, 1.0, 2.0, 0.5, 1.5, 100);
    assert!(buf.is_empty());
}

// ── CandleSlice ────────────────────────────────────────────────────

#[test]
fn slice_borrows_correctly() {
    let buf = sample_buffer();
    let sl = buf.slice(1..4);
    assert_eq!(sl.len(), 3);
    assert!(!sl.is_empty());
    assert_eq!(sl.timestamps, &[2000, 3000, 4000]);
    assert_eq!(sl.opens, &[101.0, 102.0, 103.0]);
    assert_eq!(sl.highs, &[106.0, 107.0, 108.0]);
    assert_eq!(sl.lows, &[96.0, 97.0, 98.0]);
    assert_eq!(sl.closes, &[102.0, 103.0, 104.0]);
    assert_eq!(sl.volumes, &[2000, 3000, 4000]);
}

#[test]
fn slice_full_range() {
    let buf = sample_buffer();
    let sl = buf.slice(0..5);
    assert_eq!(sl.len(), 5);
}

#[test]
fn slice_empty_range() {
    let buf = sample_buffer();
    let sl = buf.slice(2..2);
    assert!(sl.is_empty());
    assert_eq!(sl.len(), 0);
}

#[test]
fn slice_price_range() {
    let buf = sample_buffer();
    let sl = buf.slice(1..4);
    let (min, max) = sl.price_range(0..3);
    assert_eq!(min, 96.0);
    assert_eq!(max, 108.0);
}

#[test]
fn slice_find_index_by_time() {
    let buf = sample_buffer();
    let sl = buf.slice(1..4); // timestamps: [2000, 3000, 4000]
    assert_eq!(sl.find_index_by_time(3000), 1); // exact match at local idx 1
    assert_eq!(sl.find_index_by_time(2500), 1); // between 2000 and 3000
    assert_eq!(sl.find_index_by_time(1000), 0); // before all
    assert_eq!(sl.find_index_by_time(9999), 2); // after all, clamped
}

#[test]
fn slice_of_slice() {
    let buf = sample_buffer();
    let sl = buf.slice(0..5);
    let sub = sl.slice(1..3);
    assert_eq!(sub.len(), 2);
    assert_eq!(sub.timestamps, &[2000, 3000]);
}

#[test]
fn slice_is_copy() {
    let buf = sample_buffer();
    let sl = buf.slice(0..3);
    let sl2 = sl; // Copy
    assert_eq!(sl.len(), sl2.len());
}

// ── CandleData trait via dyn dispatch ──────────────────────────────

#[test]
fn candle_data_trait_on_buffer() {
    let buf = sample_buffer();
    let dyn_ref: &dyn CandleData = &buf;
    assert_eq!(dyn_ref.len(), 5);
    assert!(!dyn_ref.is_empty());
    assert_eq!(dyn_ref.timestamp(0), 1000);
    assert_eq!(dyn_ref.open(2), 102.0);
    assert_eq!(dyn_ref.high(4), 109.0);
    assert_eq!(dyn_ref.low(0), 95.0);
    assert_eq!(dyn_ref.close(3), 104.0);
    assert_eq!(dyn_ref.volume(1), 2000);
}

#[test]
fn candle_data_trait_price_range() {
    let buf = sample_buffer();
    let dyn_ref: &dyn CandleData = &buf;
    let (min, max) = dyn_ref.price_range(1..4);
    assert_eq!(min, 96.0);
    assert_eq!(max, 108.0);
}

#[test]
fn candle_data_trait_find_index() {
    let buf = sample_buffer();
    let dyn_ref: &dyn CandleData = &buf;
    assert_eq!(dyn_ref.find_index_by_time(3000), 2);
    assert_eq!(dyn_ref.find_index_by_time(500), 0);
    assert_eq!(dyn_ref.find_index_by_time(9999), 4);
}

#[test]
fn candle_data_trait_on_slice() {
    let buf = sample_buffer();
    let sl = buf.slice(1..4);
    let dyn_ref: &dyn CandleData = &sl;
    assert_eq!(dyn_ref.len(), 3);
    assert_eq!(dyn_ref.timestamp(0), 2000);
    assert_eq!(dyn_ref.open(1), 102.0);
    assert_eq!(dyn_ref.volume(2), 4000);
}

#[test]
fn candle_data_trait_on_empty_buffer() {
    let buf = CandleBuffer::new();
    let dyn_ref: &dyn CandleData = &buf;
    assert!(dyn_ref.is_empty());
    assert_eq!(dyn_ref.len(), 0);
    assert_eq!(dyn_ref.find_index_by_time(1000), 0);
}

/// Verify that a generic function bounded by CandleData works with both
/// CandleBuffer and CandleSlice.
#[test]
fn generic_function_over_candle_data() {
    fn average_close(data: &dyn CandleData) -> f32 {
        if data.is_empty() {
            return 0.0;
        }
        let sum: f32 = (0..data.len()).map(|i| data.close(i)).sum();
        sum / data.len() as f32
    }

    let buf = sample_buffer();
    let avg_buf = average_close(&buf);
    // closes: 101, 102, 103, 104, 105 -> avg = 103.0
    assert!((avg_buf - 103.0).abs() < f32::EPSILON);

    let sl = buf.slice(1..4);
    let avg_sl = average_close(&sl);
    // closes: 102, 103, 104 -> avg = 103.0
    assert!((avg_sl - 103.0).abs() < f32::EPSILON);
}

// ── Single candle edge case ────────────────────────────────────────

#[test]
fn single_candle_buffer() {
    let mut buf = CandleBuffer::new();
    buf.push(1000, 50.0, 55.0, 45.0, 52.0, 500);
    assert_eq!(buf.len(), 1);
    assert!(!buf.is_empty());

    let (min, max) = buf.price_range(0..1);
    assert_eq!(min, 45.0);
    assert_eq!(max, 55.0);

    assert_eq!(buf.find_index_by_time(500), 0);
    assert_eq!(buf.find_index_by_time(1000), 0);
    assert_eq!(buf.find_index_by_time(2000), 0);
}

#[test]
fn single_candle_slice() {
    let mut buf = CandleBuffer::new();
    buf.push(1000, 50.0, 55.0, 45.0, 52.0, 500);
    let sl = buf.slice(0..1);
    assert_eq!(sl.len(), 1);
    assert_eq!(sl.find_index_by_time(999), 0);
    assert_eq!(sl.find_index_by_time(1000), 0);
    assert_eq!(sl.find_index_by_time(1001), 0);
}

// ── Clone ──────────────────────────────────────────────────────────

#[test]
fn buffer_clone() {
    let buf = sample_buffer();
    let clone = buf.clone();
    assert_eq!(clone.len(), buf.len());
    assert_eq!(clone.timestamps, buf.timestamps);
    assert_eq!(clone.closes, buf.closes);
}

// ── Version counter ────────────────────────────────────────────────

#[test]
fn version_starts_at_zero() {
    let buf = CandleBuffer::new();
    assert_eq!(buf.version(), 0);
}

#[test]
fn version_advances_on_push() {
    let mut buf = CandleBuffer::new();
    buf.push(1000, 1.0, 2.0, 0.5, 1.5, 100);
    assert_eq!(buf.version(), 1);
    buf.push(2000, 1.5, 2.5, 1.0, 2.0, 200);
    assert_eq!(buf.version(), 2);
}

#[test]
fn version_advances_on_update_last() {
    let mut buf = CandleBuffer::new();
    buf.push(1000, 1.0, 2.0, 0.5, 1.5, 100);
    buf.push(2000, 1.5, 2.5, 1.0, 2.0, 200);
    assert_eq!(buf.version(), 2);
    buf.update_last(2000, 1.6, 2.6, 1.1, 2.1, 250);
    assert_eq!(buf.version(), 3);
}

#[test]
fn version_no_advance_on_empty_update_last() {
    let mut buf = CandleBuffer::new();
    assert_eq!(buf.version(), 0);
    buf.update_last(1000, 1.0, 2.0, 0.5, 1.5, 100);
    assert_eq!(buf.version(), 0);
}

#[test]
fn clone_preserves_version() {
    let mut buf = CandleBuffer::new();
    buf.push(1000, 1.0, 2.0, 0.5, 1.5, 100);
    buf.push(2000, 1.5, 2.5, 1.0, 2.0, 200);
    buf.push(3000, 2.0, 3.0, 1.5, 2.5, 300);
    assert_eq!(buf.version(), 3);
    let clone = buf.clone();
    assert_eq!(clone.version(), 3);
}

#[test]
fn clone_independent_version() {
    let mut buf = CandleBuffer::new();
    buf.push(1000, 1.0, 2.0, 0.5, 1.5, 100);
    buf.push(2000, 1.5, 2.5, 1.0, 2.0, 200);
    assert_eq!(buf.version(), 2);

    let mut clone = buf.clone();
    assert_eq!(clone.version(), 2);

    clone.push(3000, 2.0, 3.0, 1.5, 2.5, 300);
    assert_eq!(clone.version(), 3);
    // Original counter must not be affected.
    assert_eq!(buf.version(), 2);
}

// ─── merge_bar — sub-bucket accumulation (D1/W1 live-extend fallback) ──

#[test]
fn merge_bar_push_new_bucket_on_empty() {
    let mut buf = CandleBuffer::new();
    buf.merge_bar(86_400_000, 100.0, 100.0, 100.0, 100.0, 10);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf.opens[0], 100.0);
    assert_eq!(buf.closes[0], 100.0);
    assert_eq!(buf.volumes[0], 10);
}

#[test]
fn merge_bar_accumulates_same_bucket() {
    let mut buf = CandleBuffer::new();
    // First sub-bar opens the D1 bucket (midnight UTC of day 2).
    buf.merge_bar(86_400_000, 100.0, 100.0, 100.0, 100.0, 10);
    // Second sub-bar — higher high, same bucket.
    buf.merge_bar(86_400_000, 102.0, 105.0, 101.0, 103.0, 20);
    // Third sub-bar — lower low, new close.
    buf.merge_bar(86_400_000, 103.0, 104.0, 99.0, 100.5, 15);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf.opens[0], 100.0); // open stays at first sub-bar
    assert_eq!(buf.highs[0], 105.0); // max over all sub-bars
    assert_eq!(buf.lows[0], 99.0); // min over all sub-bars
    assert_eq!(buf.closes[0], 100.5); // latest close wins
    assert_eq!(buf.volumes[0], 45); // 10 + 20 + 15
}

#[test]
fn merge_bar_pushes_new_candle_on_new_bucket() {
    let mut buf = CandleBuffer::new();
    buf.merge_bar(86_400_000, 100.0, 101.0, 99.0, 100.5, 10);
    buf.merge_bar(172_800_000, 102.0, 103.0, 101.0, 102.5, 20);
    assert_eq!(buf.len(), 2);
    assert_eq!(buf.timestamps, &[86_400_000, 172_800_000]);
}

#[test]
fn merge_bar_drops_out_of_order() {
    let mut buf = CandleBuffer::new();
    buf.merge_bar(172_800_000, 100.0, 101.0, 99.0, 100.0, 10);
    // Second bar claims an EARLIER bucket — stale, should drop.
    buf.merge_bar(86_400_000, 90.0, 91.0, 89.0, 90.0, 5);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf.timestamps[0], 172_800_000);
}

#[test]
fn merge_bar_volume_saturates() {
    let mut buf = CandleBuffer::new();
    buf.merge_bar(86_400_000, 1.0, 1.0, 1.0, 1.0, u32::MAX - 5);
    buf.merge_bar(86_400_000, 1.0, 1.0, 1.0, 1.0, 100);
    assert_eq!(buf.volumes[0], u32::MAX);
}

#[test]
fn merge_bar_bumps_version() {
    let mut buf = CandleBuffer::new();
    assert_eq!(buf.version(), 0);
    buf.merge_bar(86_400_000, 1.0, 1.0, 1.0, 1.0, 10);
    assert_eq!(buf.version(), 1);
    buf.merge_bar(86_400_000, 2.0, 2.0, 1.0, 1.5, 5);
    assert_eq!(buf.version(), 2);
}
