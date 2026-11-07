//! `14ShowFile.bin`: display variables (number, text, icons, ...).
//!
//! ```text
//! 0x0000  14 "DGUS_2" <flag> <max page id u16> 00..   16-byte header
//! 0x0010  4092 x [count u8, offset u24]               page table; empty pages carry running offset
//! 0x4000  32-byte records grouped by ascending page    each: 5A <type> <SP u16> <len u16> <desc...>
//!         32 x FF                                      terminator
//! ```

use crate::{be16, be32, err, FormatError};
use std::collections::BTreeMap;

const MAGIC: &[u8; 7] = b"\x14DGUS_2";
const PAGES: usize = 4092;
const DATA_START: usize = 0x4000;

pub type ShowRecord = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowFile {
    /// Header byte 7. 0x10 in every DGUS V7.6 sample.
    pub flag: u8,
    /// Highest page id in the project (pages may have zero records).
    pub max_page: u16,
    /// Records per page id. Pages without records may be absent.
    pub pages: BTreeMap<u16, Vec<ShowRecord>>,
}

impl ShowFile {
    pub fn parse(d: &[u8]) -> Result<Self, FormatError> {
        if d.len() < DATA_START + 32 || &d[..7] != MAGIC {
            return err("14ShowFile: bad header");
        }
        let mut pages = BTreeMap::new();
        for p in 0..PAGES {
            let e = 16 + 4 * p;
            let count = d[e] as usize;
            if count == 0 {
                continue;
            }
            let off = (be32(d, e) & 0x00FF_FFFF) as usize;
            if d.len() < off + 32 * count {
                return err(format!("14ShowFile: page {p} records out of range"));
            }
            let recs = (0..count)
                .map(|k| d[off + 32 * k..off + 32 * (k + 1)].try_into().unwrap())
                .collect();
            pages.insert(p as u16, recs);
        }
        Ok(Self {
            flag: d[7],
            max_page: be16(d, 8),
            pages,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; DATA_START];
        out[..7].copy_from_slice(MAGIC);
        out[7] = self.flag;
        out[8..10].copy_from_slice(&self.max_page.to_be_bytes());
        let mut off = DATA_START as u32;
        for p in 0..PAGES {
            let n = self.pages.get(&(p as u16)).map_or(0, Vec::len);
            let e = 16 + 4 * p;
            out[e..e + 4].copy_from_slice(&off.to_be_bytes());
            out[e] = n as u8;
            off += 32 * n as u32;
        }
        for recs in self.pages.values() {
            for r in recs {
                out.extend_from_slice(r);
            }
        }
        out.extend_from_slice(&[0xFF; 32]);
        out
    }
}
