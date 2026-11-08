//! Compile a `Project` into a `DWIN_SET` and a C header for the MCU.
//!
//! Page ids: user pages first, then generated ones (keypad, keyboard, dropdown lists).
//! Page k normal image = k, pressed image = n + k (n = total page count) in 32.icl.
//! Icons (toggle states, slider knobs, icon states) are cut from the page and go in 48.icl.
//! Addresses: VP auto-allocated from 0x1000 (0x10 steps), SP from 0x9000 (0x10 steps).

use crate::control::{show_record, Kind, Params, Value, SHOW_KINDS, TOUCH_KINDS};
use crate::icl::{Icl, Icon};
use crate::picture;
use crate::project::{button, Event, Page, Project, Rgb, Widget, WidgetKind};
use crate::render::{gauge_geom, icon, needle_icon, render, text, text_width, to_image, Mode};
use crate::set::DwinSet;
use crate::show::ShowFile;
use crate::touch::{TouchFile, TouchRecord, NONE};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

pub const VP_START: u16 = 0x1000;
pub const SP_START: u16 = 0x9000;
/// Icon library id for generated icons. 48 x 256 KB = 12 MB flash offset;
/// leaves 32.icl (page images, from 8 MB) up to 4 MB.
/// Each library number N sits at flash offset N x 256 KB.
const SECTOR: usize = 256 * 1024;
/// Bit icons need a private work VP each; they live here, below the SP area.
const AUX_START: u16 = 0x8000;

#[derive(Debug, Clone, PartialEq)]
pub struct Alloc {
    pub page: usize,
    pub widget: usize,
    pub name: String,
    pub vp: u16,
    /// Words used at `vp`.
    pub words: u16,
    pub sp: Option<u16>,
    pub what: &'static str,
}

pub struct Output {
    pub set: DwinSet,
    pub header: String,
    /// ui_map.txt contents.
    pub map: String,
    pub allocs: Vec<Alloc>,
    pub warnings: Vec<String>,
}

fn rgb565(c: Rgb) -> u32 {
    ((c[0] as u32 >> 3) << 11) | ((c[1] as u32 >> 2) << 5) | (c[2] as u32 >> 3)
}

fn clamp16(v: i32) -> u32 {
    v.clamp(0, u16::MAX as i32) as u32
}

/// Assign every VP/SP, keeping fixed ones and filling the rest. Reports overlaps.
pub fn allocate(p: &Project) -> (Vec<Alloc>, Vec<String>) {
    let mut fixed: Vec<Alloc> = Vec::new();
    let mut auto: Vec<Alloc> = Vec::new();
    for (pi, page) in p.pages.iter().enumerate() {
        for (wi, w) in page.widgets.iter().enumerate() {
            let Some(vp) = w.kind.vp() else { continue };
            let (words, has_sp, what) = match &w.kind {
                WidgetKind::Number { .. } => (1, true, "number"),
                WidgetKind::Text { len, .. } => (len.div_ceil(2).max(1), true, "text"),
                WidgetKind::Button { .. } => (1, false, "button"),
                WidgetKind::Toggle { bit: Some(_), .. } | WidgetKind::Lamp { bit: Some(_), .. } => (1, false, "bits"),
                WidgetKind::Toggle { .. } => (1, false, "toggle"),
                WidgetKind::Lamp { .. } => (1, false, "lamp"),
                WidgetKind::Slider { .. } => (1, false, "slider"),
                WidgetKind::Icons { .. } => (1, false, "icons"),
                WidgetKind::Dropdown { .. } => (1, false, "dropdown"),
                WidgetKind::List { .. } => (1, false, "list"),
                WidgetKind::Qr { len, .. } => (len.div_ceil(2) + 1, false, "qr"),
                WidgetKind::Progress { .. } => (1, false, "progress"),
                WidgetKind::Gauge { .. } => (1, false, "gauge"),
                WidgetKind::Animation { .. } => (1, false, "animation"),
                WidgetKind::Drawing { words, .. } => ((*words).max(2), false, "drawing"),
                _ => continue,
            };
            let a = Alloc { page: pi, widget: wi, name: w.name.clone(), vp: vp.unwrap_or(0), words, sp: has_sp.then_some(0), what };
            if vp.is_some() { fixed.push(a) } else { auto.push(a) }
        }
    }
    let overlaps = |a: &Alloc, vp: u16, words: u16| a.vp < vp + words && vp < a.vp + a.words;
    let mut next = VP_START;
    for a in &mut auto {
        while fixed.iter().any(|f| overlaps(f, next, a.words)) {
            next += 0x10;
        }
        a.vp = next;
        next += a.words.div_ceil(0x10).max(1) * 0x10;
        fixed.push(a.clone());
    }
    let mut all = fixed;
    all.sort_by_key(|a| (a.page, a.widget));
    let mut sp = SP_START;
    for a in all.iter_mut().filter(|a| a.sp.is_some()) {
        a.sp = Some(sp);
        sp += 0x10;
    }

    let mut warnings = Vec::new();
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            // Several key buttons on one VP is a normal DGUS pattern (one variable, many keys).
            // So is one VP holding many bit flags.
            // And so is showing one value twice (number + icon) at the same address.
            let shared_ok = a.vp == b.vp && (a.words == b.words || (a.what == b.what && ["button", "bits"].contains(&a.what)));
            if !shared_ok && overlaps(a, b.vp, b.words) {
                warnings.push(format!("VP clash {:#06x}: '{}' and '{}'", b.vp, a.name, b.name));
            }
        }
        if a.vp + a.words > SP_START {
            warnings.push(format!("'{}' VP {:#06x} runs into SP area (0x9000+)", a.name, a.vp));
        }
    }
    // +/- buttons must point at something with a value.
    for page in &p.pages {
        for w in &page.widgets {
            if let WidgetKind::Button { event: Event::Adjust { target, .. }, .. } = &w.kind {
                if !all.iter().any(|a| &a.name == target) {
                    warnings.push(format!("'{}' adjusts '{target}', which has no value (pick a Number/Toggle/Slider/Icons)", w.name));
                }
            }
        }
    }
    (all, warnings)
}

fn n(v: u32) -> Value {
    Value::Num(v)
}

