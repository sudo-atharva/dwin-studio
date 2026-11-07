//! `13TouchFile.bin`: touch controls.
//!
//! Each control = 16-byte base record + N 16-byte extension lines (lines start with 0xFE).
//! File ends with `FF FF`.
//!
//! Base record (big-endian):
//! ```text
//! 0  page id (low 10 bits; byte0 bits 7..2 = IVR code)
//! 2  x0 y0 x1 y1        touch area (or FF kk 00 00 FF kk.. for keyboard-keyed)
//! 10 next page          FF00 = no page switch
//! 12 pressed page       FF00 = no press effect (else region copied from this page image)
//! 14 code               00xx = plain key/ASCII, FExx = extended control, FDxx = input
//! ```

use crate::{be16, err, FormatError};

pub const NONE: u16 = 0xFF00;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchRecord {
    /// Raw first 2 bytes: page id + IVR bits. Use [`TouchRecord::page`] for the page id.
    pub page_raw: u16,
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
    pub next_page: u16,
    pub pressed_page: u16,
    pub code: u16,
    /// Extension lines, each 16 bytes starting with 0xFE. Content depends on `code`.
    pub ext: Vec<[u8; 16]>,
}

impl TouchRecord {
    pub fn page(&self) -> u16 {
        self.page_raw & 0x03FF
    }

    fn write(&self, out: &mut Vec<u8>) {
        for v in [
            self.page_raw,
            self.x0,
            self.y0,
            self.x1,
            self.y1,
            self.next_page,
            self.pressed_page,
            self.code,
        ] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        for line in &self.ext {
            out.extend_from_slice(line);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TouchFile {
    pub records: Vec<TouchRecord>,
}

impl TouchFile {
    pub fn parse(d: &[u8]) -> Result<Self, FormatError> {
        let mut records = Vec::new();
        let mut i = 0;
        loop {
            if d.len() < i + 2 {
                return err("13TouchFile: missing FF FF terminator");
            }
            if d[i] == 0xFF && d[i + 1] == 0xFF {
                break;
            }
            if d.len() < i + 16 {
                return err(format!("13TouchFile: truncated record at {i:#x}"));
            }
            let w = |k: usize| be16(d, i + 2 * k);
            let mut r = TouchRecord {
                page_raw: w(0),
                x0: w(1),
                y0: w(2),
                x1: w(3),
                y1: w(4),
                next_page: w(5),
                pressed_page: w(6),
                code: w(7),
                ext: Vec::new(),
            };
            i += 16;
            while d.len() >= i + 16 && d[i] == 0xFE {
                r.ext.push(d[i..i + 16].try_into().unwrap());
                i += 16;
            }
            records.push(r);
        }
        Ok(Self { records })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for r in &self.records {
            r.write(&mut out);
        }
        out.extend_from_slice(&[0xFF, 0xFF]);
        out
    }
}
