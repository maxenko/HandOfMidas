//! Anonymize captured sessions so they can be committed to git.
//!
//! The anonymizer is a byte-level rewriter that finds sensitive patterns in
//! payloads (account codes, perm IDs, exec IDs) and replaces them with
//! deterministic synthetic equivalents.
//!
//! # Determinism
//!
//! All replacements are keyed off a committed salt (see
//! `fixtures/sessions/anonymize.config.yaml`). Running the tool twice on the
//! same input produces byte-identical output. Running it on an
//! already-anonymized input is a no-op (idempotent) because synthetic account
//! codes fall into the reserved `DU0000000..DU0000999` range which the
//! pattern excludes.
//!
//! # Patterns covered
//!
//! | Kind | Shape | Synthetic form |
//! |------|-------|----------------|
//! | Paper account codes | `DU\d{7}` (excluding `DU0000xxx`) | `DU0000` + 3 hashed digits |
//! | Retail account codes | `U\d{7}` | `U0000` + 3 hashed digits |
//! | Advisor-account codes | `F\d{7}` | `F0000` + 3 hashed digits |
//! | Perm IDs | 10–19 digit runs | `sha256(salt||in)` → digit sequence, length preserved |
//! | Exec IDs | `[0-9a-f]{6,32}\.\w{4,16}\.\w{2,8}` | `sha256(salt||in)` |
//!
//! The perm-id remapping preserves uniqueness: the *same* input perm id maps
//! to the *same* synthetic id within a run AND across runs (salt-keyed).
//!
//! # Cash balances
//!
//! Cash amounts are **not** anonymized. They're just numbers in a TWS payload
//! with no structural context the anonymizer can detect reliably; dropping
//! them from recordings would also break calibrated synthetic generators
//! that model spread + volume distributions. A caller who needs cash
//! obfuscation should strip `ACCT_VALUE` + `NETLIQUIDATION` keys before
//! committing the fixture.
//!
//! # Non-goals
//!
//! The anonymizer does NOT alter frame lengths. All substitutions are
//! length-preserving so the recorded wire frames remain valid TWS payloads
//! that downstream replay tooling can ingest unchanged.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::session::pcap::{TwsPcapReader, TwsPcapWriter};

/// Fixed reserved range for synthetic *paper* account codes. Real IB paper
/// accounts never fall into `DU0000000..DU0000999`, so the anonymizer can
/// safely skip codes already in this range (idempotency).
pub const RESERVED_SYNTHETIC_ACCOUNT_PREFIX: &str = "DU0000";
/// Reserved synthetic range for *retail* `U` accounts.
pub const RESERVED_SYNTHETIC_RETAIL_PREFIX: &str = "U0000";
/// Reserved synthetic range for *advisor/FA* `F` accounts.
pub const RESERVED_SYNTHETIC_FA_PREFIX: &str = "F0000";

/// Configuration for the anonymizer. Loaded from YAML.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnonymizeConfig {
    /// Hex-encoded salt used for all deterministic hashing.
    pub salt: String,
    /// Optional explicit map: real account code → synthetic. Takes precedence
    /// over the hash-based fallback.
    #[serde(default)]
    pub account_map: HashMap<String, String>,
    /// Optional override: real exec id → synthetic.
    #[serde(default)]
    pub exec_id_map: HashMap<String, String>,
    /// When `true`, also strip order IDs (seldom wanted — breaks replay).
    #[serde(default)]
    pub strip_order_ids: bool,
}

impl Default for AnonymizeConfig {
    fn default() -> Self {
        Self {
            // Intentionally committed placeholder salt — the real value lives
            // at `fixtures/sessions/anonymize.config.yaml`.
            salt: "midas-ib-sim-default-salt".to_string(),
            account_map: HashMap::new(),
            exec_id_map: HashMap::new(),
            strip_order_ids: false,
        }
    }
}

impl AnonymizeConfig {
    /// Load a YAML config from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AnonymizeError> {
        let text = std::fs::read_to_string(path)?;
        let cfg: AnonymizeConfig = serde_yaml::from_str(&text)?;
        Ok(cfg)
    }
}

/// Errors from the anonymization pass.
#[derive(Debug, thiserror::Error)]
pub enum AnonymizeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

