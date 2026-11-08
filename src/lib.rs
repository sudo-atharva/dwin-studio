//! DWIN T5L DGUS output formats: read and write the files that go into `DWIN_SET/`.
//!
//! Layouts come from DGUS V7.6 (decompiled `TDocumentSpace`, `InputBase`, `ICL`)
//! and are checked byte-for-byte against real DGUS output in `tests/roundtrip.rs`.

pub mod build;
pub mod control;
pub mod icl;
pub mod import;
pub mod picture;
pub mod project;
pub mod render;
pub mod set;
pub mod show;
pub mod sim;
pub mod touch;

/// Big-endian u16 at `i`.
pub(crate) fn be16(d: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([d[i], d[i + 1]])
}

/// Big-endian u32 at `i`.
pub(crate) fn be32(d: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}

#[derive(Debug)]
pub struct FormatError(pub String);

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T, FormatError> {
    Err(FormatError(msg.into()))
}
