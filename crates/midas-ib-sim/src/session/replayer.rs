//! [`Replayer`] — read back a `.tws.pcap` recording on virtual time.
//!
//! Emits `sim→client` bytes at their recorded offsets and validates that the
//! client's `client→sim` bytes match the recording, subject to a configurable
//! [`ReplayMode`]:
//!
//! - [`ReplayMode::Strict`]    — exact byte equality; any mismatch halts.
//! - [`ReplayMode::BestEffort`] — the *existence* of a matching client chunk is
//!   checked, but the payload may differ; useful when the client code has been
//!   updated and we don't care about bit-exact reproduction.
//! - [`ReplayMode::IgnoreClient`] — client bytes are consumed but never checked.
//!
//! The module is transport-agnostic: [`Replayer::step`] drives the state
//! machine one record at a time, and the I/O layer (tokio socket, in-memory
//! channel, test harness) decides how to surface emissions and feed back
//! client traffic.

use std::io::Read;

use crate::session::pcap::{Direction, TwsPcapHeader, TwsPcapReader, TwsPcapRecord};

/// Strictness of client→sim byte validation during replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ReplayMode {
    /// Exact byte equality. The default for regression tests.
    #[default]
    Strict,
    /// Accept any client bytes — only the fact that bytes were sent at
    /// roughly the right moment is validated.
    BestEffort,
    /// Never check client bytes; just replay the server side.
    IgnoreClient,
}

/// Error returned while replaying a session.
#[derive(Debug, thiserror::Error)]
pub enum ReplayerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "client→sim mismatch at record #{index} (ts={ts_ns} ns): expected {expected_len} bytes, got {actual_len}"
    )]
    ClientMismatchLen {
        index: usize,
        ts_ns: u64,
        expected_len: usize,
        actual_len: usize,
    },
    #[error(
        "client→sim mismatch at record #{index} (ts={ts_ns} ns): byte {byte_offset} differs (expected 0x{expected:02X}, got 0x{actual:02X})"
    )]
    ClientMismatchBytes {
        index: usize,
        ts_ns: u64,
        byte_offset: usize,
        expected: u8,
        actual: u8,
    },
    #[error("expected a client→sim record but got sim→client at #{index}")]
    UnexpectedDirection { index: usize },
    #[error("replay exhausted — no more records")]
    Exhausted,
}

/// One emission produced by [`Replayer::step`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayEmission {
    /// The sim would emit these bytes to the client at `ts_nanos_since_start`.
    ServerBytes { ts_nanos: u64, bytes: Vec<u8> },
    /// The sim expects the client to have sent these bytes by
    /// `ts_nanos_since_start`. The caller supplies them via
    /// [`Replayer::submit_client_bytes`].
    ExpectClient { ts_nanos: u64, expected_len: usize },
    /// No more records.
    Done,
}

/// Session replayer — wraps a pcap reader and a validation policy.
pub struct Replayer<R: Read> {
    reader: TwsPcapReader<R>,
    mode: ReplayMode,
    next: Option<TwsPcapRecord>,
    index: usize,
}

impl<R: Read> Replayer<R> {
    /// Construct a replayer over a reader. Consumes the header from the
    /// stream.
    pub fn with_reader(reader: R, mode: ReplayMode) -> Result<Self, ReplayerError> {
        let reader = TwsPcapReader::with_reader(reader)?;
        let mut r = Self {
            reader,
            mode,
            next: None,
            index: 0,
        };
        r.refill()?;
        Ok(r)
    }

    /// Access the pcap header (carries server_version_neg and start wall clock).
    pub fn header(&self) -> &TwsPcapHeader {
        self.reader.header()
    }

    /// Replay mode in effect.
    pub fn mode(&self) -> ReplayMode {
        self.mode
    }

    fn refill(&mut self) -> Result<(), ReplayerError> {
        self.next = self.reader.read_record()?;
        Ok(())
    }