/// Stateful anonymizer. Builds stable remappings as it walks a stream so
/// every occurrence of the same original code yields the same synthetic
/// replacement.
pub struct Anonymizer {
    config: AnonymizeConfig,
    /// Cache of synthesised account codes (for consistency within a stream
    /// and deterministic across runs because seeded by the salt).
    account_cache: HashMap<String, String>,
    /// Cache of synthesised exec IDs.
    exec_id_cache: HashMap<String, String>,
    /// Cache of synthesised perm IDs.
    perm_id_cache: HashMap<String, String>,
}

impl Anonymizer {
    /// Construct a new anonymizer from a config.
    pub fn new(config: AnonymizeConfig) -> Self {
        Self {
            config,
            account_cache: HashMap::new(),
            exec_id_cache: HashMap::new(),
            perm_id_cache: HashMap::new(),
        }
    }

    /// Process a pcap stream from `src` into `dst`, reusing the source header.
    pub fn process_stream<R: Read, W: Write>(
        &mut self,
        src: R,
        dst: W,
    ) -> Result<usize, AnonymizeError> {
        let mut reader = TwsPcapReader::with_reader(src)?;
        let header = *reader.header();
        let mut writer = TwsPcapWriter::with_writer(dst, header)?;
        let mut count = 0usize;
        while let Some(mut rec) = reader.read_record()? {
            rec.payload = self.anonymize_bytes(&rec.payload);
            writer.write_record(&rec)?;
            count += 1;
        }
        writer.flush()?;
        Ok(count)
    }

    /// Convenience wrapper: anonymize `src_path` into `dst_path`, preserving
    /// the header and (raw-)pcap layout.
    pub fn process_files(
        &mut self,
        src_path: impl AsRef<Path>,
        dst_path: impl AsRef<Path>,
    ) -> Result<usize, AnonymizeError> {
        let src_reader = TwsPcapReader::open(src_path.as_ref())?;
        // Reuse the source header so the resulting file carries the same
        // start timestamp / server version.
        let header = *src_reader.header();
        let mut writer = TwsPcapWriter::create(dst_path.as_ref(), header)?;
        let mut reader = src_reader;
        let mut count = 0usize;
        while let Some(mut rec) = reader.read_record()? {
            rec.payload = self.anonymize_bytes(&rec.payload);
            writer.write_record(&rec)?;
            count += 1;
        }
        writer.flush()?;
        Ok(count)
    }

