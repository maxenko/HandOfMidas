//! Typed field codec over the NUL-delimited payload fields of a
//! [`RawFrame`](crate::protocol::framing::RawFrame).
//!
//! Every TWS message, once length-prefix is stripped and payload split on
//! NUL, is a sequence of ASCII fields encoding primitives:
//!
//! * `i32` — signed decimal, sentinel `UNSET_INT = i32::MAX`
//! * `i64` — signed decimal, sentinel `UNSET_LONG = i64::MAX`
//! * `f64` — decimal or scientific, sentinel `UNSET_DOUBLE = f64::MAX`
//! * `bool` — `"1"` / `"0"` (with `""` tolerated as `false`)
//! * `String` — raw field bytes
//!
//! Optional fields decode empty / sentinel as [`None`]. Write side mirrors
//! the reader: `None` becomes the relevant sentinel.

use bytes::Bytes;

use crate::protocol::ProtocolError;

// ---------------------------------------------------------------------------
// Sentinels — values IB uses to represent "unset" on the wire.
// ---------------------------------------------------------------------------

/// IB's sentinel for an unset `i32`. See §Critical details in the Stage plan.
pub const UNSET_INT: i32 = i32::MAX;

/// IB's sentinel for an unset `i64`.
pub const UNSET_LONG: i64 = i64::MAX;

/// IB's sentinel for an unset `f64`. Serialised as the exact literal
/// `1.7976931348623157E308`.
pub const UNSET_DOUBLE: f64 = f64::MAX;

/// Canonical string representation of [`UNSET_DOUBLE`]. IB's clients compare
/// against this exact textual form.
pub const UNSET_DOUBLE_STR: &str = "1.7976931348623157E308";

// ---------------------------------------------------------------------------
// FieldReader — cursor over `&[Bytes]`.
// ---------------------------------------------------------------------------

/// Read-cursor over a slice of NUL-delimited fields. Holds a borrow of the
/// underlying bytes; all reads advance a single usize cursor.
#[derive(Debug)]
pub struct FieldReader<'a> {
    fields: &'a [Bytes],
    idx: usize,
}

impl<'a> FieldReader<'a> {
    pub fn new(fields: &'a [Bytes]) -> Self {
        Self { fields, idx: 0 }
    }

    /// Number of fields already consumed.
    pub fn pos(&self) -> usize {
        self.idx
    }

    /// Number of fields remaining.
    pub fn remaining(&self) -> usize {
        self.fields.len().saturating_sub(self.idx)
    }

    /// Returns true when the cursor is at the end of the field list.
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Borrow the next raw field without decoding it.
    fn take_raw(&mut self) -> Result<&'a [u8], ProtocolError> {
        let f = self.fields.get(self.idx).ok_or_else(|| {
            ProtocolError::Field(format!(
                "unexpected end of fields at index {} (len={})",
                self.idx,
                self.fields.len()
            ))
        })?;
        self.idx += 1;
        Ok(f.as_ref())
    }

    /// Decode the next field as UTF-8 `&str` (IB uses ASCII; we accept any
    /// valid UTF-8).
    fn take_str(&mut self) -> Result<&'a str, ProtocolError> {
        let raw = self.take_raw()?;
        std::str::from_utf8(raw)
            .map_err(|e| ProtocolError::Field(format!("invalid utf-8 in field: {e}")))
    }

    /// Read one ASCII-encoded `i32`.
    pub fn read_i32(&mut self) -> Result<i32, ProtocolError> {
        let s = self.take_str()?;
        if s.is_empty() {
            return Err(ProtocolError::Field(
                "expected i32, got empty field".to_string(),
            ));
        }
        s.parse::<i32>().map_err(ProtocolError::from)
    }

    /// Read one ASCII-encoded `i64`.
    pub fn read_i64(&mut self) -> Result<i64, ProtocolError> {
        let s = self.take_str()?;
        if s.is_empty() {
            return Err(ProtocolError::Field(
                "expected i64, got empty field".to_string(),
            ));
        }
        s.parse::<i64>().map_err(ProtocolError::from)
    }

    /// Read one ASCII-encoded `f64`. Accepts both `1.23` and `1.23E45`.
    pub fn read_f64(&mut self) -> Result<f64, ProtocolError> {
        let s = self.take_str()?;
        if s.is_empty() {
            return Err(ProtocolError::Field(
                "expected f64, got empty field".to_string(),
            ));
        }
        s.parse::<f64>().map_err(ProtocolError::from)
    }

    /// Read a `String` — always succeeds (empty yields `""`).
    pub fn read_string(&mut self) -> Result<String, ProtocolError> {
        Ok(self.take_str()?.to_owned())
    }

    /// Read a boolean — `"1"` → true, anything else → false. IB's convention
    /// is permissive; empty or `"0"` both mean false.
    pub fn read_bool(&mut self) -> Result<bool, ProtocolError> {
        let s = self.take_str()?;
        Ok(s == "1")
    }

    /// Read an optional `i32`. `None` if the field is empty or the
    /// [`UNSET_INT`] sentinel.
    pub fn read_opt_i32(&mut self) -> Result<Option<i32>, ProtocolError> {
        let s = self.take_str()?;
        if s.is_empty() {
            return Ok(None);
        }
        let v: i32 = s.parse()?;
        Ok(if v == UNSET_INT { None } else { Some(v) })
    }

    /// Read an optional `i64`. `None` if empty or [`UNSET_LONG`].
    pub fn read_opt_i64(&mut self) -> Result<Option<i64>, ProtocolError> {
        let s = self.take_str()?;
        if s.is_empty() {
            return Ok(None);
        }
        let v: i64 = s.parse()?;
        Ok(if v == UNSET_LONG { None } else { Some(v) })
    }

    /// Read an optional `f64`. `None` if empty or matches [`UNSET_DOUBLE_STR`]
    /// exactly, or parses to [`UNSET_DOUBLE`].
    pub fn read_opt_f64(&mut self) -> Result<Option<f64>, ProtocolError> {
        let s = self.take_str()?;
        if s.is_empty() || s == UNSET_DOUBLE_STR {
            return Ok(None);
        }
        let v: f64 = s.parse()?;
        // Allow byte-for-byte round-trip of the sentinel: anything that
        // parses to UNSET_DOUBLE collapses to None.
        Ok(if v == UNSET_DOUBLE { None } else { Some(v) })
    }

    /// Read an optional `String` — `None` if the field is empty.
    pub fn read_opt_string(&mut self) -> Result<Option<String>, ProtocolError> {
        let s = self.take_str()?;
        if s.is_empty() {
            Ok(None)
        } else {
            Ok(Some(s.to_owned()))
        }
    }
}

