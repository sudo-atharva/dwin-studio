//! Typed view of touch (13) and display (14) records.
//!
//! Each control type is a table of named fields at fixed byte offsets, taken from the
//! DGUS V7.6 per-control `GetByte()` / `GetData()` layouts. One table drives decode,
//! encode and (later) the editor property panel.
//!
//! Touch offsets are absolute in the record (base 0..16, extension lines 16..);
//! display offsets are absolute in the 32-byte record.

use crate::show::ShowRecord;
use crate::touch::TouchRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Num(u32),
    /// Raw bytes shown as Latin-1 text, trailing zeros trimmed.
    Text(String),
}

pub type Params = Vec<(&'static str, Value)>;

/// `size` 1/2/4 = big-endian number, larger = text bytes.
#[derive(Debug)]
pub struct Field {
    pub name: &'static str,
    pub off: usize,
    pub size: usize,
}

#[derive(Debug)]
pub struct Kind {
    /// Touch: low byte of FExx code. Display: type byte after 0x5A.
    pub code: u8,
    pub name: &'static str,
    /// Touch: total record bytes (16/32/48/64). Display: description length in words.
    pub size: usize,
    pub fields: &'static [Field],
}

macro_rules! fields {
    ($($name:ident @ $off:literal : $size:literal),* $(,)?) => {
        &[$(Field { name: stringify!($name), off: $off, size: $size }),*]
    };
}

pub static TOUCH_KINDS: &[Kind] = &[
    Kind { code: 0x00, name: "var_input", size: 64, fields: fields![
        vp@17:2, var_type@19:1, int_digits@20:1, dec_digits@21:1, show_x@22:2, show_y@24:2,
        color@26:2, lib@28:1, font@29:1, cursor_color@30:1, hide@31:1,
        kb_source@33:1, kb_page@34:2, kb_x0@36:2, kb_y0@38:2, kb_x1@40:2, kb_y1@42:2,
        kb_pos_x@44:2, kb_pos_y@46:2,
        limits_en@49:1, min@50:4, max@54:4, return_set@58:1, return_vp@59:2, return_data@61:2,
        gamma@63:1,
    ]},
    Kind { code: 0x01, name: "popup_menu", size: 48, fields: fields![
        vp@17:2, vp_mode@19:1, menu_page@20:2, menu_x0@22:2, menu_y0@24:2, menu_x1@26:2,
        menu_y1@28:2, pos_x@30:2, pos_y@33:2, gamma@35:1,
    ]},
    Kind { code: 0x02, name: "increment", size: 32, fields: fields![
        vp@17:2, vp_mode@19:1, adj_mode@20:1, return_mode@21:1, step@22:2, min@24:2, max@26:2,
        key_mode@28:1, key_delay@29:1,
    ]},
    Kind { code: 0x03, name: "slider", size: 32, fields: fields![
        vp@17:2, adj_mode@19:1, x0@20:2, y0@22:2, x1@24:2, y1@26:2, v_begin@28:2, v_end@30:2,
    ]},
    Kind { code: 0x05, name: "return_key", size: 32, fields: fields![
        vp@17:2, vp_mode@19:1, key@20:2, hold@22:1,
    ]},
    Kind { code: 0x06, name: "text_input", size: 64, fields: fields![
        vp@17:2, max_len@19:1, scan_mode@20:1, lib@21:1, font_x@22:1, font_y@23:1,
        cursor_color@24:1, color@25:2, scan_x0@27:2, scan_y0@29:2, return_mode@31:1,
        scan_x1@33:2, scan_y1@35:2, kb_source@37:1, kb_page@38:2, kb_x0@40:2, kb_y0@42:2,
        kb_x1@44:2, kb_y1@46:2,
        kb_pos_x@49:2, kb_pos_y@51:2, display_en@53:1, gamma@54:1,
    ]},
    // Three VP-copy actions: on press, while held, on release.
    Kind { code: 0x08, name: "key_copy", size: 48, fields: fields![
        press_en@17:1, press_src@18:2, press_dst@20:2, press_len@22:2,
        hold_en@25:1, hold_src@26:2, hold_dst@28:2, hold_len@30:2,
        release_en@33:1, release_src@34:2, release_dst@36:2, release_len@38:2,
    ]},
    Kind { code: 0x09, name: "rotate_adjust", size: 48, fields: fields![
        vp@17:2, data_format@19:1, cx@20:2, cy@22:2, r0@24:2, r1@26:2, a0@28:2, v_begin@30:2,
        a1@33:2, v_end@35:2,
    ]},
    Kind { code: 0x0D, name: "bit_button", size: 32, fields: fields![
        vp@17:2, bit@19:1, adj_mode@20:1,
    ]},
];

pub static SHOW_KINDS: &[Kind] = &[
    Kind { code: 0x00, name: "var_icon", size: 10, fields: fields![
        sp@2:2, vp@6:2, x@8:2, y@10:2, v_min@12:2, v_max@14:2, icon_min@16:2, icon_max@18:2,
        lib@20:1, mode@21:1, back_mode@22:1, icon_gamma@23:1, back_gamma@24:1, filter@25:1,
        group@26:1,
    ]},
    Kind { code: 0x01, name: "animation", size: 13, fields: fields![
        sp@2:2, vp@6:2, x@8:2, y@10:2, restart@12:2, v_stop@14:2, v_start@16:2, icon_stop@18:2,
        icon_start@20:2, icon_end@22:2, lib@24:1, mode@25:1, back_mode@26:1, icon_gamma@27:1,
        back_gamma@28:1, period@29:1, show_mode@30:1, filter@31:1,
    ]},
    Kind { code: 0x02, name: "slider_show", size: 12, fields: fields![
        sp@2:2, vp@6:2, v_begin@8:2, v_end@10:2, x_begin@12:2, x_end@14:2, icon@16:2, y@18:2,
        x_adj@20:1, mode@21:1, lib@22:1, icon_mode@23:1, vp_mode@24:1, back_mode@25:1,
        icon_gamma@26:1, back_gamma@27:1, filter@28:1,
    ]},
    Kind { code: 0x05, name: "icon_rotate", size: 12, fields: fields![
        sp@2:2, vp@6:2, icon@8:2, icon_cx@10:2, icon_cy@12:2, cx@14:2, cy@16:2, v_begin@18:2,
        v_end@20:2, angle_begin@22:2, angle_end@24:2, vp_mode@26:1, lib@27:1, mode@28:1,
        filter@29:1,
    ]},
    Kind { code: 0x06, name: "bit_icon", size: 13, fields: fields![
        sp@2:2, vp@6:2, vp_aux@8:2, bits@10:2, display_mode@12:1, move_mode@13:1, icon_mode@14:1,
        lib@15:1, icon0_start@16:2, icon0_end@18:2, icon1_start@20:2, icon1_end@22:2, x@24:2,
        y@26:2, move_dist@28:2, filter@30:1,
    ]},
    Kind { code: 0x10, name: "number", size: 13, fields: fields![
        sp@2:2, vp@6:2, x@8:2, y@10:2, color@12:2, lib@14:1, font_size@15:1, align@16:1,
        int_digits@17:1, dec_digits@18:1, var_type@19:1, unit_len@20:1, unit@21:11,
    ]},
    Kind { code: 0x11, name: "text", size: 13, fields: fields![
        sp@2:2, vp@6:2, x@8:2, y@10:2, color@12:2, box_x0@14:2, box_y0@16:2, box_x1@18:2,
        box_y1@20:2, length@22:2, font0@24:1, font1@25:1, font_w@26:1, font_h@27:1,
        encode@28:1, hor_gap@29:1, ver_gap@30:1,
    ]},
];

fn decode(kind: &Kind, b: &[u8]) -> Params {
    kind.fields
        .iter()
        .map(|f| {
            let s = &b[f.off..f.off + f.size];
            let v = if f.size <= 4 {
                Value::Num(s.iter().fold(0, |a, &x| a << 8 | x as u32))
            } else {
                let end = s.iter().rposition(|&x| x != 0).map_or(0, |i| i + 1);
                Value::Text(s[..end].iter().map(|&x| x as char).collect())
            };
            (f.name, v)
        })
        .collect()
}

/// Write `params` into `b`. Unknown names are ignored; missing fields keep their bytes.
fn encode(kind: &Kind, params: &Params, b: &mut [u8]) {
    for f in kind.fields {
        let Some((_, v)) = params.iter().find(|(n, _)| *n == f.name) else { continue };
        let s = &mut b[f.off..f.off + f.size];
        match v {
            Value::Num(n) => {
                for (i, x) in s.iter_mut().rev().enumerate() {
                    *x = (*n >> (8 * i)) as u8;
                }
            }
            Value::Text(t) => {
                s.fill(0);
                for (x, c) in s.iter_mut().zip(t.chars()) {
                    *x = c as u32 as u8;
                }
            }
        }
    }
}

impl TouchRecord {
    /// Control kind for FExx / FDxx codes (FD = trigger on press instead of release).
    pub fn kind(&self) -> Option<&'static Kind> {
        if !matches!(self.code >> 8, 0xFE | 0xFD) {
            return None;
        }
        TOUCH_KINDS
            .iter()
            .find(|k| k.code == self.code as u8 && k.size == 16 * (1 + self.ext.len()))
    }

    fn full_bytes(&self) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        for line in &self.ext {
            b.extend_from_slice(line);
        }
        b
    }

    pub fn params(&self) -> Option<Params> {
        Some(decode(self.kind()?, &self.full_bytes()))
    }

    /// Build a fresh extended record of `kind` from params (base fields set separately).
    pub fn set_params(&mut self, kind: &'static Kind, params: &Params) {
        let mut b = vec![0u8; kind.size];
        for line in (16..kind.size).step_by(16) {
            b[line] = 0xFE;
        }
        if kind.code == 0x08 {
            b[24] = 0xFE; // key_copy: 8-byte sub-records, extra marker mid-line
        }
        encode(kind, params, &mut b);
        if self.code >> 8 != 0xFD {
            self.code = 0xFE00;
        }
        self.code = self.code & 0xFF00 | kind.code as u16;
        self.ext = b[16..].chunks(16).map(|c| c.try_into().unwrap()).collect();
    }
}

pub fn show_kind(r: &ShowRecord) -> Option<&'static Kind> {
    if r[0] != 0x5A {
        return None;
    }
    SHOW_KINDS.iter().find(|k| k.code == r[1])
}

pub fn show_params(r: &ShowRecord) -> Option<Params> {
    Some(decode(show_kind(r)?, r))
}

pub fn show_record(kind: &'static Kind, params: &Params) -> ShowRecord {
    let mut r = [0u8; 32];
    r[0] = 0x5A;
    r[1] = kind.code;
    r[4..6].copy_from_slice(&(kind.size as u16).to_be_bytes());
    encode(kind, params, &mut r);
    r
}