    /// Anonymize one payload. Pure function of input + internal caches.
    pub fn anonymize_bytes(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            // Account codes: DU / U / F prefix + exactly 7 digits. Order
            // matters — try account first so a DU account isn't consumed
            // by the exec-id matcher.
            //
            // The boundary-prev check stops `U` / `F` single-char prefixes
            // from being matched inside a larger identifier (e.g. the
            // trailing `U1234567` of `XU1234567`). `DU` is safe without
            // the boundary check because real payloads use `DU` only as
            // an account prefix.
            if let Some((len, kind)) = match_account_code_bounded(&input[i..], prev_byte(input, i))
            {
                let original =
                    std::str::from_utf8(&input[i..i + len]).expect("ASCII by construction");
                // Skip already-synthetic codes (idempotency).
                let already_synth = match kind {
                    AccountKind::Paper => original.starts_with(RESERVED_SYNTHETIC_ACCOUNT_PREFIX),
                    AccountKind::Retail => original.starts_with(RESERVED_SYNTHETIC_RETAIL_PREFIX),
                    AccountKind::Fa => original.starts_with(RESERVED_SYNTHETIC_FA_PREFIX),
                };
                if already_synth {
                    out.extend_from_slice(&input[i..i + len]);
                } else {
                    let replacement = self.synth_account(original, kind);
                    out.extend_from_slice(replacement.as_bytes());
                }
                i += len;
                continue;
            }
            // Exec IDs: we match the classic IB shape `xxxxxxxxxxxxxxxx.yyyyyyyy.zz`
            // — hex-ish body, dot, 8-char tail, dot, 2-char suffix.
            if let Some(len) = match_exec_id(&input[i..]) {
                let original =
                    std::str::from_utf8(&input[i..i + len]).expect("ASCII by construction");
                let replacement = self.synth_exec_id(original);
                out.extend_from_slice(replacement.as_bytes());
                i += len;
                continue;
            }
            // Perm IDs: 10-19 digit runs that stand alone (no adjacent
            // alphanumeric neighbours). Must run *after* account matches
            // so we don't shred the `7`-digit suffix of a DU account.
            if let Some(len) = match_perm_id(&input[i..], prev_byte(input, i)) {
                let original =
                    std::str::from_utf8(&input[i..i + len]).expect("ASCII by construction");
                let replacement = self.synth_perm_id(original);
                out.extend_from_slice(replacement.as_bytes());
                i += len;
                continue;
            }
            out.push(input[i]);
            i += 1;
        }
        out
    }

    fn synth_account(&mut self, original: &str, kind: AccountKind) -> String {
        if let Some(over) = self.config.account_map.get(original) {
            return over.clone();
        }
        if let Some(c) = self.account_cache.get(original) {
            return c.clone();
        }
        // Restrict synthetic output to the kind-specific reserved range so
        // the pass is idempotent (the matcher skips codes already there)
        // AND length-preserving (all three input shapes are 8 or 9 chars).
        let mut hasher = Sha256::new();
        hasher.update(self.config.salt.as_bytes());
        hasher.update(b"|account|");
        hasher.update(original.as_bytes());
        let digest = hasher.finalize();
        let mut eight = [0u8; 8];
        eight.copy_from_slice(&digest[..8]);
        let n = u64::from_be_bytes(eight) % 1000;
        // DU has a 2-char prefix, U/F are 1-char — insert an extra zero
        // into the reserved range for U/F so the synthetic code matches
        // the source length exactly. DU1234567 → DU0000ABC (9 chars),
        // U1234567  → U0000ABC  (8 chars), F1234567 → F0000ABC (8 chars).
        let synth = match kind {
            AccountKind::Paper => format!("{RESERVED_SYNTHETIC_ACCOUNT_PREFIX}{n:03}"),
            AccountKind::Retail => format!("{RESERVED_SYNTHETIC_RETAIL_PREFIX}{n:03}"),
            AccountKind::Fa => format!("{RESERVED_SYNTHETIC_FA_PREFIX}{n:03}"),
        };
        debug_assert_eq!(synth.len(), original.len());
        self.account_cache
            .insert(original.to_string(), synth.clone());
        synth
    }

    fn synth_perm_id(&mut self, original: &str) -> String {
        if let Some(c) = self.perm_id_cache.get(original) {
            return c.clone();
        }
        let mut hasher = Sha256::new();
        hasher.update(self.config.salt.as_bytes());
        hasher.update(b"|perm_id|");
        hasher.update(original.as_bytes());
        let digest = hasher.finalize();
        // Turn the digest into a digit-only string matching the original
        // length. Each output digit is `digest[i % 32] % 10`; that's not
        // cryptographically uniform but is deterministic, length-preserving,
        // and collision-resistant enough that two distinct real perm ids
        // essentially never map to the same synthetic within a session.
        let mut out = String::with_capacity(original.len());
        for i in 0..original.len() {
            // First char avoids a leading zero so the synthetic retains
            // the wire-shape of a real 10+ significant-digit perm id.
            let ch = if i == 0 {
                let d = (digest[0] % 9) + 1; // 1..=9
                char::from(b'0' + d)
            } else {
                char::from(b'0' + (digest[i % digest.len()] % 10))
            };
            out.push(ch);
        }
        debug_assert_eq!(out.len(), original.len());
        self.perm_id_cache.insert(original.to_string(), out.clone());
        out
    }

    fn synth_exec_id(&mut self, original: &str) -> String {
        if let Some(over) = self.config.exec_id_map.get(original) {
            return over.clone();
        }
        if let Some(c) = self.exec_id_cache.get(original) {
            return c.clone();
        }
        let mut hasher = Sha256::new();
        hasher.update(self.config.salt.as_bytes());
        hasher.update(b"|exec|");
        hasher.update(original.as_bytes());
        let digest = hasher.finalize();
        // Preserve the shape len.len().len() of the original when possible.
        let parts: Vec<&str> = original.split('.').collect();
        let synth = if parts.len() == 3 {
            let hex: String = digest
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let mid: String = digest[8..12].iter().map(|b| format!("{b:02x}")).collect();
            let tail: String = digest[12..13].iter().map(|b| format!("{b:02x}")).collect();
            // Pad each segment to the exact source length so the byte count
            // is preserved.
            let a = pad_or_truncate(&hex, parts[0].len());
            let b = pad_or_truncate(&mid, parts[1].len());
            let c = pad_or_truncate(&tail, parts[2].len());
            format!("{a}.{b}.{c}")
        } else {
            let hex: String = digest
                .iter()
                .take(original.len() / 2 + 1)
                .map(|b| format!("{b:02x}"))
                .collect();
            pad_or_truncate(&hex, original.len())
        };
        self.exec_id_cache
            .insert(original.to_string(), synth.clone());
        synth
    }
}