fn kind(list: &'static [Kind], name: &str) -> &'static Kind {
    list.iter().find(|k| k.name == name).unwrap()
}

/// Icon library being filled during the build.
struct Icons(Vec<Option<Icon>>);

impl Icons {
    fn add(&mut self, px: &tiny_skia::Pixmap) -> Result<u32, Box<dyn std::error::Error>> {
        self.0.push(Some(picture::encode(&to_image(px))?));
        Ok(self.0.len() as u32 - 1)
    }
}

/// A screen the build adds itself: keypad, keyboard, dropdown list.
/// `codes[i]` = key code sent by widget i (None = not a key).
pub(crate) struct Extra {
    pub(crate) page: Page,
    pub(crate) codes: Vec<Option<u16>>,
}

/// (x, y, w, h)
pub(crate) type Area = (i32, i32, i32, i32);

/// Where the keypad pops up for field `w`: the half of the screen away from it.
pub(crate) fn keypad_pos(w: &Widget, (_, ky, kw, _): Area, sw: i32) -> (i32, i32) {
    ((if w.x + w.w / 2 < sw / 2 { sw - kw - 8 } else { 8 }).max(0), ky)
}

/// Where the keyboard pops up for field `w`: bottom, or top when the field is low.
pub(crate) fn keyboard_pos(w: &Widget, (kx, ky, _, _): Area, sh: i32) -> (i32, i32) {
    (kx, if w.y + w.h / 2 > sh / 2 { 8 } else { ky })
}

fn key_button(label: &str, (x, y, w, h): Area, size: f32, bg: Rgb) -> Widget {
    let mut k = button(label, Event::None);
    if let WidgetKind::Button { size: s, bg: b, pressed_bg, radius, .. } = &mut k {
        (*s, *b, *pressed_bg, *radius) = (size, bg, [37, 99, 235], 8.0);
    }
    let mut wd = Widget::new(k, label, x, y);
    (wd.w, wd.h) = (w, h);
    wd
}

/// Keys on a grid: (label, code, column, row, column span, row span).
type Key<'a> = (&'a str, u16, f32, f32, f32, f32);

fn key_page(name: &str, area: Area, cols: f32, rows: f32, g: i32, keys: &[Key]) -> Extra {
    let (x0, y0, w, h) = area;
    let kw = (w - g) as f32 / cols - g as f32;
    let kh = (h - g) as f32 / rows - g as f32;
    let mut panel = Widget::new(WidgetKind::Panel { color: [30, 41, 59], radius: 12.0 }, "kb", x0, y0);
    (panel.w, panel.h) = (w, h);
    let mut page = Page { name: name.into(), bg: [0; 3], bg_image: None, swipe: (None, None), widgets: vec![panel] };
    let mut codes = vec![None];
    for &(label, code, c, r, cs, rs) in keys {
        let x = x0 + g + (c * (kw + g as f32)) as i32;
        let y = y0 + g + (r * (kh + g as f32)) as i32;
        let (bw, bh) = ((cs * kw + (cs - 1.0) * g as f32) as i32, (rs * kh + (rs - 1.0) * g as f32) as i32);
        let bg = if (0xF0..0x100).contains(&code) { [71, 85, 105] } else { [51, 65, 85] };
        page.widgets.push(key_button(label, (x, y, bw, bh), (kh * 0.45).max(10.0), bg));
        codes.push(Some(code));
    }
    Extra { page, codes }
}

// DGUS keyboard codes: 0xF0 cancel, 0xF1 OK, 0xF2 backspace, 0xF4 caps; others = shifted << 8 | char.
const ESC: u16 = 0xF0;
const OK: u16 = 0xF1;
const BACK: u16 = 0xF2;
const CAPS: u16 = 0xF4;

/// Numeric keypad, centred on its page. Returns the page and the keypad area on it.
pub(crate) fn keypad(sw: i32, sh: i32) -> (Extra, Area) {
    let g = 8;
    let kh = ((sh * 2 / 3 - 5 * g) / 4).max(16);
    let (w, h) = (4 * (kh * 6 / 5) + 5 * g, 4 * kh + 5 * g);
    let area = ((sw - w) / 2, (sh - h) / 2, w, h);
    let c = |ch: char| ch as u16;
    let keys: [Key; 15] = [
        ("7", c('7'), 0., 0., 1., 1.), ("8", c('8'), 1., 0., 1., 1.), ("9", c('9'), 2., 0., 1., 1.), ("<-", BACK, 3., 0., 1., 1.),
        ("4", c('4'), 0., 1., 1., 1.), ("5", c('5'), 1., 1., 1., 1.), ("6", c('6'), 2., 1., 1., 1.), ("Esc", ESC, 3., 1., 1., 1.),
        ("1", c('1'), 0., 2., 1., 1.), ("2", c('2'), 1., 2., 1., 1.), ("3", c('3'), 2., 2., 1., 1.), ("OK", OK, 3., 2., 1., 2.),
        ("-", c('-'), 0., 3., 1., 1.), ("0", c('0'), 1., 3., 1., 1.), (".", c('.'), 2., 3., 1., 1.),
    ];
    (key_page("_keypad", area, 4.0, 4.0, g, &keys), area)
}

/// QWERTY keyboard across the bottom of its page.
pub(crate) fn keyboard(sw: i32, sh: i32) -> (Extra, Area) {
    let g = 6;
    let kw = (sw - 13 * g) / 10;
    let kh = kw.min((sh * 3 / 5 - 6 * g) / 5).max(16);
    let (w, h) = (10 * kw + 11 * g, 5 * kh + 6 * g);
    let area = ((sw - w) / 2, sh - h - g, w, h);
    let mut keys: Vec<(String, u16, f32, f32, f32)> = vec![];
    let rows = [("1234567890", "!@#$%^&*()", 0.0), ("qwertyuiop", "QWERTYUIOP", 0.0), ("asdfghjkl", "ASDFGHJKL", 0.5), ("zxcvbnm", "ZXCVBNM", 1.5)];
    for (r, (lo, hi, off)) in rows.iter().enumerate() {
        for (i, (l, u)) in lo.chars().zip(hi.chars()).enumerate() {
            keys.push((l.to_string(), (u as u16) << 8 | l as u16, off + i as f32, r as f32, 1.0));
        }
    }
    for (l, code, c, r, span) in [("Caps", CAPS, 0.0, 3.0, 1.5), ("<-", BACK, 8.5, 3.0, 1.5), ("Esc", ESC, 0.0, 4.0, 2.0), ("Space", 0x2020, 2.0, 4.0, 6.0), ("OK", OK, 8.0, 4.0, 2.0)] {
        keys.push((l.into(), code, c, r, span));
    }
    let keys: Vec<Key> = keys.iter().map(|(l, code, c, r, s)| (l.as_str(), *code, *c, *r, *s, 1.0)).collect();
    (key_page("_keyboard", area, 10.0, 5.0, g, &keys), area)
}

/// Dropdown list drawn where it pops up: under the box, or above when there is no room.
pub(crate) fn menu(w: &Widget, options: &[String], size: f32, fg: Rgb, bg: Rgb, sh: i32) -> (Extra, Area) {
    let h = w.h * options.len().max(1) as i32;
    let y = if w.y + w.h + h <= sh { w.y + w.h } else { (w.y - h).max(0) };
    let mut page = Page { name: format!("_list_{}", w.name), bg: [0; 3], bg_image: None, swipe: (None, None), widgets: vec![] };
    let mut codes = vec![];
    for (i, o) in options.iter().enumerate() {
        let mut k = key_button(o, (w.x, y + i as i32 * w.h, w.w, w.h), size, bg);
        if let WidgetKind::Button { fg: f, radius, .. } = &mut k.kind {
            (*f, *radius) = (fg, 0.0);
        }
        page.widgets.push(k);
        codes.push(Some(i as u16));
    }
    (Extra { page, codes }, (w.x, y, w.w, h))
}

/// Touch rectangle of a widget, clipped to the screen (inclusive corners).
fn corners(w: &Widget, sw: u16, sh: u16) -> (u16, u16, u16, u16) {
    let x1 = clamp16(w.x + w.w).min(sw as u32 - 1) as u16;
    let y1 = clamp16(w.y + w.h).min(sh as u32 - 1) as u16;
    (clamp16(w.x) as u16, clamp16(w.y) as u16, x1, y1)
}

