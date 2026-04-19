//! Field codec — NUL-delimited fields, sentinel handling. Stage 02 fills in.

use crate::protocol::CodecError;

/// Read one NUL-terminated field from `buf`, advancing the cursor.
pub fn read_field(_buf: &mut &[u8]) -> Result<String, CodecError> {
    todo!("Stage 02 — field codec read")
}

/// Write a field + trailing NUL.
pub fn write_field(_dst: &mut Vec<u8>, _value: &str) {
    todo!("Stage 02 — field codec write")
}