fn pad_or_truncate(s: &str, target: usize) -> String {
    if s.len() == target {
        s.to_string()
    } else if s.len() > target {
        s[..target].to_string()
    } else {
        let mut out = s.to_string();
        while out.len() < target {
            out.push('0');
        }
        out
    }
}

/// Account-matcher wrapper that rejects `U` / `F` matches when the
/// preceding byte is alphanumeric — prevents `XU1234567` from being
/// classified as retail account `U1234567`. `DU` bypasses this guard
/// since its two-char prefix is unambiguous in every payload shape we
/// care about.
fn match_account_code_bounded(bytes: &[u8], prev: Option<u8>) -> Option<(usize, AccountKind)> {
    let m = match_account_code(bytes)?;
    if matches!(m.1, AccountKind::Retail | AccountKind::Fa) {
        if let Some(p) = prev {
            if p.is_ascii_alphanumeric() || p == b'_' {
                return None;
            }
        }
    }
    Some(m)
}

/// Which flavour of IB account the matcher recognised.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AccountKind {
    /// Paper-trading account — `DU` + 7 digits (total 9 chars).
    Paper,
    /// Retail cash/margin account — `U` + 7 digits (total 8 chars).
    Retail,
    /// Financial-advisor account — `F` + 7 digits (total 8 chars).
    Fa,
}

/// Returns `(length, kind)` if `bytes` starts with a real account-code
/// pattern. Supports:
///
/// - `DU\d{7}` — paper accounts (e.g. `DU1234567`).
/// - `U\d{7}` — retail accounts (e.g. `U1234567`).
/// - `F\d{7}` — FA/advisor accounts (e.g. `F1234567`).
///
/// The `U` / `F` variants are guarded by a previous-byte check via
/// `prev_byte` above at the call site so they don't swallow the second
/// char of something like `DU1234567` or `XF1234567`.
fn match_account_code(bytes: &[u8]) -> Option<(usize, AccountKind)> {
    // Longest first so `DU` doesn't get consumed as a `U`.
    if bytes.len() >= 9
        && bytes[0] == b'D'
        && bytes[1] == b'U'
        && bytes[2..9].iter().all(u8::is_ascii_digit)
        && !bytes.get(9).is_some_and(u8::is_ascii_digit)
    {
        return Some((9, AccountKind::Paper));
    }
    if bytes.len() >= 8 {
        let kind = match bytes[0] {
            b'U' => AccountKind::Retail,
            b'F' => AccountKind::Fa,
            _ => return None,
        };
        if bytes[1..8].iter().all(u8::is_ascii_digit)
            && !bytes.get(8).is_some_and(u8::is_ascii_digit)
        {
            return Some((8, kind));
        }
    }
    None
}

/// Returns the length of a run of 10–19 digits starting at `bytes`
/// iff neither the preceding byte nor the following byte is an
/// alphanumeric character. The upper bound of 19 matches an `i64`
/// ceiling; the lower bound of 10 filters out short ints (order ids,
/// client ids, contract ids) that must not be mangled.
fn match_perm_id(bytes: &[u8], prev: Option<u8>) -> Option<usize> {
    const MIN: usize = 10;
    const MAX: usize = 19;
    if let Some(p) = prev {
        if p.is_ascii_alphanumeric() || p == b'_' {
            return None;
        }
    }
    let mut i = 0;
    while i < bytes.len() && i < MAX && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i < MIN {
        return None;
    }
    // Don't greedily consume into an adjacent alphanumeric — might be an
    // arbitrary numeric token inside a larger identifier.
    if bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(i)
}

/// Safe look-back helper for the perm-id boundary check.
fn prev_byte(input: &[u8], i: usize) -> Option<u8> {
    if i == 0 {
        None
    } else {
        Some(input[i - 1])
    }
}

