//! Draw a page to pixels. Same code for the editor canvas and the images that go on the screen.

use crate::project::{Page, Rgb, Widget, WidgetKind};
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use image::RgbaImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

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

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("bundled font")
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

fn paint(c: Rgb) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], 255));
    p.anti_alias = true;
    p
}

fn rounded(px: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), r: f32, c: Rgb) {
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

fn frect(w: &Widget) -> (f32, f32, f32, f32) {
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
fn text(px: &mut Pixmap, s: &str, x: f32, y: f32, size: f32, c: Rgb) {
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
fn picture(px: &mut Pixmap, img: &RgbaImage, (x, y, w, h): (i32, i32, i32, i32), dim: f32) {
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
        WidgetKind::Slider { knob, .. } => {
            let d = w.h as f32;
            rounded(px, (w.x as f32, w.y as f32, d, d), d / 2.0, *knob);
        }
        WidgetKind::Icons { images, .. } => match images.get(state).and_then(|p| load_image(base, p)) {
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
            WidgetKind::Slider { track, .. } => {
                // Track is static; the knob is an icon the screen moves.
                let (x, y, ww, hh) = frect(w);
                let th = (hh / 4.0).max(2.0);
                rounded(&mut px, (x + hh / 2.0, y + (hh - th) / 2.0, ww - hh, th), th / 2.0, *track);
                // Knob only in the editor: on screen it is an icon; a baked one would stay behind.
                if mode == Mode::Preview {
                    draw_state(&mut px, w, 0, base);
                }
            }
            WidgetKind::Toggle { .. } | WidgetKind::Icons { .. } if dynamic => draw_state(&mut px, w, 0, base),
            WidgetKind::Number { int_digits, dec_digits, color, .. } if mode == Mode::Preview => {
                let mut s = "8".repeat(*int_digits as usize);
                if *dec_digits > 0 {
                    s += &format!(".{}", "8".repeat(*dec_digits as usize));
                }
                text(&mut px, &s, w.x as f32, w.y as f32, w.h as f32, *color);
            }
            WidgetKind::Text { color, .. } if mode == Mode::Preview => {
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

/// Pixmap (premultiplied, but always opaque here) -> RGBA image.
pub fn to_image(px: &Pixmap) -> image::DynamicImage {
    let rgba = image::RgbaImage::from_raw(px.width(), px.height(), px.data().to_vec()).unwrap();
    image::DynamicImage::ImageRgba8(rgba)
}
