//! Draw a page to pixels. Same code for the editor canvas and the images that go on the screen.

use crate::project::{Page, Rgb, Widget, WidgetKind};
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use image::RgbaImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// Page image as the screen stores it: buttons up, dynamic widgets in state 0.
    Normal,
    /// Every button drawn pressed. DWIN copies button regions from this image on touch.
    Pressed,
    /// Editor view: normal + placeholders for runtime numbers/text.
    Preview,
    /// Static content only (no toggles/sliders knobs/icons): background that icons are cut from.
    Base,
}

/// Font file in use (path, bytes). None = bundled Ubuntu Light.
static FONT: Mutex<Option<(PathBuf, &'static [u8])>> = Mutex::new(None);

fn font() -> FontRef<'static> {
    let bytes = FONT.lock().unwrap().as_ref().map_or(epaint_default_fonts::UBUNTU_LIGHT, |f| f.1);
    FontRef::try_from_slice(bytes).expect("checked in set_font")
}

/// Switch the drawing font (preview, page images, HZK). Returns true if it changed.
/// Unreadable or invalid files fall back to the bundled font.
/// ponytail: font bytes are leaked once per distinct font chosen; fine for an editor session.
pub fn set_font(path: Option<&Path>) -> bool {
    let mut cur = FONT.lock().unwrap();
    if cur.as_ref().map(|f| f.0.as_path()) == path {
        return false;
    }
    let loaded = path.and_then(|p| {
        let bytes = std::fs::read(p).ok()?;
        FontRef::try_from_slice(&bytes).ok()?;
        Some((p.to_path_buf(), &*Box::leak(bytes.into_boxed_slice())))
    });
    let changed = cur.as_ref().map(|f| &f.0) != loaded.as_ref().map(|f| &f.0);
    *cur = loaded;
    changed
}

/// Decoded images by path. ponytail: never invalidated; restart editor after editing an asset file.
pub fn load_image(base: &Path, rel: &str) -> Option<Arc<RgbaImage>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<RgbaImage>>>>> = OnceLock::new();
    let path = base.join(rel);
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache
        .entry(path.clone())
        .or_insert_with(|| image::open(&path).ok().map(|i| Arc::new(i.to_rgba8())))
        .clone()
}

pub(crate) fn paint(c: Rgb) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], 255));
    p.anti_alias = true;
    p
}

pub(crate) fn rounded(px: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), r: f32, c: Rgb) {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let path = if r == 0.0 {
        PathBuilder::from_rect(Rect::from_xywh(x, y, w, h).unwrap())
    } else {
        let k = 0.5523 * r; // cubic circle approximation
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        pb.close();
        pb.finish().unwrap()
    };
    px.fill_path(&path, &paint(c), FillRule::Winding, Transform::identity(), None);
}

pub(crate) fn frect(w: &Widget) -> (f32, f32, f32, f32) {
    (w.x as f32, w.y as f32, w.w as f32, w.h as f32)
}

pub fn text_width(text: &str, size: f32) -> f32 {
    let f = font();
    let sf = f.as_scaled(PxScale::from(size));
    text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum()
}

/// Blend one pixel into an opaque pixmap.
fn blend(data: &mut [u8], pw: i32, ph: i32, x: i32, y: i32, c: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= pw || y >= ph {
        return;
    }
    let i = 4 * (y * pw + x) as usize;
    for k in 0..3 {
        let d = data[i + k] as f32;
        data[i + k] = (d + (c[k] as f32 - d) * a.clamp(0.0, 1.0)) as u8;
    }
}

/// Draw `text` with its top-left at (x, y).
pub(crate) fn text(px: &mut Pixmap, s: &str, x: f32, y: f32, size: f32, c: Rgb) {
    let f = font();
    let sf = f.as_scaled(PxScale::from(size));
    let (pw, ph) = (px.width() as i32, px.height() as i32);
    let data = px.data_mut();
    let mut caret = x;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        let g = id.with_scale_and_position(size, point(caret, y + sf.ascent()));
        caret += sf.h_advance(id);
        let Some(o) = f.outline_glyph(g) else { continue };
        let b = o.px_bounds();
        o.draw(|gx, gy, cov| blend(data, pw, ph, b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, c, cov));
    }
}

