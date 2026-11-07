//! Draw a page to pixels. Same code for the editor canvas and the images that go on the screen.

use crate::project::{Page, Rgb, Widget, WidgetKind};
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// Page background as the screen stores it (buttons up, no runtime values).
    Normal,
    /// Same with every button drawn pressed. DWIN copies button regions from this image on touch.
    Pressed,
    /// Editor view: normal + placeholders for runtime values.
    Preview,
}

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("bundled font")
}

fn paint(c: Rgb) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], 255));
    p.anti_alias = true;
    p
}

fn rounded_rect(px: &mut Pixmap, w: &Widget, r: f32, c: Rgb) {
    let (x, y, ww, hh) = (w.x as f32, w.y as f32, w.w.max(1) as f32, w.h.max(1) as f32);
    let r = r.min(ww / 2.0).min(hh / 2.0).max(0.0);
    let path = if r == 0.0 {
        PathBuilder::from_rect(Rect::from_xywh(x, y, ww, hh).unwrap())
    } else {
        let k = 0.5523 * r; // cubic circle approximation
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + ww - r, y);
        pb.cubic_to(x + ww - r + k, y, x + ww, y + r - k, x + ww, y + r);
        pb.line_to(x + ww, y + hh - r);
        pb.cubic_to(x + ww, y + hh - r + k, x + ww - r + k, y + hh, x + ww - r, y + hh);
        pb.line_to(x + r, y + hh);
        pb.cubic_to(x + r - k, y + hh, x, y + hh - r + k, x, y + hh - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        pb.close();
        pb.finish().unwrap()
    };
    px.fill_path(&path, &paint(c), FillRule::Winding, Transform::identity(), None);
}

pub fn text_width(text: &str, size: f32) -> f32 {
    let f = font();
    let sf = f.as_scaled(PxScale::from(size));
    text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum()
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
        o.draw(|gx, gy, cov| {
            let (px_x, px_y) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
            if px_x < 0 || px_y < 0 || px_x >= pw || px_y >= ph {
                return;
            }
            let i = 4 * (px_y * pw + px_x) as usize;
            for k in 0..3 {
                let d = data[i + k] as f32;
                data[i + k] = (d + (c[k] as f32 - d) * cov.min(1.0)) as u8;
            }
        });
    }
}

fn centered(px: &mut Pixmap, w: &Widget, s: &str, size: f32, c: Rgb) {
    let tx = w.x as f32 + (w.w as f32 - text_width(s, size)) / 2.0;
    let ty = w.y as f32 + (w.h as f32 - size) / 2.0;
    text(px, s, tx, ty, size, c);
}

pub fn render(page: &Page, width: u16, height: u16, mode: Mode) -> Pixmap {
    let mut px = Pixmap::new(width.max(1) as u32, height.max(1) as u32).unwrap();
    px.fill(Color::from_rgba8(page.bg[0], page.bg[1], page.bg[2], 255));
    for w in &page.widgets {
        match &w.kind {
            WidgetKind::Panel { color, radius } => rounded_rect(&mut px, w, *radius, *color),
            WidgetKind::Label { text: t, size, color } => {
                text(&mut px, t, w.x as f32, w.y as f32, *size, *color)
            }
            WidgetKind::Button { text: t, size, fg, bg, pressed_bg, radius, .. } => {
                let fill = if mode == Mode::Pressed { *pressed_bg } else { *bg };
                rounded_rect(&mut px, w, *radius, fill);
                centered(&mut px, w, t, *size, *fg);
            }
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
            WidgetKind::Number { .. } | WidgetKind::Text { .. } => {}
        }
    }
    px
}

/// Pixmap (premultiplied, but always opaque here) -> RGB image.
pub fn to_image(px: &Pixmap) -> image::DynamicImage {
    let rgba = image::RgbaImage::from_raw(px.width(), px.height(), px.data().to_vec()).unwrap();
    image::DynamicImage::ImageRgba8(rgba)
}