/// IB exec IDs look like `0000e1a7.66218745.01.01` — up to four dot-separated
/// segments of hex/digit chars. Matches the classic three-segment form.
fn match_exec_id(bytes: &[u8]) -> Option<usize> {
    // Greedy: up to 32 hex/digit chars, dot, up to 16 more, dot, up to 8 more.
    let mut i = 0;
    while i < bytes.len() && i < 32 && is_exec_char(bytes[i]) {
        i += 1;
    }
    let a = i;
    if a < 6 {
        return None;
    }
    if bytes.get(i) != Some(&b'.') {
        return None;
    }
    i += 1;
    let mid_start = i;
    while i < bytes.len() && (i - mid_start) < 16 && is_exec_char(bytes[i]) {
        i += 1;
    }
    if i - mid_start < 4 {
        return None;
    }
    if bytes.get(i) != Some(&b'.') {
        return None;
    }
    i += 1;
    let tail_start = i;
    while i < bytes.len() && (i - tail_start) < 8 && is_exec_char(bytes[i]) {
        i += 1;
    }
    if i - tail_start < 2 {
        return None;
    }
    // Guard — don't match if followed by another exec-char (ambiguous).
    if bytes.get(i).copied().is_some_and(is_exec_char) {
        return None;
    }
    Some(i)
}

