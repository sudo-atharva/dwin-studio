//! `*.icl` image libraries (`32.icl` = page backgrounds, `4x.icl` = icons).
//!
//! ```text
//! 0   "DGUS_3" <crc16 LE>        crc = Modbus CRC16 over bytes [8..]
//! 8   <file len - 8 : u32>
//! 12  04 <max id u16> 00
//! 16  (max id + 1) x u32 offset  0 = no icon with that id
//!     entries...
//! ```
//! Entry: `<w u16> <h u16> <scan off u16> <scan len u32>` then `scan off` bytes of
//! trimmed JPEG header and `scan len` bytes of scan data (4-aligned, 8 zero tail).

use crate::{be16, be32, err, FormatError};

pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut c: u16 = 0xFFFF;
    for &b in data {
        c ^= b as u16;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xA001 } else { c >> 1 };
        }
    }
    c
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u16,
    pub height: u16,
    pub scan_off: u16,
    pub scan_len: u32,
    /// `scan_off + scan_len` bytes: JPEG header part then scan data.
    pub body: Vec<u8>,
}

impl Icon {
    fn len(&self) -> usize {
        10 + self.body.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icl {
    pub icons: Vec<Option<Icon>>,
    /// Entry start alignment. Newer DGUS packs entries (4); V7.641 aligns to 0x800.
    pub align: usize,
    /// Pad file to this many bytes (0 = no padding).
    pub pad_to: usize,
}

fn align_up(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

impl Icl {
    pub fn parse(d: &[u8]) -> Result<Self, FormatError> {
        if d.len() < 16 || &d[..6] != b"DGUS_3" {
            return err("icl: bad header");
        }
        let crc = u16::from_le_bytes([d[6], d[7]]);
        if crc != crc16_modbus(&d[8..]) {
            return err("icl: CRC mismatch");
        }
        let max_id = be16(d, 13) as usize;
        let mut icons = Vec::with_capacity(max_id + 1);
        let mut offsets = Vec::new();
        for i in 0..=max_id {
            let off = be32(d, 16 + 4 * i) as usize;
            if off == 0 {
                icons.push(None);
                continue;
            }
            if d.len() < off + 10 {
                return err(format!("icl: icon {i} out of range"));
            }
            let scan_off = be16(d, off + 4);
            let scan_len = be32(d, off + 6);
            let end = off + 10 + scan_off as usize + scan_len as usize;
            if d.len() < end {
                return err(format!("icl: icon {i} body out of range"));
            }
            offsets.push(off);
            icons.push(Some(Icon {
                width: be16(d, off),
                height: be16(d, off + 2),
                scan_off,
                scan_len,
                body: d[off + 10..end].to_vec(),
            }));
        }
        // Entries after the first sit on a fixed alignment; detect it.
        let align = [0x800, 4]
            .into_iter()
            .find(|a| offsets.iter().skip(1).all(|o| o % a == 0))
            .unwrap_or(1);
        let mut icl = Self { icons, align, pad_to: 0 };
        if icl.layout().1 != d.len() {
            icl.pad_to = d.len();
        }
        Ok(icl)
    }

    /// Entry offsets and unpadded file length.
    fn layout(&self) -> (Vec<usize>, usize) {
        let mut pos = 16 + 4 * self.icons.len();
        let mut offs = Vec::new();
        for (k, icon) in self.icons.iter().enumerate() {
            match icon {
                Some(ic) => {
                    if k > 0 && !offs.iter().all(|&o| o == 0) {
                        pos = align_up(pos, self.align);
                    }
                    offs.push(pos);
                    pos += ic.len();
                }
                None => offs.push(0),
            }
        }
        (offs, pos)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let (offs, len) = self.layout();
        let mut out = vec![0u8; len.max(self.pad_to)];
        out[..6].copy_from_slice(b"DGUS_3");
        let total = (out.len() - 8) as u32;
        out[8..12].copy_from_slice(&total.to_be_bytes());
        out[12] = 4;
        out[13..15].copy_from_slice(&((self.icons.len() - 1) as u16).to_be_bytes());
        for (k, (icon, &off)) in self.icons.iter().zip(&offs).enumerate() {
            out[16 + 4 * k..20 + 4 * k].copy_from_slice(&(off as u32).to_be_bytes());
            if let Some(ic) = icon {
                out[off..off + 2].copy_from_slice(&ic.width.to_be_bytes());
                out[off + 2..off + 4].copy_from_slice(&ic.height.to_be_bytes());
                out[off + 4..off + 6].copy_from_slice(&ic.scan_off.to_be_bytes());
                out[off + 6..off + 10].copy_from_slice(&ic.scan_len.to_be_bytes());
                out[off + 10..off + ic.len()].copy_from_slice(&ic.body);
            }
        }
        let crc = crc16_modbus(&out[8..]);
        out[6..8].copy_from_slice(&crc.to_le_bytes());
        out
    }
}
