//! Editor color themes. Each theme is a small JSON file in `themes/` next to the exe,
//! so a person or an AI agent can add one by writing a file (see themes/README.txt).

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Vec2};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

type Rgb = [u8; 3];

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    /// Base look: true = dark, false = light. Hover/border shades are derived from it.
    pub dark: bool,
    pub accent: Rgb,
    /// Side panels and toolbar.
    pub background: Rgb,
    /// Buttons, dropdowns.
    pub surface: Rgb,
    /// Text boxes and number fields.
    pub field: Rgb,
    pub text: Rgb,
    /// Around the screen in the middle.
    pub canvas: Rgb,
    /// Status bar while editing / simulating.
    pub status_edit: Rgb,
    pub status_sim: Rgb,
}

pub fn builtin() -> [Theme; 2] {
    [
        Theme {
            name: "Dark".into(), dark: true, accent: [56, 139, 253], background: [37, 37, 38], surface: [58, 58, 62],
            field: [28, 28, 30], text: [212, 212, 212], canvas: [30, 30, 30], status_edit: [0, 102, 184], status_sim: [22, 130, 70],
        },
        Theme {
            name: "Light".into(), dark: false, accent: [0, 95, 184], background: [243, 243, 243], surface: [226, 226, 230],
            field: [255, 255, 255], text: [32, 32, 36], canvas: [214, 216, 222], status_edit: [0, 102, 184], status_sim: [22, 130, 70],
        },
    ]
}

pub fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// `themes/` next to the exe (created with the built-in themes and a README on first run).
pub fn dir() -> PathBuf {
    let d = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("themes"))).unwrap_or("themes".into());
    if !d.join("README.txt").exists() && std::fs::create_dir_all(&d).is_ok() {
        for t in builtin() {
            let f = d.join(format!("{}.json", t.name.to_lowercase()));
            if !f.exists() {
                let _ = std::fs::write(f, serde_json::to_string_pretty(&t).unwrap());
            }
        }
        let _ = std::fs::write(d.join("README.txt"), README);
    }
    d
}

/// Built-ins plus every valid *.json in `themes/` (a file with a built-in's name replaces it).
pub fn load_all() -> Vec<Theme> {
    let mut all = builtin().to_vec();
    let mut files: Vec<_> = std::fs::read_dir(dir()).into_iter().flatten().flatten().map(|e| e.path()).collect();
    files.sort();
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "json")) {
        if let Some(t) = std::fs::read_to_string(f).ok().and_then(|s| serde_json::from_str::<Theme>(&s).ok()) {
            all.retain(|o| o.name != t.name);
            all.push(t);
        }
    }
    all
}

pub fn saved_name() -> String {
    std::fs::read_to_string(dir().join("selected.txt")).map_or("Dark".into(), |s| s.trim().to_string())
}

pub fn save_name(name: &str) {
    let _ = std::fs::write(dir().join("selected.txt"), name);
}

/// Shift a color away from the base: lighter on dark themes, darker on light ones.
fn shade(t: &Theme, rgb: Rgb, d: i16) -> Color32 {
    let d = if t.dark { d } else { -d };
    let f = |v: u8| (v as i16 + d).clamp(0, 255) as u8;
    Color32::from_rgb(f(rgb[0]), f(rgb[1]), f(rgb[2]))
}

pub fn apply(ctx: &egui::Context, t: &Theme) {
    let which = if t.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(which);
    ctx.style_mut_of(which, |s| style(s, t));
}

fn style(s: &mut egui::Style, t: &Theme) {
    let mut v = if t.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let (text, accent) = (c(t.text), c(t.accent));
    let sel = Color32::from_rgb(
        ((t.accent[0] as u16 + t.background[0] as u16) / 2) as u8,
        ((t.accent[1] as u16 + t.background[1] as u16) / 2) as u8,
        ((t.accent[2] as u16 + t.background[2] as u16) / 2) as u8,
    );
    v.panel_fill = c(t.background);
    v.window_fill = shade(t, t.background, 8);
    v.faint_bg_color = shade(t, t.background, 5);
    v.extreme_bg_color = c(t.field);
    v.code_bg_color = c(t.field);
    v.hyperlink_color = accent;
    v.selection.bg_fill = sel;
    v.selection.stroke = Stroke::new(1.0, text);
    v.window_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, shade(t, t.background, 23));
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, shade(t, t.background, 15));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
    v.widgets.inactive.weak_bg_fill = c(t.surface);
    v.widgets.inactive.bg_fill = c(t.surface);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, text);
    v.widgets.hovered.weak_bg_fill = shade(t, t.surface, 14);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, shade(t, t.surface, 32));
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, text);
    v.widgets.active.weak_bg_fill = sel;
    v.widgets.active.fg_stroke = Stroke::new(1.5, text);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(5);
    }
    s.visuals = v;
    s.spacing.item_spacing = Vec2::new(8.0, 6.0);
    s.spacing.button_padding = Vec2::new(10.0, 4.0);
    s.spacing.interact_size.y = 24.0;
    s.spacing.window_margin = egui::Margin::same(10);
    for (ts, size) in [(TextStyle::Body, 13.5), (TextStyle::Button, 13.5), (TextStyle::Heading, 17.0), (TextStyle::Small, 11.0)] {
        s.text_styles.insert(ts, FontId::proportional(size));
    }
    s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.5));
}

const README: &str = r#"DWIN Studio themes
==================

Every *.json file in this folder is an editor color theme. Pick one with the
theme button in the toolbar (the list re-reads this folder each time you open it).

To make a theme, copy dark.json or light.json, change "name" and the colors, save.
An AI agent can do the same: write one JSON file here with exactly these fields.

{
  "name": "Ocean",               shown in the theme list (unique)
  "dark": true,                  true = dark base, false = light base
  "accent": [56, 139, 253],      selection, links, highlights
  "background": [37, 37, 38],    side panels, toolbar
  "surface": [58, 58, 62],       buttons, dropdowns
  "field": [28, 28, 30],         text boxes, number fields
  "text": [212, 212, 212],       all text
  "canvas": [30, 30, 30],        area around the screen being edited
  "status_edit": [0, 102, 184],  status bar while editing
  "status_sim": [22, 130, 70]    status bar while simulating
}

Colors are [red, green, blue], 0..255. Keep "text" readable on "background",
"surface" and "field". A file that isn't valid JSON or misses a field is skipped.
Themes only change the editor, never what goes on the DWIN screen.
selected.txt remembers the last theme picked.
"#;