    /// Peek at the next emission without advancing.
    pub fn peek(&self) -> ReplayEmission {
        match &self.next {
            None => ReplayEmission::Done,
            Some(r) => match r.direction {
                Direction::SimToClient => ReplayEmission::ServerBytes {
                    ts_nanos: r.ts_nanos_since_start,
                    bytes: r.payload.clone(),
                },
                Direction::ClientToSim => ReplayEmission::ExpectClient {
                    ts_nanos: r.ts_nanos_since_start,
                    expected_len: r.payload.len(),
                },
            },
        }
    }

    /// Advance the replay by one record:
    ///
    /// - `ServerBytes` — caller should write them to the client socket at
    ///   `ts_nanos` on virtual time.
    /// - `ExpectClient` — caller should read the client's next chunk and feed
    ///   it to [`Replayer::submit_client_bytes`] *before* calling [`step`]
    ///   again, unless [`ReplayMode::IgnoreClient`] is in effect.
    /// - `Done` — the recording is exhausted.
    ///
    /// In `IgnoreClient` mode the replayer auto-advances past client records
    /// so the caller only ever sees `ServerBytes` or `Done`.
    pub fn step(&mut self) -> Result<ReplayEmission, ReplayerError> {
        loop {
            let rec = match self.next.take() {
                Some(r) => r,
                None => return Ok(ReplayEmission::Done),
            };
            match rec.direction {
                Direction::SimToClient => {
                    self.refill()?;
                    self.index += 1;
                    return Ok(ReplayEmission::ServerBytes {
                        ts_nanos: rec.ts_nanos_since_start,
                        bytes: rec.payload,
                    });
                }
                Direction::ClientToSim => {
                    if self.mode == ReplayMode::IgnoreClient {
                        self.refill()?;
                        self.index += 1;
                        continue;
                    }
                    let emission = ReplayEmission::ExpectClient {
                        ts_nanos: rec.ts_nanos_since_start,
                        expected_len: rec.payload.len(),
                    };
                    // Preserve the record so submit_client_bytes can validate.
                    self.next = Some(rec);
                    return Ok(emission);
                }
            }
        }
    }

