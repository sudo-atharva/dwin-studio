//! Editor project: what the user designs. Saved as JSON (`*.dwin.json`), compiled by `build`.

use serde::{Deserialize, Serialize};

pub type Rgb = [u8; 3];

/// Common DWIN panel sizes. Any width/height works; these are just shortcuts.
pub const PRESETS: &[(u16, u16)] = &[
    (320, 240), (480, 272), (480, 480), (480, 854), (800, 480), (854, 480),
    (800, 600), (1024, 600), (600, 1024), (1024, 768), (1280, 800),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub width: u16,
    pub height: u16,
    pub pages: Vec<Page>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub name: String,
    pub bg: Rgb,
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Widget {
    /// Used for `ui_gen.h` names. Unique per project is best.
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub kind: WidgetKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WidgetKind {
    /// Static text, baked into the page image.
    Label { text: String, size: f32, color: Rgb },
    /// Static filled box, baked into the page image.
    Panel { color: Rgb, radius: f32 },
    Button {
        text: String,
        size: f32,
        fg: Rgb,
        bg: Rgb,
        pressed_bg: Rgb,
        radius: f32,
        action: Action,
    },
    /// Number shown by the screen from a VP (int16, fixed-point by `dec_digits`).
    Number { vp: Option<u16>, int_digits: u8, dec_digits: u8, color: Rgb },
    /// Text shown by the screen from a VP (ASCII, `len` bytes).
    Text { vp: Option<u16>, len: u16, color: Rgb },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    None,
    /// Switch to page (index into `Project::pages`).
    GoTo(usize),
    /// Write `key` to VP and send it to the MCU (DGUS "return key code").
    ReturnKey { vp: Option<u16>, key: u16 },
}

impl Widget {
    pub fn new(kind: WidgetKind, name: &str, x: i32, y: i32) -> Self {
        let (w, h) = match &kind {
            WidgetKind::Label { .. } => (160, 32),
            WidgetKind::Panel { .. } => (200, 120),
            WidgetKind::Button { .. } => (160, 60),
            WidgetKind::Number { .. } => (120, 40),
            WidgetKind::Text { .. } => (200, 32),
        };
        Self { name: name.into(), x, y, w, h, kind }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            WidgetKind::Label { .. } => "Label",
            WidgetKind::Panel { .. } => "Panel",
            WidgetKind::Button { .. } => "Button",
            WidgetKind::Number { .. } => "Number",
            WidgetKind::Text { .. } => "Text",
        }
    }
}

pub fn default_widgets() -> Vec<(&'static str, WidgetKind)> {
    vec![
        ("Button", WidgetKind::Button {
            text: "Button".into(), size: 24.0, fg: [255; 3], bg: [37, 99, 235],
            pressed_bg: [29, 78, 216], radius: 10.0, action: Action::None,
        }),
        ("Label", WidgetKind::Label { text: "Label".into(), size: 24.0, color: [255; 3] }),
        ("Panel", WidgetKind::Panel { color: [30, 41, 59], radius: 12.0 }),
        ("Number", WidgetKind::Number { vp: None, int_digits: 3, dec_digits: 1, color: [255; 3] }),
        ("Text", WidgetKind::Text { vp: None, len: 20, color: [255; 3] }),
    ]
}

impl Default for Project {
    fn default() -> Self {
        Self {
            width: 800,
            height: 480,
            pages: vec![Page { name: "home".into(), bg: [15, 23, 42], widgets: vec![] }],
        }
    }
}