fn centered(px: &mut Pixmap, r: (f32, f32, f32, f32), s: &str, size: f32, c: Rgb) {
    let tx = r.0 + (r.2 - text_width(s, size)) / 2.0;
    let ty = r.1 + (r.3 - size) / 2.0;
    text(px, s, tx, ty, size, c);
}

/// Stretch an image into a rect, alpha-blended. `dim` < 1 darkens (pressed state).
pub(crate) fn picture(px: &mut Pixmap, img: &RgbaImage, (x, y, w, h): (i32, i32, i32, i32), dim: f32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let scaled = image::imageops::resize(img, w as u32, h as u32, image::imageops::FilterType::Triangle);
    let (pw, ph) = (px.width() as i32, px.height() as i32);
    let data = px.data_mut();
    for (sx, sy, p) in scaled.enumerate_pixels() {
        let c = [0, 1, 2].map(|k| (p[k] as f32 * dim) as u8);
        blend(data, pw, ph, x + sx as i32, y + sy as i32, c, p[3] as f32 / 255.0);
    }
}

/// Polyline with round caps.
pub(crate) fn line(px: &mut Pixmap, pts: &[(f32, f32)], width: f32, c: Rgb) {
    let mut pb = PathBuilder::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    let Some(path) = pb.finish() else { return };
    let stroke = Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
    px.stroke_path(&path, &paint(c), &stroke, Transform::identity(), None);
}

/// Point at `deg` clockwise from 12 o'clock (DGUS rotation convention).
pub(crate) fn polar(cx: f32, cy: f32, r: f32, deg: f32) -> (f32, f32) {
    let a = deg.to_radians();
    (cx + r * a.sin(), cy - r * a.cos())
}

/// Gauge centre, radius, start angle (deg). The sweep is centred on 6 o'clock's gap.
pub fn gauge_geom(w: &Widget, sweep: u16) -> (f32, f32, f32, f32) {
    let r = w.w.min(w.h) as f32 / 2.0;
    (w.x as f32 + w.w as f32 / 2.0, w.y as f32 + w.h as f32 / 2.0, r, 360.0 - sweep as f32 / 2.0)
}

pub fn needle_len_width(r: f32) -> (f32, f32) {
    (r * 0.8, (r / 16.0).max(3.0))
}

/// Needle on black, pointing up, pivot at (width/2, len): the icon the screen rotates.
pub fn needle_icon(r: f32, c: Rgb) -> (Pixmap, u32, u32) {
    let (len, nw) = needle_len_width(r);
    let (iw, ih) = ((nw + 4.0).ceil() as u32, (len + nw + 2.0).ceil() as u32);
    let mut px = Pixmap::new(iw, ih).unwrap();
    px.fill(Color::BLACK);
    let cx = iw as f32 / 2.0;
    line(&mut px, &[(cx, len), (cx, nw / 2.0 + 1.0)], nw, c);
    (px, iw / 2, len as u32)
}

fn missing(px: &mut Pixmap, w: &Widget) {
    rounded(px, frect(w), 0.0, [90, 30, 30]);
    centered(px, frect(w), "?", (w.h as f32 * 0.6).max(8.0), [255, 200, 200]);
}

