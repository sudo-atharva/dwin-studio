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
use crate::render::{gauge_geom, icon, needle_icon, render, to_image, Mode};
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
pub const ICON_LIB: u8 = 48;
const PAGE_LIB_MAX: usize = (ICON_LIB as usize - 32) * 256 * 1024;

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
                WidgetKind::Toggle { .. } => (1, false, "toggle"),
                WidgetKind::Slider { .. } => (1, false, "slider"),
                WidgetKind::Icons { .. } => (1, false, "icons"),
                WidgetKind::Dropdown { .. } => (1, false, "dropdown"),
                WidgetKind::Qr { len, .. } => (len.div_ceil(2) + 1, false, "qr"),
                WidgetKind::Progress { .. } => (1, false, "progress"),
                WidgetKind::Gauge { .. } => (1, false, "gauge"),
                WidgetKind::Animation { .. } => (1, false, "animation"),
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
            let shared_ok = a.what == "button" && b.what == "button" && a.vp == b.vp;
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
struct Extra {
    page: Page,
    codes: Vec<Option<u16>>,
}

/// (x, y, w, h)
type Area = (i32, i32, i32, i32);

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
    let mut page = Page { name: name.into(), bg: [0; 3], bg_image: None, widgets: vec![panel] };
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
fn keypad(sw: i32, sh: i32) -> (Extra, Area) {
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
fn keyboard(sw: i32, sh: i32) -> (Extra, Area) {
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
fn menu(w: &Widget, options: &[String], size: f32, fg: Rgb, bg: Rgb, sh: i32) -> (Extra, Area) {
    let h = w.h * options.len().max(1) as i32;
    let y = if w.y + w.h + h <= sh { w.y + w.h } else { (w.y - h).max(0) };
    let mut page = Page { name: format!("_list_{}", w.name), bg: [0; 3], bg_image: None, widgets: vec![] };
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
                WidgetKind::Number { int_digits, dec_digits, color, input, .. } => {
                    // ASCII font: size = character width, height is twice that.
                    let font = (w.h / 2).clamp(1, 255) as u32;
                    let params: Params = vec![
                        ("sp", n(a.unwrap().sp.unwrap() as u32)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("font_size", n(font)),
                        ("int_digits", n(*int_digits as u32)), ("dec_digits", n(*dec_digits as u32)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "number"), &params));
                    if let (Some((min, max)), Some((kp, (kx, ky, kw, kh)))) = (input, kpad) {
                        // Keypad pops up on the half of the screen away from the field.
                        let pos_x = if w.x + w.w / 2 < sw / 2 { sw - kw - 8 } else { 8 };
                        let params: Params = vec![
                            ("vp", vp()), ("int_digits", n(*int_digits as u32)), ("dec_digits", n(*dec_digits as u32)),
                            ("show_x", n(x0)), ("show_y", n(y0)), ("color", n(rgb565(*color))), ("font", n(font)),
                            ("cursor_color", n(cursor(*color))), ("kb_source", n(1)), ("kb_page", n(kp as u32)),
                            ("kb_x0", n(kx as u32)), ("kb_y0", n(ky as u32)), ("kb_x1", n((kx + kw - 1) as u32)),
                            ("kb_y1", n((ky + kh - 1) as u32)), ("kb_pos_x", n(pos_x.max(0) as u32)), ("kb_pos_y", n(ky as u32)),
                            ("limits_en", n(1)), ("min", n(*min as u32)), ("max", n(*max as u32)),
                        ];
                        rec.set_params(kind(TOUCH_KINDS, "var_input"), &params);
                        touch.push(rec);
                    }
                }
                WidgetKind::Text { len, color, input, .. } => {
                    let size = w.h.clamp(0, 255) as u32;
                    let params: Params = vec![
                        ("sp", n(a.unwrap().sp.unwrap() as u32)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("box_x0", n(x0)), ("box_y0", n(y0)), ("box_x1", n(x1)),
                        ("box_y1", n(y1)), ("length", n(*len as u32)), ("font_w", n(size)), ("font_h", n(size)),
                        ("encode", n(0x80)), // ASCII, auto character spacing
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "text"), &params));
                    if let (true, Some((kp, (kx, ky, kw, kh)))) = (*input, kbd) {
                        // Keyboard sits at the bottom unless the field is there; then at the top.
                        let pos_y = if w.y + w.h / 2 > sh / 2 { 8 } else { ky };
                        let params: Params = vec![
                            ("vp", vp()), ("max_len", n((*len).min(255) as u32)), ("font_x", n(size)), ("font_y", n(size)),
                            ("cursor_color", n(cursor(*color))), ("color", n(rgb565(*color))),
                            ("scan_x0", n(x0)), ("scan_y0", n(y0)), ("scan_x1", n(x1)), ("scan_y1", n(y1)),
                            ("kb_source", n(1)), ("kb_page", n(kp as u32)), ("kb_x0", n(kx as u32)), ("kb_y0", n(ky as u32)),
                            ("kb_x1", n((kx + kw - 1) as u32)), ("kb_y1", n((ky + kh - 1) as u32)),
                            ("kb_pos_x", n(kx as u32)), ("kb_pos_y", n(pos_y as u32)),
                        ];
                        rec.set_params(kind(TOUCH_KINDS, "text_input"), &params);
                        touch.push(rec);
                    }
                }
                WidgetKind::Toggle { .. } | WidgetKind::Icons { .. } | WidgetKind::Dropdown { .. } | WidgetKind::Animation { .. } => {
                    let states = match &w.kind {
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
                            ("lib", n(ICON_LIB as u32)), ("period", n((*period_ms / 8).clamp(1, 255) as u32)),
                        ];
                        shows.push(show_record(kind(SHOW_KINDS, "animation"), &params));
                        if *autoplay {
                            let i = a.unwrap().vp as usize * 2;
                            config[i..i + 2].copy_from_slice(&1u16.to_be_bytes());
                        }
                        continue;
                    }
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("x", n(x0)), ("y", n(y0)),
                        ("v_min", n(0)), ("v_max", n(states - 1)), ("icon_min", n(first)),
                        ("icon_max", n(last)), ("lib", n(ICON_LIB as u32)),
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
                        ("lib", n(ICON_LIB as u32)),
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
                WidgetKind::Gauge { min, max, sweep, needle, .. } => {
                    let (cx, cy, r, start) = gauge_geom(w, *sweep);
                    let (px, icx, icy) = needle_icon(r, *needle);
                    let id = icons.add(&px)?;
                    // DGUS angles: 0.5 degree units, 0..720.
                    let half = |deg: f32| n((deg * 2.0).round() as u32 % 720);
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", vp()), ("icon", n(id)), ("icon_cx", n(icx)), ("icon_cy", n(icy)),
                        ("cx", n(cx as u32)), ("cy", n(cy as u32)), ("v_begin", n(*min as u32)), ("v_end", n(*max as u32)),
                        ("angle_begin", half(start)), ("angle_end", half(start + *sweep as f32)),
                        ("lib", n(ICON_LIB as u32)), ("mode", n(1)), // 1 = black is transparent, as in DGUS demo 4
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "icon_rotate"), &params));
                }
                WidgetKind::Label { .. } | WidgetKind::Panel { .. } | WidgetKind::Image { .. } => {}
            }
        }
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
    let page_icl = Icl { icons: pages, align: 4, pad_to: 0 };
    if page_icl.to_bytes().len() > PAGE_LIB_MAX {
        warnings.push(format!("page images exceed {} MB and would overlap {ICON_LIB}.icl", PAGE_LIB_MAX >> 20));
    }
    set.icls.insert("32.icl".into(), page_icl);
    if !icons.0.is_empty() {
        set.icls.insert(format!("{ICON_LIB}.icl"), Icl { icons: icons.0, align: 4, pad_to: 0 });
    }
    set.raw.insert("22_Config.bin".into(), config);

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
        WidgetKind::Text { len, input, .. } => format!(
            "TEXT    VP {:#06x} ({len} bytes ASCII), SP {:#06x}{}",
            vp(),
            a.and_then(|a| a.sp).unwrap_or(0),
            if *input { "; tap opens keyboard, screen sends the text on OK" } else { "" }
        ),
        WidgetKind::Toggle { .. } => format!("TOGGLE  VP {:#06x} = 0 off / 1 on; tap flips it and sends it", vp()),
        WidgetKind::Slider { min, max, .. } => format!("SLIDER  VP {:#06x} = {min}..{max}; drag sends it, MCU write moves knob", vp()),
        WidgetKind::Icons { images, .. } => format!("ICONS   VP {:#06x} = 0..{} picks the picture", vp(), images.len().max(1) - 1),
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
        WidgetKind::Gauge { min, max, .. } => format!("GAUGE   VP {:#06x} = {min}..{max} turns the needle", vp()),
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
            pages: vec![
                Page { name: "home".into(), bg: [10, 20, 30], bg_image: None, widgets: vec![
                    Widget::new(palette("Button"), "start", 20, 20),
                    Widget::new(goto, "settings", 20, 100),
                    Widget::new(palette("Number"), "temp", 300, 20),
                    Widget::new(palette("Text"), "status", 300, 100),
                    Widget::new(palette("Toggle"), "gas", 300, 200),
                    Widget::new(palette("Slider"), "current", 300, 300),
                    Widget::new(plus, "temp_up", 500, 20),
                ]},
                Page { name: "settings".into(), bg: [0, 0, 0], bg_image: None, widgets: vec![] },
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
