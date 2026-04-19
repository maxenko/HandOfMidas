//! `cargo-fuzz` target for the incoming-message parser.
//!
//! This file is a stub — see `plan/ib-sim/02-protocol-layer.md` §"Fuzzing
//! target (`cargo-fuzz`)". Stage 02b ships it with a `todo!()` body so that
//! a later commit (after `cargo-fuzz` is wired into CI) can drop the
//! harness in without touching the crate root.
//!
//! To actually run:
//!
//! ```bash
//! cargo +nightly install cargo-fuzz
//! cargo +nightly fuzz run decode_incoming
//! ```

#![cfg_attr(fuzzing, no_main)]

#[cfg(fuzzing)]
use libfuzzer_sys::fuzz_target;

#[cfg(fuzzing)]
fuzz_target!(|_data: &[u8]| {
    // TODO: wire up TwsCodec + IncomingMsg::parse here once cargo-fuzz
    // entry exists. Stage 02b intentionally keeps this as a no-op scaffold
    // so that flipping on the nightly fuzzer is a one-file change.
    todo!("cargo-fuzz entry pending CI wiring");
});

#[cfg(not(fuzzing))]
fn main() {}