/// DGUS cursor colour index: 1 (white) for light text on a dark field, else 0 (black).
fn cursor(c: Rgb) -> u32 {
    (c.iter().map(|&v| v as u32).sum::<u32>() > 384) as u32
}

/// `base` = folder that relative image paths are resolved against (the project file's folder).
pub fn build(p: &Project, base: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    let (allocs, mut warnings) = allocate(p);
    let find = |pi: usize, wi: usize| allocs.iter().find(|a| a.page == pi && a.widget == wi);
    let by_name = |name: &str| allocs.iter().find(|a| a.name == name);
    let npages = p.pages.len();
    let (sw, sh) = (p.width as i32, p.height as i32);
    let ilib = p.icon_lib as u32;
    crate::render::set_font(p.font.as_ref().map(|f| base.join(f)).as_deref());

    // Generated screens go after the user's.
    let mut extras: Vec<Extra> = vec![];
    let uses = |f: &dyn Fn(&WidgetKind) -> bool| p.pages.iter().flat_map(|pg| &pg.widgets).any(|w| f(&w.kind));
    let mut add = |(e, area): (Extra, Area)| {
        extras.push(e);
        (npages + extras.len() - 1, area)
    };
    let kpad = uses(&|k| matches!(k, WidgetKind::Number { input: Some(_), .. })).then(|| add(keypad(sw, sh)));
    let kbd = uses(&|k| matches!(k, WidgetKind::Text { input: true, .. })).then(|| add(keyboard(sw, sh)));
    let mut menus = BTreeMap::new();
    for (pi, page) in p.pages.iter().enumerate() {
        for (wi, w) in page.widgets.iter().enumerate() {
            if let WidgetKind::Dropdown { options, size, fg, bg, .. } = &w.kind {
                menus.insert((pi, wi), add(menu(w, options, *size, *fg, *bg, sh)));
            }
        }
    }
    let total = npages + extras.len();

    let mut pages = vec![None; 2 * total];
    let mut icons = Icons(vec![]);
    let mut touch = Vec::new();
    let mut show = ShowFile { flag: 0x10, max_page: (2 * total - 1) as u16, pages: BTreeMap::new() };
    let mut config = vec![0u8; 131076];
    let mut aux = AUX_START;
    let mut unicode_fonts: BTreeMap<usize, u32> = BTreeMap::new(); // text size -> font library

    for (pi, page) in p.pages.iter().enumerate() {
        let has_buttons = page.widgets.iter().any(|w| matches!(w.kind, WidgetKind::Button { .. }));
        pages[pi] = Some(picture::encode(&to_image(&render(page, p.width, p.height, Mode::Normal, base)))?);
        if has_buttons {
            pages[total + pi] = Some(picture::encode(&to_image(&render(page, p.width, p.height, Mode::Pressed, base)))?);
        }
        let base_px = render(page, p.width, p.height, Mode::Base, base);
        let shows = show.pages.entry(pi as u16).or_default();

        for (wi, w) in page.widgets.iter().enumerate() {
            let (cx0, cy0, cx1, cy1) = corners(w, p.width, p.height);
            let (x0, y0, x1, y1) = (cx0 as u32, cy0 as u32, cx1 as u32, cy1 as u32);
            let mut rec = TouchRecord {
                page_raw: pi as u16, x0: cx0, y0: cy0, x1: cx1, y1: cy1, next_page: NONE, pressed_page: NONE, code: 0, ext: vec![],
            };
            let a = find(pi, wi);
            let vp = || n(a.unwrap().vp as u32);
            match &w.kind {
                WidgetKind::Button { goto, event, .. } => {
                    rec.pressed_page = (total + pi) as u16;
                    match goto {
                        Some(t) if *t < npages => rec.next_page = *t as u16,
                        Some(t) => warnings.push(format!("'{}': page {t} does not exist", w.name)),
                        None => {}
                    }
                    match event {
                        Event::None => {}
                        Event::Key { key, .. } => {
                            let params: Params = vec![("vp", vp()), ("key", n(*key as u32))];
                            rec.set_params(kind(TOUCH_KINDS, "return_key"), &params);
                        }
                        Event::Adjust { target, step, min, max, wrap, repeat } => {
                            let Some(t) = by_name(target) else { touch.push(rec); continue };
                            let params: Params = vec![
                                ("vp", n(t.vp as u32)), ("adj_mode", n((*step > 0) as u32)), ("return_mode", n(*wrap as u32)),
                                ("step", n(step.unsigned_abs() as u32)), ("min", n(*min as u32)), ("max", n(*max as u32)),
                                ("key_mode", n(*repeat as u32)),
                            ];
                            rec.set_params(kind(TOUCH_KINDS, "increment"), &params);
                        }
                    }
                    touch.push(rec);
                }
                WidgetKind::Number { int_digits, dec_digits, color, input, smooth, .. } => {
                    // ASCII font: size = character width, height is twice that.
                    let font = (w.h / 2).clamp(1, 255) as u32;
                    let params: Params = vec![
                        ("sp", n(a.unwrap().sp.unwrap() as u32)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("font_size", n(font)),
                        ("int_digits", n(*int_digits as u32)), ("dec_digits", n(*dec_digits as u32)),
                    ];
                    if *smooth {
                        // Digits pre-rendered with the editor font: '0'..'9', '-', '.' in one row of icons.
                        let size = w.h as f32;
                        let cw = "0123456789-.".chars().map(|c| text_width(&c.to_string(), size).ceil() as i32).max().unwrap_or(1);
                        let first = icons.0.len() as u32;
                        for c in "0123456789-.".chars() {
                            // Background cut from under the first digit.
                            let mut px = icon(&base_px, w, 0, (cw, w.h), base);
                            let s = c.to_string();
                            text(&mut px, &s, (cw as f32 - text_width(&s, size)) / 2.0, 0.0, size, *color);
                            icons.add(&px)?;
                        }
                        let params: Params = vec![
                            ("sp", n(a.unwrap().sp.unwrap() as u32)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)),
                            ("icon0", n(first)), ("lib", n(ilib)),
                            ("int_digits", n(*int_digits as u32)), ("dec_digits", n(*dec_digits as u32)),
                        ];
                        shows.push(show_record(kind(SHOW_KINDS, "art_number"), &params));
                    } else {
                        shows.push(show_record(kind(SHOW_KINDS, "number"), &params));
                    }
                    if let (Some((min, max)), Some((kp, (kx, ky, kw, kh)))) = (input, kpad) {
                        let (pos_x, pos_y) = keypad_pos(w, (kx, ky, kw, kh), sw);
                        let params: Params = vec![
                            ("vp", vp()), ("int_digits", n(*int_digits as u32)), ("dec_digits", n(*dec_digits as u32)),
                            ("show_x", n(x0)), ("show_y", n(y0)), ("color", n(rgb565(*color))), ("font", n(font)),
                            ("cursor_color", n(cursor(*color))), ("kb_source", n(1)), ("kb_page", n(kp as u32)),
                            ("kb_x0", n(kx as u32)), ("kb_y0", n(ky as u32)), ("kb_x1", n((kx + kw - 1) as u32)),
                            ("kb_y1", n((ky + kh - 1) as u32)), ("kb_pos_x", n(pos_x as u32)), ("kb_pos_y", n(pos_y as u32)),
                            ("limits_en", n(1)), ("min", n(*min as u32)), ("max", n(*max as u32)),
                        ];
                        rec.set_params(kind(TOUCH_KINDS, "var_input"), &params);
                        touch.push(rec);
                    }
                }
                WidgetKind::Text { len, color, input, scroll, unicode, .. } => {
                    let size = w.h.clamp(0, 255) as u32;
                    let params: Params = vec![
                        ("sp", n(a.unwrap().sp.unwrap() as u32)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("box_x0", n(x0)), ("box_y0", n(y0)), ("box_x1", n(x1)),
                        ("box_y1", n(y1)), ("length", n(*len as u32)), ("font_w", n(size)), ("font_h", n(size)),
                        ("encode", n(0x80)), // ASCII, auto character spacing
                    ];
                    let mut params = params;
                    if *unicode {
                        // UNICODE (0x05) + auto spacing; font = the generated .dzk of this size.
                        let next = p.unicode_lib as u32 + unicode_fonts.len() as u32;
                        let lib = unicode_fonts.entry(size as usize).or_insert(next);
                        params.retain(|(name, _)| *name != "encode");
                        params.extend([("encode", n(0x85)), ("font0", n(*lib)), ("font1", n(*lib))]);
                    }
                    if *scroll {
                        let params: Params = vec![
                            ("sp", n(a.unwrap().sp.unwrap() as u32)), ("vp", vp()), ("step", n(1)), ("color", n(rgb565(*color))),
                            ("x0", n(x0)), ("y0", n(y0)), ("x1", n(x1)), ("y1", n(y1)), ("font_w", n(size)), ("font_h", n(size)),
                            ("speed", n(2)),
                        ];
                        shows.push(show_record(kind(SHOW_KINDS, "scroll_text"), &params));
                    } else {
                        shows.push(show_record(kind(SHOW_KINDS, "text"), &params));
                    }
                    if let (true, Some((kp, (kx, ky, kw, kh)))) = (*input, kbd) {
                        let (pos_x, pos_y) = keyboard_pos(w, (kx, ky, kw, kh), sh);
                        let params: Params = vec![
                            ("vp", vp()), ("max_len", n((*len).min(255) as u32)), ("font_x", n(size)), ("font_y", n(size)),
                            ("cursor_color", n(cursor(*color))), ("color", n(rgb565(*color))),
                            ("scan_x0", n(x0)), ("scan_y0", n(y0)), ("scan_x1", n(x1)), ("scan_y1", n(y1)),
                            ("kb_source", n(1)), ("kb_page", n(kp as u32)), ("kb_x0", n(kx as u32)), ("kb_y0", n(ky as u32)),
                            ("kb_x1", n((kx + kw - 1) as u32)), ("kb_y1", n((ky + kh - 1) as u32)),
                            ("kb_pos_x", n(pos_x as u32)), ("kb_pos_y", n(pos_y as u32)),
                        ];
                        rec.set_params(kind(TOUCH_KINDS, "text_input"), &params);
                        touch.push(rec);
                    }
                }
                WidgetKind::Toggle { .. }
                | WidgetKind::Lamp { .. }
                | WidgetKind::Icons { .. }
                | WidgetKind::Dropdown { .. }
                | WidgetKind::Animation { .. }
                | WidgetKind::List { .. } => {
                    let states = match &w.kind {
                        WidgetKind::List { items, .. } => items.len() + 1,
                        WidgetKind::Icons { images, .. } | WidgetKind::Animation { images, .. } => images.len().max(1),
                        WidgetKind::Dropdown { options, .. } => options.len().max(1),
                        _ => 2,
                    } as u32;
                    let first = icons.0.len() as u32;
                    for s in 0..states {
                        icons.add(&icon(&base_px, w, s as usize, (w.w, w.h), base))?;
                    }
                    let last = first + states - 1;
                    if let WidgetKind::Animation { period_ms, autoplay, .. } = &w.kind {
                        // VP 1 = play, 0 = stop on the first frame.
                        let params: Params = vec![
                            ("sp", n(0xFFFF)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)), ("v_stop", n(0)), ("v_start", n(1)),
                            ("icon_stop", n(first)), ("icon_start", n(first)), ("icon_end", n(last)),
                            ("lib", n(ilib)), ("period", n((*period_ms / 8).clamp(1, 255) as u32)),
                        ];
                        shows.push(show_record(kind(SHOW_KINDS, "animation"), &params));
                        if *autoplay {
                            let i = a.unwrap().vp as usize * 2;
                            config[i..i + 2].copy_from_slice(&1u16.to_be_bytes());
                        }
                        continue;
                    }
                    let bit = match &w.kind {
                        WidgetKind::Toggle { bit, .. } | WidgetKind::Lamp { bit, .. } => *bit,
                        _ => None,
                    };
                    if let Some(b) = bit {
                        // One bit of the VP picks icon `first` (0) or `last` (1).
                        let params: Params = vec![
                            ("sp", n(0xFFFF)), ("vp", vp()), ("vp_aux", n(aux as u32)), ("bits", n(1 << (b % 16))),
                            ("lib", n(ilib)), ("icon0_start", n(first)), ("icon0_end", n(first)),
                            ("icon1_start", n(last)), ("icon1_end", n(last)), ("x", n(x0)), ("y", n(y0)),
                        ];
                        aux += 2;
                        shows.push(show_record(kind(SHOW_KINDS, "bit_icon"), &params));
                        if matches!(w.kind, WidgetKind::Toggle { .. }) {
                            let params: Params = vec![("vp", vp()), ("bit", n((b % 16) as u32)), ("adj_mode", n(2))];
                            rec.set_params(kind(TOUCH_KINDS, "bit_button"), &params);
                            touch.push(rec);
                        }
                        continue;
                    }
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)),
                        ("v_min", n(0)), ("v_max", n(states - 1)), ("icon_min", n(first)),
                        ("icon_max", n(last)), ("lib", n(ilib)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "var_icon"), &params));
                    match &w.kind {
                        WidgetKind::Toggle { .. } => {
                            // Same as DGUS demo 2: increment, step 1, 0..1, wrap.
                            let params: Params = vec![
                                ("vp", vp()), ("adj_mode", n(1)), ("return_mode", n(1)),
                                ("step", n(1)), ("min", n(0)), ("max", n(1)),
                            ];
                            rec.set_params(kind(TOUCH_KINDS, "increment"), &params);
                            touch.push(rec);
                        }
                        WidgetKind::List { items, .. } => {
                            // One key per row: row i sends i + 1.
                            let rh = w.h / items.len().max(1) as i32;
                            for i in 0..items.len() as i32 {
                                let mut row = w.clone();
                                (row.y, row.h) = (w.y + i * rh, rh);
                                let (x0, y0, x1, y1) = corners(&row, p.width, p.height);
                                let mut r = TouchRecord { page_raw: pi as u16, x0, y0, x1, y1, next_page: NONE, pressed_page: NONE, code: 0, ext: vec![] };
                                r.set_params(kind(TOUCH_KINDS, "return_key"), &vec![("vp", vp()), ("key", n(i as u32 + 1))]);
                                touch.push(r);
                            }
                        }
                        WidgetKind::Dropdown { .. } => {
                            let (mp, (mx, my, mw, mh)) = menus[&(pi, wi)];
                            let params: Params = vec![
                                ("vp", vp()), ("menu_page", n(mp as u32)), ("menu_x0", n(mx as u32)), ("menu_y0", n(my as u32)),
                                ("menu_x1", n((mx + mw - 1) as u32)), ("menu_y1", n((my + mh - 1) as u32)),
                                ("pos_x", n(mx as u32)), ("pos_y", n(my as u32)),
                            ];
                            rec.set_params(kind(TOUCH_KINDS, "popup_menu"), &params);
                            touch.push(rec);
                        }
                        _ => {}
                    }
                }
                WidgetKind::Slider { min, max, .. } => {
                    let d = w.h;
                    // Knob background is cut from the middle of the track, where it looks like everywhere else.
                    let mut mid = w.clone();
                    (mid.x, mid.w) = (w.x + (w.w - d) / 2, d);
                    let knob = icons.add(&icon(&base_px, &mid, 0, (d, d), base))?;
                    let (kx0, kx1) = (x0, clamp16(w.x + w.w - d));
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("v_begin", n(*min as u32)), ("v_end", n(*max as u32)),
                        ("x_begin", n(kx0)), ("x_end", n(kx1)), ("icon", n(knob)), ("y", n(y0)),
                        ("lib", n(ilib)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "slider_show"), &params));
                    // Touch: value follows the knob centre between the track ends.
                    let (tx0, tx1) = (clamp16(w.x + d / 2), clamp16(w.x + w.w - d / 2));
                    let params: Params = vec![
                        ("vp", vp()), ("adj_mode", n(0)), ("x0", n(tx0)), ("y0", n(y0)), ("x1", n(tx1)),
                        ("y1", n(y1)), ("v_begin", n(*min as u32)), ("v_end", n(*max as u32)),
                    ];
                    rec.set_params(kind(TOUCH_KINDS, "slider"), &params);
                    touch.push(rec);
                }
                WidgetKind::Drawing { .. } => {
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("x0", n(x0)), ("y0", n(y0)), ("x1", n(x1)), ("y1", n(y1)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "graphics"), &params));
                }
                WidgetKind::Clock { format, color } => {
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("x", n(x0)), ("y", n(y0)), ("color", n(rgb565(*color))),
                        ("font_size", n((w.h / 2).clamp(1, 255) as u32)), ("format", Value::Text(format.clone())),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "rtc_text"), &params));
                }
                WidgetKind::Qr { dot, .. } => {
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)), ("dot", n((*dot).max(1) as u32)),
                        ("bg_color", n(0xFFFF)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "qr_code"), &params));
                }
                WidgetKind::Progress { min, max, fg, bg, border, vertical, .. } => {
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("x0", n(x0)), ("y0", n(y0)), ("x1", n(x1)), ("y1", n(y1)),
                        ("border_color", n(rgb565(*border))), ("fore_color", n(rgb565(*fg))), ("back_color", n(rgb565(*bg))),
                        ("v_max", n(*max as u32)), ("v_min", n(*min as u32)), ("orientation", n(*vertical as u32)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "progress_bar"), &params));
                }
                WidgetKind::Chart { channel, min, max, step, color, .. } => {
                    let span = (*max as i32 - *min as i32).max(1);
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("move_dir", n(1)), ("x0", n(x0)), ("y0", n(y0)), ("x1", n(x1)), ("y1", n(y1)),
                        ("y_center", n(y0 + w.h as u32 / 2)), ("v_center", n(((*min as i32 + *max as i32) / 2) as u16 as u32)),
                        ("color", n(rgb565(*color))), ("mul_y", n((256 * w.h / span).clamp(1, 0xFFFF) as u32)),
                        ("channel", n((*channel).min(7) as u32)), ("step", n((*step).max(1) as u32)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "curve"), &params));
                }
                WidgetKind::Gauge { min, max, sweep, needle, knob, .. } => {
                    let (cx, cy, r, start) = gauge_geom(w, *sweep);
                    let (px, icx, icy) = needle_icon(r, *needle);
                    let id = icons.add(&px)?;
                    // DGUS angles: 0.5 degree units, 0..720.
                    let half = |deg: f32| n((deg * 2.0).round() as u32 % 720);
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("icon", n(id)), ("icon_cx", n(icx)), ("icon_cy", n(icy)),
                        ("cx", n(cx as u32)), ("cy", n(cy as u32)), ("v_begin", n(*min as u32)), ("v_end", n(*max as u32)),
                        ("angle_begin", half(start)), ("angle_end", half(start + *sweep as f32)),
                        ("lib", n(ilib)), ("mode", n(1)), // 1 = black is transparent, as in DGUS demo 4
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "icon_rotate"), &params));
                    if *knob {
                        // Centre given relative to the touch area's centre (0,0), as in DGUS demo 5.
                        let params: Params = vec![
                            ("vp", vp()), ("r0", n((r * 0.3) as u32)), ("r1", n(r as u32)), ("a0", half(start)),
                            ("v_begin", n(*min as u32)), ("a1", half(start + *sweep as f32)), ("v_end", n(*max as u32)),
                        ];
                        rec.set_params(kind(TOUCH_KINDS, "rotate_adjust"), &params);
                        touch.push(rec);
                    }
                }
                WidgetKind::Label { .. } | WidgetKind::Panel { .. } | WidgetKind::Image { .. } => {}
            }
        }
        // Swipe goes last so buttons keep their taps.
        if let (None, None) = page.swipe {
            continue;
        }
        let target = |t: Option<usize>| n(t.filter(|&t| t < npages).unwrap_or(pi) as u32);
        let mut rec = TouchRecord {
            page_raw: pi as u16, x0: 0, y0: 0, x1: p.width - 1, y1: p.height - 1, next_page: NONE, pressed_page: NONE, code: 0, ext: vec![],
        };
        let params: Params = vec![
            ("prev", target(page.swipe.0)), ("next", target(page.swipe.1)), ("range0", n(0)), ("range1", n(p.height as u32 - 1)),
            ("speed", n(25)), ("dist", n(50)),
        ];
        rec.set_params(kind(TOUCH_KINDS, "swipe_page"), &params);
        touch.push(rec);
    }

    for (k, e) in extras.iter().enumerate() {
        let id = npages + k;
        pages[id] = Some(picture::encode(&to_image(&render(&e.page, p.width, p.height, Mode::Normal, base)))?);
        pages[total + id] = Some(picture::encode(&to_image(&render(&e.page, p.width, p.height, Mode::Pressed, base)))?);
        for (w, code) in e.page.widgets.iter().zip(&e.codes) {
            let Some(code) = code else { continue };
            let (x0, y0, x1, y1) = corners(w, p.width, p.height);
            touch.push(TouchRecord {
                page_raw: id as u16, x0, y0, x1, y1, next_page: NONE, pressed_page: (total + id) as u16, code: *code, ext: vec![],
            });
        }
    }
    show.pages.retain(|_, v| !v.is_empty());

    let mut set = DwinSet { touch: Some(TouchFile { records: touch }), show: Some(show), icls: BTreeMap::new(), raw: BTreeMap::new() };
    // Plain JPEGs too, like DGUS puts in DWIN_SET (NN_name.jpg).
    for (id, img) in pages.iter().enumerate() {
        if let Some(img) = img {
            let name = match id {
                i if i < npages => &p.pages[i].name,
                i if i < total => &extras[i - npages].page.name,
                _ => "pressed",
            };
            set.raw.insert(format!("{id:02}_{}.jpg", sanitize(name).to_lowercase()), img.to_jpeg()?);
        }
    }
    if p.page_lib == p.icon_lib {
        warnings.push(format!("page and icon library are both {}.icl", p.page_lib));
    }
    set.icls.insert(format!("{}.icl", p.page_lib), Icl { icons: pages, align: 4, pad_to: 0 });
    if !icons.0.is_empty() {
        set.icls.insert(format!("{ilib}.icl"), Icl { icons: icons.0, align: 4, pad_to: 0 });
    }
    set.raw.insert("22_Config.bin".into(), config);
    // Font 0 drawn with the editor font: numbers/text look like the preview.
    set.raw.insert("0_DWIN_ASC.HZK".into(), crate::render::ascii_hzk());
    for (size, lib) in &unicode_fonts {
        set.raw.insert(format!("{lib}_u{size}.dzk"), crate::render::unicode_dzk(*size, p.unicode_max));
    }
    if let Some(cfg) = &p.cfg {
        match std::fs::read(base.join(cfg)) {
            Ok(b) => drop(set.raw.insert("T5LCFG.CFG".into(), b)),
            Err(e) => warnings.push(format!("T5LCFG.CFG not copied ({cfg}): {e}")),
        }
    }
    warnings.extend(flash_overlaps(&set));

    let header = header(p, &allocs);
    let mut map = ui_map(p, &allocs);
    if !extras.is_empty() {
        writeln!(map, "\n=== Generated pages (the screen opens them itself) ===").unwrap();
        for (k, e) in extras.iter().enumerate() {
            writeln!(map, "Page {}: {}", npages + k, e.page.name).unwrap();
        }
    }
    Ok(Output { set, header, map, allocs, warnings })
}

