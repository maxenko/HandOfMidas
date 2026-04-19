//! `cargo-fuzz` target for the incoming-message parser.
//!
//! See `plan/ib-sim/02-protocol-layer.md` §"Fuzzing target (`cargo-fuzz`)".
//! The harness feeds arbitrary bytes into `TwsCodec::decode` and asserts that
//! the decoder never panics, regardless of input shape.
//!
//! Run locally (requires nightly toolchain + `cargo-fuzz`):
//!
//! ```bash
//! cargo +nightly install cargo-fuzz
//! cd crates/midas-ib-sim
//! cargo +nightly fuzz run decode_incoming -- -max_total_time=60
//! ```
//!
//! CI invocation lives in `.github/workflows/rust.yml` under the
//! `sim_fuzz_nightly` job (scheduled trigger only).

#![no_main]

use bytes::BytesMut;
use libfuzzer_sys::fuzz_target;
use midas_ib_sim::protocol::TwsCodec;
use tokio_util::codec::Decoder;

fuzz_target!(|data: &[u8]| {
    // Start the codec already in the post-handshake `Framed` state so that
    // every byte of the fuzz input is fed straight into the length-prefixed
    // frame parser — the interesting attack surface.
    let mut codec = TwsCodec::new_framed();
    let mut buf = BytesMut::from(data);

    // `decode` should never panic, no matter what bytes arrive. Errors and
    // `Ok(None)` (incomplete frame) are both valid outcomes; the only
    // unacceptable result is a crash.
    loop {
        match codec.decode(&mut buf) {
            Ok(Some(_frame)) => continue,
            Ok(None) => break,
            Err(_) => break,
        }
    }
});