    /// Validate the next `client→sim` record against `actual` and advance.
    /// Must be called while the current record is a `ClientToSim` one.
    pub fn submit_client_bytes(&mut self, actual: &[u8]) -> Result<(), ReplayerError> {
        let Some(rec) = self.next.as_ref() else {
            return Err(ReplayerError::Exhausted);
        };
        if rec.direction != Direction::ClientToSim {
            return Err(ReplayerError::UnexpectedDirection { index: self.index });
        }
        let idx = self.index;
        let ts_ns = rec.ts_nanos_since_start;
        match self.mode {
            ReplayMode::Strict => {
                if rec.payload.len() != actual.len() {
                    return Err(ReplayerError::ClientMismatchLen {
                        index: idx,
                        ts_ns,
                        expected_len: rec.payload.len(),
                        actual_len: actual.len(),
                    });
                }
                for (i, (a, b)) in rec.payload.iter().zip(actual.iter()).enumerate() {
                    if a != b {
                        return Err(ReplayerError::ClientMismatchBytes {
                            index: idx,
                            ts_ns,
                            byte_offset: i,
                            expected: *a,
                            actual: *b,
                        });
                    }
                }
            }
            ReplayMode::BestEffort => {
                // Existence check only — a completely empty client chunk in a
                // slot where we expected data is still a mismatch.
                if rec.payload.is_empty() != actual.is_empty() {
                    return Err(ReplayerError::ClientMismatchLen {
                        index: idx,
                        ts_ns,
                        expected_len: rec.payload.len(),
                        actual_len: actual.len(),
                    });
                }
            }
            ReplayMode::IgnoreClient => {
                // Unreachable in normal flow — `step` auto-skips client records
                // when IgnoreClient is set — but harmless to accept anything.
            }
        }
        // Consume and advance.
        self.next = None;
        self.refill()?;
        self.index += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::pcap::{TwsPcapHeader, TwsPcapWriter};
    use std::io::Cursor;

    fn sample_buf() -> Vec<u8> {
        let hdr = TwsPcapHeader::new(210, 0);
        let mut buf: Vec<u8> = Vec::new();
        let mut w = TwsPcapWriter::with_writer(&mut buf, hdr).unwrap();
        w.write_record(&TwsPcapRecord::new(
            100,
            Direction::SimToClient,
            b"hello".to_vec(),
        ))
        .unwrap();
        w.write_record(&TwsPcapRecord::new(
            200,
            Direction::ClientToSim,
            b"req1".to_vec(),
        ))
        .unwrap();
        w.write_record(&TwsPcapRecord::new(
            300,
            Direction::SimToClient,
            b"resp".to_vec(),
        ))
        .unwrap();
        w.flush().unwrap();
        buf
    }

    #[test]
    fn replayer_strict_roundtrip_matches() {
        let buf = sample_buf();
        let mut r = Replayer::with_reader(Cursor::new(buf), ReplayMode::Strict).unwrap();
        assert!(matches!(
            r.step().unwrap(),
            ReplayEmission::ServerBytes { ts_nanos: 100, .. }
        ));
        assert!(matches!(
            r.step().unwrap(),
            ReplayEmission::ExpectClient {
                ts_nanos: 200,
                expected_len: 4
            }
        ));
        r.submit_client_bytes(b"req1").unwrap();
        assert!(matches!(
            r.step().unwrap(),
            ReplayEmission::ServerBytes { ts_nanos: 300, .. }
        ));
        assert!(matches!(r.step().unwrap(), ReplayEmission::Done));
    }

    #[test]
    fn replayer_strict_rejects_byte_mismatch() {
        let buf = sample_buf();
        let mut r = Replayer::with_reader(Cursor::new(buf), ReplayMode::Strict).unwrap();
        let _ = r.step().unwrap(); // server
        let _ = r.step().unwrap(); // expect client
        let err = r.submit_client_bytes(b"req2").unwrap_err();
        assert!(matches!(err, ReplayerError::ClientMismatchBytes { .. }));
    }

    #[test]
    fn replayer_strict_rejects_length_mismatch() {
        let buf = sample_buf();
        let mut r = Replayer::with_reader(Cursor::new(buf), ReplayMode::Strict).unwrap();
        let _ = r.step().unwrap();
        let _ = r.step().unwrap();
        let err = r.submit_client_bytes(b"req").unwrap_err();
        assert!(matches!(err, ReplayerError::ClientMismatchLen { .. }));
    }

    #[test]
    fn replayer_best_effort_tolerates_payload_drift() {
        let buf = sample_buf();
        let mut r = Replayer::with_reader(Cursor::new(buf), ReplayMode::BestEffort).unwrap();
        let _ = r.step().unwrap();
        let _ = r.step().unwrap();
        // Non-empty, but different content AND different length — still OK.
        r.submit_client_bytes(b"anything-really").unwrap();
        assert!(matches!(
            r.step().unwrap(),
            ReplayEmission::ServerBytes { ts_nanos: 300, .. }
        ));
    }

    #[test]
    fn replayer_ignore_client_skips_client_records() {
        let buf = sample_buf();
        let mut r = Replayer::with_reader(Cursor::new(buf), ReplayMode::IgnoreClient).unwrap();
        // Only server records should surface.
        assert!(matches!(
            r.step().unwrap(),
            ReplayEmission::ServerBytes { ts_nanos: 100, .. }
        ));
        assert!(matches!(
            r.step().unwrap(),
            ReplayEmission::ServerBytes { ts_nanos: 300, .. }
        ));
        assert!(matches!(r.step().unwrap(), ReplayEmission::Done));
    }
}
