//! GPU-layout instance types and render-adjacent metadata for chart rendering.
//!
//! This crate is a pure-data leaf: it carries the `#[repr(C)]` /
//! `bytemuck::Pod` GPU instance structs that `midas-render` uploads to
//! WGSL shaders, plus a small set of non-Pod render-metadata types
//! (`AxisLabel`, `CrosshairRender`, `TimelineLabel`) that travel with
//! the data they describe.
//!
//! All pixel coordinates are in logical pixels (pre-DPI-scaling). The
//! projection matrix in the renderer handles the mapping to NDC.
//!
//! **Wire format:** WGSL shaders read the Pod structs in this crate by
//! byte offset. Field order is part of the wire contract — do not
//! reorder fields without updating the matching `.wgsl` shader.

pub mod instances;
pub mod labels;
pub mod timeline;

pub use instances::{
    BadgeInstance, CandleInstance, GridLine, GridLineInstance, SessionBoundary, VolumeInstance,
};
pub use labels::{AxisLabel, CrosshairRender, OhlcvOverlay};
pub use timeline::{Tier, TimelineLabel};