fn is_exec_char(b: u8) -> bool {
    b.is_ascii_digit() || matches!(b, b'a'..=b'f' | b'A'..=b'F')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_pattern_matches_classic_du() {
        assert_eq!(
            match_account_code(b"DU1234567"),
            Some((9, AccountKind::Paper))
        );
        assert_eq!(
            match_account_code(b"DU1234567\0rest"),
            Some((9, AccountKind::Paper))
        );
        assert_eq!(match_account_code(b"DU123456"), None); // too short
        assert_eq!(match_account_code(b"XX1234567"), None);
        assert_eq!(match_account_code(b"DU12345678"), None); // extra digit
    }

    #[test]
    fn account_pattern_matches_retail_and_fa() {
        assert_eq!(
            match_account_code(b"U1234567"),
            Some((8, AccountKind::Retail))
        );
        assert_eq!(match_account_code(b"F9876543"), Some((8, AccountKind::Fa)));
        // Trailing digit disqualifies.
        assert_eq!(match_account_code(b"U12345678"), None);
        // Short/no-digit input.
        assert_eq!(match_account_code(b"U123456"), None);
        assert_eq!(match_account_code(b"Ufoobar7"), None);
    }

    #[test]
    fn anonymize_replaces_retail_account_deterministically() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let inp = b" U1234567 acct";
        let out = a.anonymize_bytes(inp);
        assert_eq!(out.len(), inp.len());
        let s = std::str::from_utf8(&out).unwrap();
        // U accounts must remap into the U0000xxx reserved range.
        assert!(
            s.contains(RESERVED_SYNTHETIC_RETAIL_PREFIX),
            "retail prefix missing in {s}"
        );
        assert!(
            !s.contains("U1234567"),
            "retail account leaked into anonymised output: {s}"
        );
    }

    #[test]
    fn anonymize_replaces_fa_account_deterministically() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let inp = b" F1234567 acct";
        let out = a.anonymize_bytes(inp);
        assert_eq!(out.len(), inp.len());
        let s = std::str::from_utf8(&out).unwrap();
        assert!(
            s.contains(RESERVED_SYNTHETIC_FA_PREFIX),
            "FA prefix missing in {s}"
        );
        assert!(
            !s.contains("F1234567"),
            "FA account leaked into anonymised output: {s}"
        );
    }

    #[test]
    fn perm_id_matches_standalone_10_digit_run() {
        assert_eq!(match_perm_id(b"1656474657", None), Some(10));
        assert_eq!(match_perm_id(b"1656474657 rest", None), Some(10));
        // Too short: order id, not perm id.
        assert_eq!(match_perm_id(b"123456789", None), None);
        // Prior alphanumeric: don't start a match.
        assert_eq!(match_perm_id(b"1656474657", Some(b'A')), None);
        // Adjacent alphanumeric after: don't greedily eat into larger tokens.
        assert_eq!(match_perm_id(b"1656474657A", None), None);
    }

    #[test]
    fn anonymize_replaces_perm_id_deterministically() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg.clone());
        let mut b = Anonymizer::new(cfg);
        let inp = b"perm_id=1656474657 price=100";
        let out_a = a.anonymize_bytes(inp);
        let out_b = b.anonymize_bytes(inp);
        assert_eq!(out_a, out_b);
        assert_eq!(out_a.len(), inp.len());
        let s = std::str::from_utf8(&out_a).unwrap();
        assert!(!s.contains("1656474657"), "perm id leaked: {s}");
    }

    #[test]
    fn anonymize_same_perm_id_maps_consistently() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let inp = b"a=1656474657 b=1656474657 c=9988776655";
        let out = a.anonymize_bytes(inp);
        let s = std::str::from_utf8(&out).unwrap();
        let parts: Vec<&str> = s.split(' ').collect();
        let a_syn = parts[0].trim_start_matches("a=");
        let b_syn = parts[1].trim_start_matches("b=");
        let c_syn = parts[2].trim_start_matches("c=");
        assert_eq!(a_syn, b_syn);
        assert_ne!(a_syn, c_syn);
    }

    #[test]
    fn anonymize_replaces_account_deterministically() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg.clone());
        let mut b = Anonymizer::new(cfg);
        let inp = b"acct=DU1234567 bal=100";
        let out_a = a.anonymize_bytes(inp);
        let out_b = b.anonymize_bytes(inp);
        assert_eq!(out_a, out_b);
        assert_ne!(out_a, inp);
        // Length preserved.
        assert_eq!(out_a.len(), inp.len());
    }

    #[test]
    fn anonymize_is_idempotent_on_reserved_range() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let inp = b"sim-account=DU0000042 balance=1000";
        // Second pass on first-pass output must be stable.
        let first = a.anonymize_bytes(inp);
        let second = a.anonymize_bytes(&first);
        assert_eq!(first, second);
    }

    #[test]
    fn anonymize_full_roundtrip_maps_consistently() {
        // Same real account appearing twice → same synthetic both times.
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let inp = b"A=DU1234567 B=DU1234567 C=DU9999999";
        let out = a.anonymize_bytes(inp);
        // Two instances of DU1234567 must map to the same synthetic.
        let s = std::str::from_utf8(&out).unwrap();
        let parts: Vec<&str> = s.split(' ').collect();
        let a_syn = parts[0].trim_start_matches("A=");
        let b_syn = parts[1].trim_start_matches("B=");
        let c_syn = parts[2].trim_start_matches("C=");
        assert_eq!(a_syn, b_syn);
        assert_ne!(a_syn, c_syn);
    }

    #[test]
    fn anonymize_exec_id_matches_three_segment_form() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let inp = b"exec=0000e1a7.00218745.01";
        let out = a.anonymize_bytes(inp);
        assert_eq!(out.len(), inp.len());
        assert_ne!(out, inp);
    }

    #[test]
    fn anonymize_preserves_non_matching_bytes() {
        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let inp = b"hello world no secrets here";
        let out = a.anonymize_bytes(inp);
        assert_eq!(out, inp);
    }

    #[test]
    fn anonymize_config_override_takes_precedence() {
        let mut cfg = AnonymizeConfig::default();
        cfg.account_map
            .insert("DU1234567".into(), "DU0000042".into());
        let mut a = Anonymizer::new(cfg);
        let out = a.anonymize_bytes(b"DU1234567");
        assert_eq!(&out[..], b"DU0000042");
    }

    #[test]
    fn anonymize_stream_preserves_record_count_and_header() {
        use crate::session::pcap::{
            Direction, TwsPcapHeader, TwsPcapReader, TwsPcapRecord, TwsPcapWriter,
        };
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let src = dir.path().join("raw.tws.pcap");
        let dst = dir.path().join("anon.tws.pcap");

        let hdr = TwsPcapHeader::new(210, 12345);
        {
            let mut w = TwsPcapWriter::create(&src, hdr).unwrap();
            w.write_record(&TwsPcapRecord::new(
                100,
                Direction::ClientToSim,
                b"acct=DU1234567".to_vec(),
            ))
            .unwrap();
            w.write_record(&TwsPcapRecord::new(
                200,
                Direction::SimToClient,
                b"exec=0000e1a7.00218745.01 acct=DU1234567".to_vec(),
            ))
            .unwrap();
        }

        let cfg = AnonymizeConfig::default();
        let mut a = Anonymizer::new(cfg);
        let n = a.process_files(&src, &dst).unwrap();
        assert_eq!(n, 2);

        let r = TwsPcapReader::open(&dst).unwrap();
        assert_eq!(*r.header(), hdr);
        let recs = r.read_all().unwrap();
        assert_eq!(recs.len(), 2);
        for r in &recs {
            assert!(
                !r.payload.windows(9).any(|w| w == b"DU1234567"),
                "raw account leaked"
            );
        }
    }
}
