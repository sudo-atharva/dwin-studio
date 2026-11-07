//! Editor project: what the user designs. Saved as JSON (`*.dwin.json`), compiled by `build`.
//! Image paths are relative to the project file's folder (imports go to `assets/`).

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
    /// Background picture, stretched to the screen (drawn over `bg`).
    #[serde(default)]
    pub bg_image: Option<String>,
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Widget {
    /// Used for `ui_gen.h` names and as the target of +/- buttons. Keep unique.
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// What this widget does; goes into ui_map.txt and ui_gen.h.
    #[serde(default)]
    pub comment: String,
    pub kind: WidgetKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WidgetKind {
    /// Static text, baked into the page image.
    Label { text: String, size: f32, color: Rgb },
    /// Static filled box, baked into the page image.
    Panel { color: Rgb, radius: f32 },
    /// Static picture, baked into the page image.
    Image { path: String },
    Button {
        text: String,
        size: f32,
        fg: Rgb,
        bg: Rgb,
        pressed_bg: Rgb,
        radius: f32,
        /// Optional picture drawn instead of the filled shape (pressed = darkened).
        #[serde(default)]
        image: Option<String>,
        /// Switch to this page index on release.
        goto: Option<usize>,
        event: Event,
    },
    /// Number shown by the screen from a VP (int16, fixed-point by `dec_digits`).
    Number { vp: Option<u16>, int_digits: u8, dec_digits: u8, color: Rgb },
    /// Text shown by the screen from a VP (ASCII, `len` bytes).
    Text { vp: Option<u16>, len: u16, color: Rgb },
    /// On/off switch. Tapping flips VP between 0 and 1.
    Toggle { vp: Option<u16>, on: Rgb, off: Rgb, knob: Rgb },
    /// Horizontal slider. Dragging writes min..max to VP; MCU writes move the knob.
    Slider { vp: Option<u16>, min: u16, max: u16, track: Rgb, knob: Rgb },
    /// Shows `images[v]` for VP value v (status lights, battery, modes...).
    Icons { vp: Option<u16>, images: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    None,
    /// Write `key` to VP and send it to the MCU (DGUS return key).
    Key { vp: Option<u16>, key: u16 },
    /// Change another widget's value by `step` (DGUS increment): +/- buttons.
    Adjust { target: String, step: i16, min: u16, max: u16, wrap: bool, repeat: bool },
}

impl WidgetKind {
    /// The widget's own VP setting, if it has one.
    pub fn vp_mut(&mut self) -> Option<&mut Option<u16>> {
        match self {
            WidgetKind::Button { event: Event::Key { vp, .. }, .. }
            | WidgetKind::Number { vp, .. }
            | WidgetKind::Text { vp, .. }
            | WidgetKind::Toggle { vp, .. }
            | WidgetKind::Slider { vp, .. }
            | WidgetKind::Icons { vp, .. } => Some(vp),
            _ => None,
        }
    }

    pub fn vp(&self) -> Option<Option<u16>> {
        self.clone().vp_mut().map(|v| *v)
    }
}

impl Widget {
    pub fn new(kind: WidgetKind, name: &str, x: i32, y: i32) -> Self {
        let (w, h) = match &kind {
            WidgetKind::Label { .. } => (160, 32),
            WidgetKind::Panel { .. } => (200, 120),
            WidgetKind::Image { .. } => (120, 120),
            WidgetKind::Button { .. } => (160, 60),
            WidgetKind::Number { .. } => (120, 40),
            WidgetKind::Text { .. } => (200, 32),
            WidgetKind::Toggle { .. } => (80, 40),
            WidgetKind::Slider { .. } => (300, 40),
            WidgetKind::Icons { .. } => (64, 64),
        };
        Self { name: name.into(), x, y, w, h, comment: String::new(), kind }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            WidgetKind::Label { .. } => "Label",
            WidgetKind::Panel { .. } => "Panel",
            WidgetKind::Image { .. } => "Image",
            WidgetKind::Button { .. } => "Button",
            WidgetKind::Number { .. } => "Number",
            WidgetKind::Text { .. } => "Text",
            WidgetKind::Toggle { .. } => "Toggle",
            WidgetKind::Slider { .. } => "Slider",
            WidgetKind::Icons { .. } => "Icons",
        }
    }

    pub fn rect(&self) -> (i32, i32, i32, i32) {
        (self.x, self.y, self.w, self.h)
    }
}

pub fn button(text: &str, event: Event) -> WidgetKind {
    WidgetKind::Button {
        text: text.into(), size: 24.0, fg: [255; 3], bg: [37, 99, 235], pressed_bg: [29, 78, 216],
        radius: 10.0, image: None, goto: None, event,
    }
}

/// Palette entries: (name, kind).
pub fn default_widgets() -> Vec<(&'static str, WidgetKind)> {
    vec![
        ("Button", button("Button", Event::Key { vp: None, key: 1 })),
        ("+ / − Button", button("+", Event::Adjust { target: String::new(), step: 1, min: 0, max: 100, wrap: false, repeat: true })),
        ("Label", WidgetKind::Label { text: "Label".into(), size: 24.0, color: [255; 3] }),
        ("Panel", WidgetKind::Panel { color: [30, 41, 59], radius: 12.0 }),
        ("Number", WidgetKind::Number { vp: None, int_digits: 3, dec_digits: 1, color: [255; 3] }),
        ("Text", WidgetKind::Text { vp: None, len: 20, color: [255; 3] }),
        ("Toggle", WidgetKind::Toggle { vp: None, on: [34, 197, 94], off: [71, 85, 105], knob: [255; 3] }),
        ("Slider", WidgetKind::Slider { vp: None, min: 0, max: 100, track: [71, 85, 105], knob: [255; 3] }),
        ("Icons", WidgetKind::Icons { vp: None, images: vec![] }),
    ]
}

impl Default for Project {
    fn default() -> Self {
        Self {
            width: 800,
            height: 480,
            pages: vec![Page { name: "home".into(), bg: [15, 23, 42], bg_image: None, widgets: vec![] }],
        }
    }
}