// ---------------------------------------------------------------------------
// FieldWriter — builder for NUL-delimited payload fields.
// ---------------------------------------------------------------------------

/// Writer-side companion to [`FieldReader`]. Pushes fields onto an internal
/// `Vec<u8>`, each followed by a NUL terminator.
#[derive(Debug, Default)]
pub struct FieldWriter {
    buf: Vec<u8>,
    count: usize,
}

impl FieldWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            count: 0,
        }
    }

    /// Finalise — return the underlying payload bytes (NUL-terminated fields,
    /// ready to be length-prefixed by the framing codec).
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Borrow the in-progress payload.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Number of fields written so far.
    pub fn field_count(&self) -> usize {
        self.count
    }

    /// Split the payload into `Bytes` fields, mirroring what the framing
    /// codec will later split off the wire. Consumes `self` — intended for
    /// tests that want to roundtrip through `FieldReader` without going
    /// through the full codec.
    pub fn into_fields(self) -> Vec<Bytes> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(self.count);
        let mut start = 0;
        for (i, &b) in self.buf.iter().enumerate() {
            if b == 0 {
                out.push(Bytes::copy_from_slice(&self.buf[start..i]));
                start = i + 1;
            }
        }
        if start < self.buf.len() {
            out.push(Bytes::copy_from_slice(&self.buf[start..]));
        }
        out
    }

    fn push_raw(&mut self, bytes: &[u8]) {
        self.buf.reserve(bytes.len() + 1);
        self.buf.extend_from_slice(bytes);
        self.buf.push(0);
        self.count += 1;
    }

    pub fn write_i32(&mut self, v: i32) -> &mut Self {
        let mut buf = itoa_i32(v);
        self.push_raw(buf.as_bytes());
        buf.clear();
        self
    }

    pub fn write_i64(&mut self, v: i64) -> &mut Self {
        let buf = itoa_i64(v);
        self.push_raw(buf.as_bytes());
        self
    }

    pub fn write_f64(&mut self, v: f64) -> &mut Self {
        let s = format_f64(v);
        self.push_raw(s.as_bytes());
        self
    }

    pub fn write_str(&mut self, s: &str) -> &mut Self {
        self.push_raw(s.as_bytes());
        self
    }

    pub fn write_string(&mut self, s: &str) -> &mut Self {
        self.write_str(s)
    }

    pub fn write_bool(&mut self, v: bool) -> &mut Self {
        self.push_raw(if v { b"1" } else { b"0" });
        self
    }

    pub fn write_opt_i32(&mut self, v: Option<i32>) -> &mut Self {
        match v {
            Some(x) => self.write_i32(x),
            None => self.write_i32(UNSET_INT),
        }
    }

    pub fn write_opt_i64(&mut self, v: Option<i64>) -> &mut Self {
        match v {
            Some(x) => self.write_i64(x),
            None => self.write_i64(UNSET_LONG),
        }
    }

    pub fn write_opt_f64(&mut self, v: Option<f64>) -> &mut Self {
        match v {
            Some(x) => self.write_f64(x),
            None => {
                // Use the canonical sentinel string to match real IB traffic
                // byte-for-byte.
                self.push_raw(UNSET_DOUBLE_STR.as_bytes());
                self
            }
        }
    }

    pub fn write_opt_string(&mut self, v: Option<&str>) -> &mut Self {
        match v {
            Some(s) => self.write_str(s),
            None => self.write_str(""),
        }
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers — keep them cheap + allocation-light.
// ---------------------------------------------------------------------------

fn itoa_i32(v: i32) -> String {
    v.to_string()
}

fn itoa_i64(v: i64) -> String {
    v.to_string()
}

/// Format an `f64` using IB's textual conventions: integral doubles stay
/// integral (`100` not `100.0`), the `UNSET_DOUBLE` sentinel matches the
/// canonical string exactly.
fn format_f64(v: f64) -> String {
    if v == UNSET_DOUBLE {
        return UNSET_DOUBLE_STR.to_string();
    }
    if v.is_nan() {
        // IB doesn't transmit NaN; encode as the sentinel to keep wire sanity.
        return UNSET_DOUBLE_STR.to_string();
    }
    // Rust's default `{}` already matches IB's common output shape for
    // sensible values (e.g. `1.5` → `"1.5"`, `100.0` → `"100"`).
    if v.fract() == 0.0 && v.is_finite() && v.abs() < 1e16 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ---- FieldWriter output shape ---------------------------------------

    #[test]
    fn writer_produces_nul_terminated_fields() {
        let mut w = FieldWriter::new();
        w.write_i32(9)
            .write_i32(1)
            .write_str("DU1")
            .write_bool(true);
        assert_eq!(w.as_bytes(), b"9\x001\x00DU1\x001\x00");
        assert_eq!(w.field_count(), 4);
    }

    #[test]
    fn writer_encodes_unset_double_as_canonical_string() {
        let mut w = FieldWriter::new();
        w.write_opt_f64(None);
        assert_eq!(w.as_bytes(), b"1.7976931348623157E308\x00");
    }

    #[test]
    fn writer_encodes_integral_double_without_decimal_point() {
        let mut w = FieldWriter::new();
        w.write_f64(100.0);
        assert_eq!(w.as_bytes(), b"100\x00");
    }

    #[test]
    fn writer_encodes_fractional_double() {
        let mut w = FieldWriter::new();
        w.write_f64(1.5);
        assert_eq!(w.as_bytes(), b"1.5\x00");
    }

    // ---- Reader round-trip ----------------------------------------------

    #[test]
    fn reader_roundtrips_all_primitives() {
        let mut w = FieldWriter::new();
        w.write_i32(9)
            .write_i64(1_234_567_890_123)
            .write_f64(42.25)
            .write_string("hello")
            .write_bool(true)
            .write_bool(false)
            .write_opt_i32(Some(-7))
            .write_opt_i32(None)
            .write_opt_i64(Some(99))
            .write_opt_i64(None)
            .write_opt_f64(Some(1.5))
            .write_opt_f64(None)
            .write_opt_string(Some("x"))
            .write_opt_string(None);
        let fields = w.into_fields();
        let mut r = FieldReader::new(&fields);

        assert_eq!(r.read_i32().unwrap(), 9);
        assert_eq!(r.read_i64().unwrap(), 1_234_567_890_123);
        assert_eq!(r.read_f64().unwrap(), 42.25);
        assert_eq!(r.read_string().unwrap(), "hello");
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.read_opt_i32().unwrap(), Some(-7));
        assert_eq!(r.read_opt_i32().unwrap(), None);
        assert_eq!(r.read_opt_i64().unwrap(), Some(99));
        assert_eq!(r.read_opt_i64().unwrap(), None);
        assert_eq!(r.read_opt_f64().unwrap(), Some(1.5));
        assert_eq!(r.read_opt_f64().unwrap(), None);
        assert_eq!(r.read_opt_string().unwrap(), Some("x".to_string()));
        assert_eq!(r.read_opt_string().unwrap(), None);
        assert!(r.is_empty());
    }

    #[test]
    fn reader_rejects_empty_required_int() {
        let fields = vec![Bytes::from_static(b"")];
        let mut r = FieldReader::new(&fields);
        assert!(matches!(r.read_i32(), Err(ProtocolError::Field(_))));
    }

    #[test]
    fn reader_rejects_read_past_end() {
        let fields: Vec<Bytes> = Vec::new();
        let mut r = FieldReader::new(&fields);
        let err = r.read_i32().unwrap_err();
        assert!(matches!(err, ProtocolError::Field(_)));
    }

    #[test]
    fn reader_opt_i32_accepts_sentinel() {
        // Writer emits UNSET_INT for None; reader must collapse it back.
        let mut w = FieldWriter::new();
        w.write_opt_i32(None);
        let fields = w.into_fields();
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_opt_i32().unwrap(), None);
    }

    #[test]
    fn reader_opt_string_empty_is_none() {
        let fields = vec![Bytes::from_static(b"")];
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_opt_string().unwrap(), None);
    }

    #[test]
    fn reader_bool_permissive() {
        // "1" → true, anything else (empty, "0", "2", "true") → false.
        let fields = vec![
            Bytes::from_static(b"1"),
            Bytes::from_static(b"0"),
            Bytes::from_static(b""),
            Bytes::from_static(b"true"),
        ];
        let mut r = FieldReader::new(&fields);
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
    }

    #[test]
    fn reader_parses_scientific_f64() {
        let fields = vec![Bytes::from_static(UNSET_DOUBLE_STR.as_bytes())];
        let mut r = FieldReader::new(&fields);
        // Directly reading f64 should give UNSET_DOUBLE.
        assert_eq!(r.read_f64().unwrap(), UNSET_DOUBLE);
    }

    #[test]
    fn reader_tracks_position() {
        let mut w = FieldWriter::new();
        w.write_i32(1).write_i32(2).write_i32(3);
        let fields = w.into_fields();
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.pos(), 0);
        assert_eq!(r.remaining(), 3);
        r.read_i32().unwrap();
        assert_eq!(r.pos(), 1);
        assert_eq!(r.remaining(), 2);
    }

    // ---- Proptest roundtrip ---------------------------------------------

    proptest! {
        #[test]
        fn prop_i32_roundtrip(v: i32) {
            let mut w = FieldWriter::new();
            w.write_i32(v);
            let fields = w.into_fields();
            let mut r = FieldReader::new(&fields);
            prop_assert_eq!(r.read_i32().unwrap(), v);
        }

        #[test]
        fn prop_i64_roundtrip(v: i64) {
            let mut w = FieldWriter::new();
            w.write_i64(v);
            let fields = w.into_fields();
            let mut r = FieldReader::new(&fields);
            prop_assert_eq!(r.read_i64().unwrap(), v);
        }

        #[test]
        fn prop_opt_i32_roundtrip(v: Option<i32>) {
            // Filter out the sentinel — it collapses to None by design.
            prop_assume!(v != Some(UNSET_INT));
            let mut w = FieldWriter::new();
            w.write_opt_i32(v);
            let fields = w.into_fields();
            let mut r = FieldReader::new(&fields);
            prop_assert_eq!(r.read_opt_i32().unwrap(), v);
        }

        #[test]
        fn prop_opt_i64_roundtrip(v: Option<i64>) {
            prop_assume!(v != Some(UNSET_LONG));
            let mut w = FieldWriter::new();
            w.write_opt_i64(v);
            let fields = w.into_fields();
            let mut r = FieldReader::new(&fields);
            prop_assert_eq!(r.read_opt_i64().unwrap(), v);
        }

        #[test]
        fn prop_f64_finite_roundtrip(v in prop::num::f64::NORMAL) {
            prop_assume!(v != UNSET_DOUBLE);
            let mut w = FieldWriter::new();
            w.write_f64(v);
            let fields = w.into_fields();
            let mut r = FieldReader::new(&fields);
            let decoded = r.read_f64().unwrap();
            // Tolerate minor rounding in very large normals (our integral
            // shortcut kicks in only below 1e16).
            let rel = ((decoded - v).abs()) / v.abs().max(1.0);
            prop_assert!(rel < 1e-12, "v={v} decoded={decoded} rel={rel}");
        }

        #[test]
        fn prop_string_roundtrip(s in "[\\PC&&[^\\x00]]{0,32}") {
            let mut w = FieldWriter::new();
            w.write_string(&s);
            let fields = w.into_fields();
            let mut r = FieldReader::new(&fields);
            prop_assert_eq!(r.read_string().unwrap(), s);
        }
    }
}