/// Draw a dynamic widget in a given state (toggle 0/1, icon index, slider knob at min).
pub fn draw_state(px: &mut Pixmap, w: &Widget, state: usize, base: &Path) {
    match &w.kind {
        WidgetKind::Toggle { on, off, knob, .. } => {
            let (x, y, ww, hh) = frect(w);
            rounded(px, (x, y, ww, hh), hh / 2.0, if state > 0 { *on } else { *off });
            let d = hh - 6.0;
            let kx = if state > 0 { x + ww - 3.0 - d } else { x + 3.0 };
            rounded(px, (kx, y + 3.0, d, d), d / 2.0, *knob);
        }
        WidgetKind::Lamp { on, off, .. } => {
            let (x, y, ww, hh) = frect(w);
            let d = ww.min(hh);
            rounded(px, (x + (ww - d) / 2.0, y + (hh - d) / 2.0, d, d), d / 2.0, if state > 0 { *on } else { *off });
        }
        WidgetKind::Slider { knob, .. } => {
            let d = w.h as f32;
            rounded(px, (w.x as f32, w.y as f32, d, d), d / 2.0, *knob);
        }
        WidgetKind::List { items, size, fg, bg, sel, .. } => {
            // state = selected row + 1 (0 = none).
            let (x, y, ww, hh) = frect(w);
            let rh = hh / items.len().max(1) as f32;
            rounded(px, (x, y, ww, hh), 0.0, *bg);
            for (i, it) in items.iter().enumerate() {
                let ry = y + i as f32 * rh;
                if state == i + 1 {
                    rounded(px, (x, ry, ww, rh), 0.0, *sel);
                }
                text(px, it, x + 12.0, ry + (rh - size) / 2.0, *size, *fg);
            }
        }
        WidgetKind::Dropdown { options, size, fg, .. } => {
            let t = options.get(state).map_or("", |s| s.as_str());
            text(px, t, w.x as f32 + 12.0, w.y as f32 + (w.h as f32 - size) / 2.0, *size, *fg);
        }
        WidgetKind::Icons { images, .. } | WidgetKind::Animation { images, .. } => match images.get(state).and_then(|p| load_image(base, p)) {
            Some(img) => picture(px, &img, w.rect(), 1.0),
            None => missing(px, w),
        },
        _ => {}
    }
}

