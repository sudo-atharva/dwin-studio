//! Images <-> ICL icons, matching what DGUS writes:
//! baseline JPEG, 4:2:0, IJG quality 90, standard Huffman tables, one DQT/DHT segment
//! per table, SOS padded so scan data starts 4-byte aligned in the entry, 8 zero tail.

use crate::icl::Icon;
use crate::{be16, err, FormatError};
use image::DynamicImage;
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

pub const QUALITY: u8 = 90;

/// JFIF APP0 exactly as in DGUS output (v1.1, 96 dpi).
const APP0: [u8; 14] = *b"JFIF\0\x01\x01\x01\x00\x60\x00\x60\x00\x00";

/// Header segments up to and including SOS: (marker, payload without length). Returns scan start.
fn segments(j: &[u8]) -> Result<(Vec<(u8, Vec<u8>)>, usize), FormatError> {
    if j.len() < 4 || j[..2] != [0xFF, 0xD8] {
        return err("jpeg: no SOI");
    }
    let mut segs = Vec::new();
    let mut i = 2;
    while i + 4 <= j.len() {
        if j[i] != 0xFF {
            return err(format!("jpeg: expected marker at {i}"));
        }
        let m = j[i + 1];
        let len = be16(j, i + 2) as usize;
        let end = i + 2 + len;
        if end > j.len() {
            return err("jpeg: truncated segment");
        }
        segs.push((m, j[i + 4..end].to_vec()));
        if m == 0xDA {
            return Ok((segs, end));
        }
        i = end;
    }
    err("jpeg: no SOS")
}

/// Split a DQT/DHT payload holding several tables into one payload per table.
fn split_tables(m: u8, p: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < p.len() {
        let n = match m {
            0xDB => 1 + 64 * (1 + (p[i] >> 4) as usize),
            _ => 17 + p[i + 1..i + 17].iter().map(|&c| c as usize).sum::<usize>(),
        };
        out.push(p[i..i + n].to_vec());
        i += n;
    }
    out
}

fn put_seg(out: &mut Vec<u8>, m: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, m]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

/// Repack any baseline JPEG into DGUS ICL entry layout.
pub fn pack_jpeg(j: &[u8], width: u16, height: u16) -> Result<Icon, FormatError> {
    let (segs, scan_start) = segments(j)?;
    let mut h = vec![0xFF, 0xD8];
    let app0 = segs.iter().find(|(m, p)| *m == 0xE0 && p.len() == 14).map_or(&APP0[..], |(_, p)| p);
    put_seg(&mut h, 0xE0, app0);
    for m in [0xDB, 0xC0, 0xC4] {
        for (_, p) in segs.iter().filter(|(sm, _)| *sm == m) {
            if m == 0xC0 {
                put_seg(&mut h, m, p);
            } else {
                for t in split_tables(m, p) {
                    put_seg(&mut h, m, &t);
                }
            }
        }
    }
    if !segs.iter().any(|(m, _)| *m == 0xC0) {
        return err("jpeg: not baseline (no SOF0)");
    }
    let (_, sos) = segs.last().unwrap();
    // Entry = 10-byte icon header + JPEG header; scan data must start 4-aligned.
    let pad = (4 - (10 + h.len() + 4 + sos.len()) % 4) % 4;
    let mut sos = sos.clone();
    sos.resize(sos.len() + pad, 0);
    put_seg(&mut h, 0xDA, &sos);

    let mut scan = j[scan_start..].to_vec();
    scan.extend_from_slice(&[0; 8]);
    scan.resize(scan.len().div_ceil(4) * 4, 0);
    Ok(Icon {
        width,
        height,
        scan_off: h.len() as u16,
        scan_len: scan.len() as u32,
        body: [h, scan].concat(),
    })
}

/// Encode an image as a DGUS icon. Transparency is flattened onto black
/// (DWIN icons use black + "filter" for see-through).
pub fn encode(img: &DynamicImage) -> Result<Icon, FormatError> {
    let rgba = img.to_rgba8();
    let rgb: Vec<u8> = rgba
        .pixels()
        .flat_map(|p| {
            let a = p[3] as u16;
            [0, 1, 2].map(|c| (p[c] as u16 * a / 255) as u8)
        })
        .collect();
    let (w, h) = (rgba.width() as u16, rgba.height() as u16);
    let mut j = Vec::new();
    let mut enc = Encoder::new(&mut j, QUALITY);
    enc.set_sampling_factor(SamplingFactor::F_2_2);
    enc.encode(&rgb, w, h, ColorType::Rgb)
        .map_err(|e| FormatError(format!("jpeg encode: {e}")))?;
    pack_jpeg(&j, w, h)
}

impl Icon {
    /// Standard JPEG (SOS padding and zero tail removed), for preview/decoding.
    pub fn to_jpeg(&self) -> Result<Vec<u8>, FormatError> {
        let (segs, _) = segments(&self.body[..self.scan_off as usize])?;
        let mut out = vec![0xFF, 0xD8];
        for (m, p) in &segs {
            let p = if *m == 0xDA { &p[..1 + 2 * p[0] as usize + 3] } else { &p[..] };
            put_seg(&mut out, *m, p);
        }
        let scan = &self.body[self.scan_off as usize..];
        let end = scan.windows(2).rposition(|w| w == [0xFF, 0xD9]).map_or(scan.len(), |i| i + 2);
        out.extend_from_slice(&scan[..end]);
        Ok(out)
    }

    pub fn decode(&self) -> Result<DynamicImage, FormatError> {
        image::load_from_memory_with_format(&self.to_jpeg()?, image::ImageFormat::Jpeg)
            .map_err(|e| FormatError(format!("jpeg decode: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icl::Icl;
    use image::{Rgb, RgbImage};

    /// Marker sequence of every icon DGUS wrote in the sample projects.
    const DGUS_LAYOUT: &[(u8, usize)] = &[
        (0xE0, 16), (0xDB, 67), (0xDB, 67), (0xC0, 17),
        (0xC4, 31), (0xC4, 181), (0xC4, 31), (0xC4, 181), (0xDA, 15),
    ];

    #[test]
    fn encode_matches_dgus_layout_and_decodes() {
        let img = RgbImage::from_fn(800, 480, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        let icon = encode(&DynamicImage::ImageRgb8(img.clone())).unwrap();

        let (segs, _) = segments(&icon.body[..icon.scan_off as usize]).unwrap();
        let layout: Vec<_> = segs.iter().map(|(m, p)| (*m, p.len() + 2)).collect();
        assert_eq!(layout, DGUS_LAYOUT);
        assert_eq!(icon.scan_off, 626, "same header size as DGUS");
        assert_eq!((10 + icon.scan_off as usize) % 4, 0);
        assert_eq!(icon.scan_len % 4, 0);

        // Through a full ICL file and back.
        let icl = Icl { icons: vec![Some(icon)], align: 4, pad_to: 0 };
        let back = Icl::parse(&icl.to_bytes()).unwrap();
        let out = back.icons[0].as_ref().unwrap().decode().unwrap().to_rgb8();
        assert_eq!(out.dimensions(), (800, 480));
        let p = out.get_pixel(100, 50);
        assert!((p[0] as i32 - 100).abs() < 8 && (p[1] as i32 - 50).abs() < 8, "{p:?}");
    }
}
