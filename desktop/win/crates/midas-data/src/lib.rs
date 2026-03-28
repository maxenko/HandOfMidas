//! midas-data: Data storage, SoA buffers, binary format, LOD, and mmap.
//!
//! Depends on: midas-core

pub mod candle;
pub mod binary;
pub mod lod;

// ── Planned modules (uncomment as implemented) ──────────────────────
// pub mod symbol;    // SymbolId registry, symbol metadata
// pub mod timeframe; // Timeframe enum, boundary math
// pub mod cache;     // In-memory LRU cache for loaded data

/// Re-export primary candle types at the crate root for ergonomic imports.
/// Example: `use midas_data::{CandleBuffer, CandleSlice};`
pub use candle::{CandleBuffer, CandleSlice};

/// Re-export binary format types.
/// Example: `use midas_data::{MidasHeader, CandleRecord, MmapCandleFile};`
pub use binary::{
    BinaryError, CandleRecord, MidasHeader, MmapCandleFile,
    read_midas_file, write_midas_file,
    HEADER_SIZE, MIDAS_MAGIC, MIDAS_VERSION, RECORD_SIZE,
};

/// Re-export LOD functions.
/// Example: `use midas_data::{downsample_minmax, downsample_lttb, select_lod};`
pub use lod::{downsample_lttb, downsample_minmax, select_lod};