pub fn render(page: &Page, width: u16, height: u16, mode: Mode, base: &Path) -> Pixmap {
    let mut px = Pixmap::new(width.max(1) as u32, height.max(1) as u32).unwrap();
    px.fill(Color::from_rgba8(page.bg[0], page.bg[1], page.bg[2], 255));
    if let Some(img) = page.bg_image.as_deref().and_then(|p| load_image(base, p)) {
        picture(&mut px, &img, (0, 0, width as i32, height as i32), 1.0);
    }
    let dynamic = mode != Mode::Base;
    for w in &page.widgets {
        match &w.kind {
            WidgetKind::Panel { color, radius } => rounded(&mut px, frect(w), *radius, *color),
            WidgetKind::Label { text: t, size, color } => text(&mut px, t, w.x as f32, w.y as f32, *size, *color),
            WidgetKind::Image { path } => match load_image(base, path) {
                Some(img) => picture(&mut px, &img, w.rect(), 1.0),
                None => missing(&mut px, w),
            },
            WidgetKind::Button { text: t, size, fg, bg, pressed_bg, radius, image, .. } => {
                let pressed = mode == Mode::Pressed;
                match image.as_deref().and_then(|p| load_image(base, p)) {
                    Some(img) => picture(&mut px, &img, w.rect(), if pressed { 0.7 } else { 1.0 }),
                    None => rounded(&mut px, frect(w), *radius, if pressed { *pressed_bg } else { *bg }),
                }
                centered(&mut px, frect(w), t, *size, *fg);
            }
            WidgetKind::Slider { track, show_track, .. } => {
                // Track is static; the knob is an icon the screen moves.
                let (x, y, ww, hh) = frect(w);
                let th = (hh / 4.0).max(2.0);
                if *show_track {
                    rounded(&mut px, (x + hh / 2.0, y + (hh - th) / 2.0, ww - hh, th), th / 2.0, *track);
                }
                // Knob only in the editor: on screen it is an icon; a baked one would stay behind.
                if mode == Mode::Preview {
                    draw_state(&mut px, w, 0, base);
                }
            }
            WidgetKind::Dropdown { bg, fg, .. } => {
                let (x, y, ww, hh) = frect(w);
                rounded(&mut px, (x, y, ww, hh), 8.0, *bg);
                // Caret: small down triangle at the right.
                let (tx, ty, s) = (x + ww - hh * 0.55, y + hh / 2.0, hh * 0.15);
                let mut pb = PathBuilder::new();
                pb.move_to(tx - s, ty - s / 2.0);
                pb.line_to(tx + s, ty - s / 2.0);
                pb.line_to(tx, ty + s);
                pb.close();
                px.fill_path(&pb.finish().unwrap(), &paint(*fg), FillRule::Winding, Transform::identity(), None);
                if dynamic {
                    draw_state(&mut px, w, 0, base);
                }
            }
            WidgetKind::Drawing { bg, .. } => rounded(&mut px, frect(w), 0.0, *bg),
            WidgetKind::Chart { step, color, bg, .. } => {
                rounded(&mut px, frect(w), 0.0, *bg);
                if mode == Mode::Preview {
                    let (x, y, ww, hh) = frect(w);
                    let pts: Vec<_> = (0..=(ww as i32 / (*step).max(1) as i32))
                        .map(|i| {
                            let px_x = x + i as f32 * (*step).max(1) as f32;
                            let v = (px_x / 30.0).sin() * 0.4 + 0.5; // fake samples
                            (px_x, y + hh * (1.0 - v))
                        })
                        .collect();
                    line(&mut px, &pts, 2.0, *color);
                }
            }
            WidgetKind::Gauge { sweep, needle, dial, .. } => {
                let (cx, cy, r, start) = gauge_geom(w, *sweep);
                let arc: Vec<_> = (0..=64).map(|i| polar(cx, cy, r - 4.0, start + *sweep as f32 * i as f32 / 64.0)).collect();
                line(&mut px, &arc, 4.0, *dial);
                for i in 0..=10 {
                    let a = start + *sweep as f32 * i as f32 / 10.0;
                    line(&mut px, &[polar(cx, cy, r * 0.82, a), polar(cx, cy, r - 6.0, a)], 2.0, *dial);
                }
                if mode == Mode::Preview {
                    let (len, nw) = needle_len_width(r);
                    line(&mut px, &[(cx, cy), polar(cx, cy, len, start)], nw, *needle);
                }
            }
            WidgetKind::Clock { format, color } if mode == Mode::Preview => {
                let s: String = format
                    .chars()
                    .map(|c| match c {
                        'Y' => "2026".into(), 'M' => "11".into(), 'D' => "08".into(), 'H' => "12".into(),
                        'Q' => "34".into(), 'S' => "56".into(), 'W' => "SUN".into(), c => c.to_string(),
                    })
                    .collect();
                text(&mut px, &s, w.x as f32, w.y as f32, w.h as f32, *color);
            }
            WidgetKind::Qr { .. } if mode == Mode::Preview => {
                rounded(&mut px, frect(w), 0.0, [255; 3]);
                let cell = (w.w.min(w.h) / 9).max(1);
                for i in 0..81 {
                    if (i * 7 + i / 9) % 3 == 0 {
                        let (cx, cy) = (w.x + (i % 9) * cell, w.y + (i / 9) * cell);
                        rounded(&mut px, (cx as f32, cy as f32, cell as f32, cell as f32), 0.0, [0; 3]);
                    }
                }
            }
            WidgetKind::Progress { fg, bg, border, vertical, .. } if mode == Mode::Preview => {
                let (x, y, ww, hh) = frect(w);
                rounded(&mut px, (x, y, ww, hh), 0.0, *border);
                rounded(&mut px, (x + 1.0, y + 1.0, ww - 2.0, hh - 2.0), 0.0, *bg);
                let r = if *vertical { (x + 1.0, y + hh * 0.6, ww - 2.0, hh * 0.4 - 1.0) } else { (x + 1.0, y + 1.0, ww * 0.4, hh - 2.0) };
                rounded(&mut px, r, 0.0, *fg);
            }
            WidgetKind::Toggle { .. } | WidgetKind::Lamp { .. } | WidgetKind::Icons { .. } | WidgetKind::Animation { .. } | WidgetKind::List { .. }
                if dynamic =>
            {
                draw_state(&mut px, w, 0, base)
            }
            // Editable fields get a box so the operator sees where to tap.
            WidgetKind::Number { input: Some(_), .. } | WidgetKind::Text { input: true, .. } if mode != Mode::Preview => {
                rounded(&mut px, frect(w), 6.0, [30, 41, 59]);
            }
            WidgetKind::Number { int_digits, dec_digits, color, input, .. } if mode == Mode::Preview => {
                if input.is_some() {
                    rounded(&mut px, frect(w), 6.0, [30, 41, 59]);
                }
                let mut s = "8".repeat(*int_digits as usize);
                if *dec_digits > 0 {
                    s += &format!(".{}", "8".repeat(*dec_digits as usize));
                }
                text(&mut px, &s, w.x as f32, w.y as f32, w.h as f32, *color);
            }
            WidgetKind::Text { color, input, .. } if mode == Mode::Preview => {
                if *input {
                    rounded(&mut px, frect(w), 6.0, [30, 41, 59]);
                }
                text(&mut px, &w.name, w.x as f32, w.y as f32, w.h as f32, *color)
            }
            _ => {}
        }
    }
    px
}