/// Files land in flash at (leading number) x 256 KB; report any two that overlap.
fn flash_overlaps(set: &DwinSet) -> Vec<String> {
    let mut spans: Vec<(String, usize, usize)> = set
        .files()
        .into_iter()
        .filter_map(|(name, bytes)| {
            let id: usize = name.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()?;
            // Plain page JPEGs (NN_name.jpg) are copies for people, not loaded by the screen.
            (!name.ends_with(".jpg")).then(|| (name, id * SECTOR, id * SECTOR + bytes.len()))
        })
        .collect();
    spans.sort_by_key(|s| s.1);
    spans
        .windows(2)
        .filter(|w| w[0].2 > w[1].1)
        .map(|w| format!("{} ({} KB) runs into {} in the screen's flash: pick a higher number for {}", w[0].0, w[0].2.div_ceil(1024) - w[0].1 / 1024, w[1].0, w[1].0))
        .collect()
}

pub fn sanitize(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn header(p: &Project, allocs: &[Alloc]) -> String {
    let mut h = String::new();
    writeln!(h, "// Generated by dwin-studio. Do not edit; rebuild instead.\n#pragma once\n").unwrap();
    for (i, page) in p.pages.iter().enumerate() {
        writeln!(h, "#define PAGE_{:<24} {i}", sanitize(&page.name)).unwrap();
    }
    for a in allocs {
        let w = &p.pages[a.page].widgets[a.widget];
        let name = sanitize(&a.name);
        writeln!(h).unwrap();
        if !w.comment.is_empty() {
            writeln!(h, "// {}", w.comment.replace('\n', " ")).unwrap();
        }
        writeln!(h, "#define VP_{name:<26} {:#06x}  // {}, {} word(s)", a.vp, a.what, a.words).unwrap();
        if let Some(sp) = a.sp {
            writeln!(h, "#define SP_{name:<26} {sp:#06x}  // move/recolor at runtime").unwrap();
        }
        if let WidgetKind::Button { event: Event::Key { key, .. }, .. } = &w.kind {
            writeln!(h, "#define KEY_{name:<25} {key:#06x}").unwrap();
        }
    }
    h
}

fn describe(p: &Project, w: &Widget, a: Option<&Alloc>) -> Option<String> {
    let vp = || a.map_or(0, |a| a.vp);
    Some(match &w.kind {
        WidgetKind::Button { text, goto, event, .. } => {
            let mut parts = vec![format!("BUTTON  \"{text}\"")];
            match event {
                Event::None => {}
                Event::Key { key, .. } => parts.push(format!("sends key {key:#06x} to VP {:#06x}", vp())),
                Event::Adjust { target, step, min, max, wrap, .. } => parts.push(format!(
                    "adds {step:+} to '{target}' ({min}..{max}{})",
                    if *wrap { ", wraps" } else { "" }
                )),
            }
            if let Some(g) = goto {
                let name = p.pages.get(*g).map_or("?", |pg| pg.name.as_str());
                parts.push(format!("goes to page {g} ({name})"));
            }
            if matches!(event, Event::None) && goto.is_none() {
                parts.push("no action".into());
            }
            parts.join(", ")
        }
        WidgetKind::Number { int_digits, dec_digits, input, .. } => format!(
            "NUMBER  VP {:#06x} (int16, {int_digits}.{dec_digits} digits), SP {:#06x}{}",
            vp(),
            a.and_then(|a| a.sp).unwrap_or(0),
            input.map_or(String::new(), |(lo, hi)| format!("; tap opens keypad ({lo}..{hi}), screen sends the value on OK"))
        ),
        WidgetKind::Text { len, input, unicode, .. } => format!(
            "TEXT    VP {:#06x} ({len} bytes {}), SP {:#06x}{}",
            vp(),
            if *unicode { "UTF-16BE: 2 bytes per character, end with FF FF" } else { "ASCII" },
            a.and_then(|a| a.sp).unwrap_or(0),
            if *input { "; tap opens keyboard, screen sends the text on OK" } else { "" }
        ),
        WidgetKind::Toggle { bit: Some(b), .. } => format!("TOGGLE  VP {:#06x} bit {b}; tap flips the bit and sends the VP", vp()),
        WidgetKind::Toggle { .. } => format!("TOGGLE  VP {:#06x} = 0 off / 1 on; tap flips it and sends it", vp()),
        WidgetKind::Lamp { bit: Some(b), .. } => format!("LAMP    VP {:#06x} bit {b}: 1 = on", vp()),
        WidgetKind::Lamp { .. } => format!("LAMP    VP {:#06x}: 0 off / 1 on", vp()),
        WidgetKind::Slider { min, max, .. } => format!("SLIDER  VP {:#06x} = {min}..{max}; drag sends it, MCU write moves knob", vp()),
        WidgetKind::Icons { images, .. } => format!("ICONS   VP {:#06x} = 0..{} picks the picture", vp(), images.len().max(1) - 1),
        WidgetKind::List { items, .. } => {
            let list: Vec<_> = items.iter().enumerate().map(|(i, o)| format!("{}={o}", i + 1)).collect();
            format!("LIST    VP {:#06x}: 0 = none, {}; tap sends it (pair with an OK button)", vp(), list.join(", "))
        }
        WidgetKind::Dropdown { options, .. } => {
            let list: Vec<_> = options.iter().enumerate().map(|(i, o)| format!("{i}={o}")).collect();
            format!("DROPDOWN VP {:#06x}: {}; picking sends it", vp(), list.join(", "))
        }
        WidgetKind::Clock { format, .. } => {
            format!("CLOCK   \"{format}\"; set time: 5A A5 0B 82 00 9C 5A A5 <YY MM DD hh mm ss> (check your T5L manual)")
        }
        WidgetKind::Qr { len, .. } => format!("QR      VP {:#06x}: write up to {len} bytes of text, then FF FF", vp()),
        WidgetKind::Progress { min, max, .. } => format!("PROGRESS VP {:#06x} = {min}..{max}", vp()),
        WidgetKind::Chart { channel, min, max, .. } => format!(
            "CHART   channel {channel}, {min}..{max}; add samples: 5A A5 <len> 82 03 10 5A A5 01 00 {channel:02X} <count> <int16 samples> (check your T5L manual)"
        ),
        WidgetKind::Drawing { words, .. } => format!(
            "DRAWING VP {:#06x} ({words} words): MCU writes basic-graphics commands here (T5L manual: 0x0002 lines, 0x0003 boxes, 0x0004 filled boxes...), 0x0000 ends",
            vp()
        ),
        WidgetKind::Gauge { min, max, knob, .. } => format!(
            "GAUGE   VP {:#06x} = {min}..{max} turns the needle{}",
            vp(),
            if *knob { "; dragging around it sets the value and sends it" } else { "" }
        ),
        WidgetKind::Animation { autoplay, .. } => format!(
            "ANIMATION VP {:#06x}: 1 = play, 0 = stop (boots {})",
            vp(),
            if *autoplay { "playing" } else { "stopped" }
        ),
        WidgetKind::Label { .. } | WidgetKind::Panel { .. } | WidgetKind::Image { .. } => return None,
    })
}

/// Human-readable list of everything on every page: what the MCU sees and what each item does.
pub fn ui_map(p: &Project, allocs: &[Alloc]) -> String {
    let mut t = String::new();
    writeln!(t, "DWIN Studio UI map - {} x {}, {} page(s)", p.width, p.height, p.pages.len()).unwrap();
    writeln!(t, "Screen sends on touch: 5A A5 06 83 <VP hi> <VP lo> 01 <value hi> <value lo>").unwrap();
    writeln!(t, "MCU sets a value:      5A A5 05 82 <VP hi> <VP lo> <value hi> <value lo>").unwrap();
    writeln!(t, "MCU switches page:     5A A5 07 82 00 84 5A 01 <page hi> <page lo>").unwrap();
    for (pi, page) in p.pages.iter().enumerate() {
        writeln!(t, "\n=== Page {pi}: {} ===", page.name).unwrap();
        let mut any = false;
        for (wi, w) in page.widgets.iter().enumerate() {
            let a = allocs.iter().find(|a| a.page == pi && a.widget == wi);
            let Some(what) = describe(p, w, a) else { continue };
            any = true;
            writeln!(t, "{:<16} {what}", w.name).unwrap();
            writeln!(t, "{:<16} area ({},{})-({},{})", "", w.x, w.y, w.x + w.w, w.y + w.h).unwrap();
            if !w.comment.is_empty() {
                writeln!(t, "{:<16} // {}", "", w.comment).unwrap();
            }
        }
        if !any {
            writeln!(t, "(no interactive or runtime items)").unwrap();
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::show_params;
    use crate::project::{button, default_widgets, Page};

    fn palette(name: &str) -> WidgetKind {
        default_widgets().into_iter().find(|(n, _)| *n == name).unwrap().1
    }

    fn sample() -> Project {
        let mut goto = button("Settings", Event::None);
        if let WidgetKind::Button { goto: g, .. } = &mut goto {
            *g = Some(1);
        }
        let plus = button("+", Event::Adjust { target: "temp".into(), step: 1, min: 0, max: 100, wrap: false, repeat: true });
        Project {
            width: 1024,
            height: 600,
            page_lib: 32,
            icon_lib: 48,
            font: None,
            cfg: None,
            unicode_lib: 24,
            unicode_max: 0x052F,
            pages: vec![
                Page { name: "home".into(), bg: [10, 20, 30], bg_image: None, swipe: (None, None), widgets: vec![
                    Widget::new(palette("Button"), "start", 20, 20),
                    Widget::new(goto, "settings", 20, 100),
                    Widget::new(palette("Number"), "temp", 300, 20),
                    Widget::new(palette("Text"), "status", 300, 100),
                    Widget::new(palette("Toggle"), "gas", 300, 200),
                    Widget::new(palette("Slider"), "current", 300, 300),
                    Widget::new(plus, "temp_up", 500, 20),
                ]},
                Page { name: "settings".into(), bg: [0, 0, 0], bg_image: None, swipe: (None, None), widgets: vec![] },
            ],
        }
    }

    #[test]
    fn build_parses_back() {
        let out = build(&sample(), Path::new(".")).unwrap();
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let dir = std::env::temp_dir().join("dwin_studio_build_test");
        let _ = std::fs::remove_dir_all(&dir);
        out.set.write(&dir).unwrap();
        let back = DwinSet::load(&dir).unwrap();

        let icl = &back.icls["32.icl"];
        assert_eq!(icl.icons.iter().flatten().count(), 3, "2 pages + 1 pressed");
        assert_eq!((icl.icons[0].as_ref().unwrap().width, icl.icons[0].as_ref().unwrap().height), (1024, 600));
        assert_eq!(back.icls["48.icl"].icons.len(), 3, "toggle off/on + slider knob");

        let t = &back.touch.unwrap().records;
        let kinds: Vec<_> = t.iter().map(|r| r.kind().map_or("basic", |k| k.name)).collect();
        assert_eq!(kinds, ["return_key", "basic", "increment", "slider", "increment"]);
        let key = t[0].params().unwrap();
        assert!(key.contains(&("vp", Value::Num(0x1000))) && key.contains(&("key", Value::Num(1))));
        assert_eq!((t[0].pressed_page, t[1].next_page), (2, 1));
        let plus = t[4].params().unwrap();
        assert!(plus.contains(&("vp", Value::Num(0x1010))), "+ button targets temp's VP: {plus:?}");

        let recs = &back.show.unwrap().pages[&0];
        let num = show_params(&recs[0]).unwrap();
        assert!(num.contains(&("vp", Value::Num(0x1010))) && num.contains(&("sp", Value::Num(0x9000))));
        assert!(out.header.contains("#define VP_TEMP") && out.header.contains("#define PAGE_SETTINGS"));
        assert!(out.map.contains("sends key 0x0001 to VP 0x1000") && out.map.contains("goes to page 1"), "{}", out.map);
    }

    #[test]
    fn inputs_menus_and_displays() {
        let mut p = sample();
        let ws = &mut p.pages[1].widgets;
        for (i, name) in ["Number Input", "Text Input", "Dropdown", "Clock", "QR Code", "Progress", "Chart", "Gauge"].iter().enumerate() {
            ws.push(Widget::new(palette(name), &format!("w{i}"), 20, 20 + 50 * i as i32));
        }
        let out = build(&p, Path::new(".")).unwrap();
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        // 2 user pages + keypad + keyboard + 1 dropdown list = 5; pressed images follow.
        assert_eq!(out.set.icls["32.icl"].icons.len(), 10);
        let t = &out.set.touch.as_ref().unwrap().records;
        let kinds: Vec<_> = t.iter().filter(|r| r.page() == 1).map(|r| r.kind().map_or("basic", |k| k.name)).collect();
        assert_eq!(kinds, ["var_input", "text_input", "popup_menu"]);
        let kb_page = |name: &str| t.iter().find(|r| r.kind().is_some_and(|k| k.name == name)).unwrap().params().unwrap();
        assert!(kb_page("var_input").contains(&("kb_page", Value::Num(2))));
        assert!(kb_page("text_input").contains(&("kb_page", Value::Num(3))));
        assert!(kb_page("popup_menu").contains(&("menu_page", Value::Num(4))));
        // Keypad: 15 keys incl. OK (0xF1); keyboard: 'q' key sends Q/q.
        assert_eq!(t.iter().filter(|r| r.page() == 2).count(), 15);
        assert!(t.iter().any(|r| r.page() == 2 && r.code == 0x00F1));
        assert!(t.iter().any(|r| r.page() == 3 && r.code == 0x5171));
        assert_eq!(t.iter().filter(|r| r.page() == 4).count(), 3, "3 dropdown options");
        let shows: Vec<_> = out.set.show.as_ref().unwrap().pages[&1].iter().map(|r| crate::control::show_kind(r).unwrap().name).collect();
        assert_eq!(shows, ["number", "text", "var_icon", "rtc_text", "qr_code", "progress_bar", "curve", "icon_rotate"]);
        assert!(out.map.contains("DROPDOWN") && out.map.contains("_keypad"), "{}", out.map);
    }

    #[test]
    fn bits_swipe_scroll_smooth() {
        let mut p = sample();
        p.pages[1].swipe = (Some(0), None);
        let mut t = palette("Toggle");
        if let WidgetKind::Toggle { bit, vp, .. } = &mut t {
            (*bit, *vp) = (Some(3), Some(0x2000));
        }
        let mut lamp = palette("Lamp");
        if let WidgetKind::Lamp { bit, vp, .. } = &mut lamp {
            (*bit, *vp) = (Some(4), Some(0x2000));
        }
        let ws = &mut p.pages[1].widgets;
        ws.push(Widget::new(t, "flag3", 10, 10));
        ws.push(Widget::new(lamp, "flag4", 100, 10));
        ws.push(Widget::new(palette("Scrolling Text"), "ticker", 10, 100));
        ws.push(Widget::new(palette("Big Number"), "big", 10, 200));
        let out = build(&p, Path::new(".")).unwrap();
        assert!(out.warnings.is_empty(), "shared bit VP is fine: {:?}", out.warnings);
        let t = &out.set.touch.as_ref().unwrap().records;
        let kinds: Vec<_> = t.iter().filter(|r| r.page() == 1).map(|r| r.kind().map_or("basic", |k| k.name)).collect();
        assert_eq!(kinds, ["bit_button", "swipe_page"]);
        let bb = t.iter().find(|r| r.page() == 1).unwrap();
        assert_eq!(bb.code, 0xFE0D);
        assert!(bb.params().unwrap().contains(&("bit", Value::Num(3))));
        let shows: Vec<_> = out.set.show.as_ref().unwrap().pages[&1].iter().map(|r| crate::control::show_kind(r).unwrap().name).collect();
        assert_eq!(shows, ["bit_icon", "bit_icon", "scroll_text", "art_number"]);
        // toggle(2) + slider(1) on home, then 2 + 2 bit icons, then 12 digits.
        assert_eq!(out.set.icls["48.icl"].icons.len(), 3 + 4 + 12);
        let hzk = &out.set.raw["0_DWIN_ASC.HZK"];
        assert_eq!(hzk.len(), 3_082_752, "same size as DWIN's own font file");
        // Size 16 'A': 32 rows x 2 bytes, some ink, nothing for a space.
        let at = |size: usize, c: usize| (4..size).map(|i| 128 * 2 * i * i.div_ceil(8)).sum::<usize>() + c * 2 * size * size.div_ceil(8);
        assert!(hzk[at(16, 65)..at(16, 66)].iter().any(|&b| b != 0));
        assert!(hzk[at(16, 32)..at(16, 33)].iter().all(|&b| b == 0));
    }

    #[test]
    fn list_and_libraries() {
        let mut p = sample();
        p.pages[1].widgets.push(Widget::new(palette("List"), "programs", 10, 10));
        (p.page_lib, p.icon_lib) = (23, 42);
        let out = build(&p, Path::new(".")).unwrap();
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert!(out.set.icls.contains_key("23.icl") && out.set.icls.contains_key("42.icl"));
        let rows: Vec<_> = out.set.touch.as_ref().unwrap().records.iter().filter(|r| r.page() == 1).map(|r| r.params().unwrap()).collect();
        assert_eq!(rows.len(), 4);
        assert!(rows[3].contains(&("key", Value::Num(4))));
        // Unicode text: own .dzk of its size from library 24, records point at it.
        let mut p2 = sample();
        p2.pages[1].widgets.push(Widget::new(palette("Unicode Text"), "greet", 10, 300));
        let out2 = build(&p2, Path::new(".")).unwrap();
        let dzk = &out2.set.raw["24_u32.dzk"];
        assert_eq!(dzk.len(), 4 * 32 * (0x052F + 1));
        let cell = |cp: usize| &dzk[cp * 128..(cp + 1) * 128];
        assert!(cell(0x416).iter().any(|&b| b != 0), "Cyrillic glyph drawn");
        let rec = out2.set.show.as_ref().unwrap().pages[&1].iter().find(|r| crate::control::show_kind(r).unwrap().name == "text").unwrap();
        let tp = crate::control::show_params(rec).unwrap();
        assert!(tp.contains(&("encode", Value::Num(0x85))) && tp.contains(&("font1", Value::Num(24))));
        // The 3 MB ASCII font fills slots 0..11, so pages at 5 would be overwritten.
        p.page_lib = 5;
        let out = build(&p, Path::new(".")).unwrap();
        assert!(out.warnings.iter().any(|w| w.contains("0_DWIN_ASC.HZK") && w.contains("5.icl")), "{:?}", out.warnings);
    }

    #[test]
    fn clash_and_bad_target_reported() {
        let mut p = sample();
        *p.pages[0].widgets[2].kind.vp_mut().unwrap() = Some(0x2000);
        *p.pages[0].widgets[3].kind.vp_mut().unwrap() = Some(0x2000);
        if let WidgetKind::Button { event: Event::Adjust { target, .. }, .. } = &mut p.pages[0].widgets[6].kind {
            *target = "nothing".into();
        }
        let (_, w) = allocate(&p);
        assert_eq!(w.len(), 2, "{w:?}");
    }
}