/// Cut `w`'s area out of a Base render and draw the widget in `state` on it: one DWIN icon.
pub fn icon(base_px: &Pixmap, w: &Widget, state: usize, size: (i32, i32), base: &Path) -> Pixmap {
    let (iw, ih) = (size.0.max(1) as u32, size.1.max(1) as u32);
    let mut px = Pixmap::new(iw, ih).unwrap();
    let (bw, bh) = (base_px.width() as i32, base_px.height() as i32);
    for y in 0..ih as i32 {
        for x in 0..iw as i32 {
            let (sx, sy) = ((w.x + x).clamp(0, bw - 1), (w.y + y).clamp(0, bh - 1));
            let s = 4 * (sy * bw + sx) as usize;
            let d = 4 * (y * iw as i32 + x) as usize;
            px.data_mut()[d..d + 4].copy_from_slice(&base_px.data()[s..s + 4]);
        }
    }
    let mut local = w.clone();
    (local.x, local.y) = (0, 0);
    draw_state(&mut px, &local, state, base);
    px
}

/// DWIN ASCII font library (`0_DWIN_ASC.HZK`) drawn with the editor font, so text on the
/// screen looks like the preview. Layout (checked against DWIN's file): sizes 4..=64, each
/// 128 chars of `size` x `2*size` dots, rows of ceil(size/8) bytes, MSB = leftmost.
pub fn ascii_hzk() -> Vec<u8> {
    let f = font();
    let mut out = Vec::with_capacity(3_082_752);
    for size in 4..=64usize {
        let (w, h) = (size, 2 * size);
        let bpr = w.div_ceil(8);
        let sf = f.as_scaled(PxScale::from(h as f32));
        for c in 0..128u8 {
            let mut cell = vec![0u8; bpr * h];
            if c >= 0x20 {
                let id = sf.glyph_id(c as char);
                let x = ((w as f32 - sf.h_advance(id)) / 2.0).max(0.0);
                if let Some(o) = f.outline_glyph(id.with_scale_and_position(h as f32, point(x, sf.ascent()))) {
                    let b = o.px_bounds();
                    o.draw(|gx, gy, cov| {
                        let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                        if cov >= 0.35 && px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                            cell[py as usize * bpr + px as usize / 8] |= 0x80 >> (px % 8);
                        }
                    });
                }
            }
            out.extend(cell);
        }
    }
    out
}

/// Pixmap (premultiplied, but always opaque here) -> RGBA image.
pub fn to_image(px: &Pixmap) -> image::DynamicImage {
    let rgba = image::RgbaImage::from_raw(px.width(), px.height(), px.data().to_vec()).unwrap();
    image::DynamicImage::ImageRgba8(rgba)
}
