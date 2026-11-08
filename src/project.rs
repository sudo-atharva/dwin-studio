//! Editor project: what the user designs. Saved as JSON (`*.dwin`; older `*.dwin.json` still opens), compiled by `build`.
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
    /// Icon library numbers in DWIN_SET: page pictures (DGUS default 32) and generated
    /// icons (toggle states, knobs, list states...). Must match what the screen expects.
    #[serde(default = "page_lib")]
    pub page_lib: u8,
    #[serde(default = "icon_lib")]
    pub icon_lib: u8,
    /// TTF/OTF (relative to the project) for previews and the screen's ASCII font
    /// (0_DWIN_ASC.HZK). None = built-in Ubuntu.
    #[serde(default)]
    pub font: Option<String>,
    /// The screen's own T5LCFG.CFG (baud, backlight, LCD timing), copied into every build.
    /// Not generated: a wrong byte there can blank the panel.
    #[serde(default)]
    pub cfg: Option<String>,
    /// Unicode text fonts: one .dzk per text size, numbered from `unicode_lib`, covering
    /// U+0000..=`unicode_max` (bigger range = bigger file; glyphs come from `font`).
    #[serde(default = "unicode_lib")]
    pub unicode_lib: u8,
    #[serde(default = "unicode_max")]
    pub unicode_max: u32,
}

fn unicode_lib() -> u8 {
    24
}

fn unicode_max() -> u32 {
    0x052F // Latin, Greek, Cyrillic
}

fn page_lib() -> u8 {
    32
}

fn icon_lib() -> u8 {
    48
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub name: String,
    pub bg: Rgb,
    /// Background picture, stretched to the screen (drawn over `bg`).
    #[serde(default)]
    pub bg_image: Option<String>,
    /// Pages a finger swipe goes to: (swipe right = previous, swipe left = next).
    #[serde(default)]
    pub swipe: (Option<usize>, Option<usize>),
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
        /// Touch area only: nothing drawn (the page picture shows the button).
        #[serde(default)]
        invisible: bool,
    },
    /// Number shown by the screen from a VP (int16, fixed-point by `dec_digits`).
    /// `input` = Some((min, max)): tapping opens a generated keypad.
    Number {
        vp: Option<u16>,
        int_digits: u8,
        dec_digits: u8,
        color: Rgb,
        #[serde(default)]
        input: Option<(i32, i32)>,
        /// Draw digits with the editor's smooth font (pre-rendered digit icons) instead of
        /// the screen's built-in bitmap font.
        #[serde(default)]
        smooth: bool,
    },
    /// Text shown by the screen from a VP (ASCII, `len` bytes). `input`: tap opens a keyboard.
    Text {
        vp: Option<u16>,
        len: u16,
        color: Rgb,
        #[serde(default)]
        input: bool,
        /// Marquee: text scrolls inside the box.
        #[serde(default)]
        scroll: bool,
        /// Any language: VP holds UTF-16BE (2 bytes per character) drawn with a generated
        /// unicode font. Off = plain ASCII with the built-in font 0.
        #[serde(default)]
        unicode: bool,
    },
    /// On/off switch. Tapping flips VP between 0 and 1.
    /// `bit` = Some(n): flips only bit n of VP (many toggles can share one VP).
    Toggle {
        vp: Option<u16>,
        on: Rgb,
        off: Rgb,
        knob: Rgb,
        #[serde(default)]
        bit: Option<u8>,
    },
    /// Status light: on when VP (or bit `bit` of VP) is set.
    Lamp { vp: Option<u16>, bit: Option<u8>, on: Rgb, off: Rgb },
    /// Horizontal slider. Dragging writes min..max to VP; MCU writes move the knob.
    /// `show_track` false = no bar drawn (only the knob; background shows through).
    Slider {
        vp: Option<u16>,
        min: u16,
        max: u16,
        track: Rgb,
        knob: Rgb,
        #[serde(default = "yes")]
        show_track: bool,
    },
    /// Shows `images[v]` for VP value v (status lights, battery, modes...).
    Icons { vp: Option<u16>, images: Vec<String> },
    /// Items stacked in rows. Tapping row i highlights it and sends VP = i + 1 (0 = none
    /// selected), so a separate "OK" button can act on the choice.
    /// `mcu_len` > 0: row texts come from the MCU (row i at VP + 0x10 + i * mcu_len/2,
    /// `mcu_len` bytes ASCII); `items` are then only editor placeholders.
    List {
        vp: Option<u16>,
        items: Vec<String>,
        size: f32,
        fg: Rgb,
        bg: Rgb,
        sel: Rgb,
        #[serde(default)]
        mcu_len: u16,
    },
    /// Picker wheel: shows previous / current / next item; tapping above or below the
    /// middle steps VP (0..n-1) and sends it. `wrap`: past the end goes back to the start.
    Roller { vp: Option<u16>, items: Vec<String>, size: f32, fg: Rgb, bg: Rgb, wrap: bool },
    /// Tap opens a list; picking item i writes i to VP and shows it in the box.
    Dropdown { vp: Option<u16>, options: Vec<String>, size: f32, fg: Rgb, bg: Rgb },
    /// Screen's real-time clock. `format`: Y M D H Q(minute) S W(weekday), other chars as-is.
    /// `settable`: tapping opens the keypad to set the screen's clock (DGUS RTC input).
    Clock {
        format: String,
        color: Rgb,
        #[serde(default)]
        settable: bool,
    },
    /// QR code of the text at VP (end it with 0xFFFF). `dot` = pixels per module.
    Qr { vp: Option<u16>, len: u16, dot: u8 },
    /// Bar filled from VP between min and max, drawn by the screen.
    Progress { vp: Option<u16>, min: u16, max: u16, fg: Rgb, bg: Rgb, border: Rgb, vertical: bool },
    /// Live curve; MCU pushes samples to `channel` (0..7) through VP 0x0310.
    Chart { channel: u8, min: i16, max: i16, step: u8, color: Rgb, bg: Rgb },
    /// Dial with a needle rotated by VP between min and max over `sweep` degrees.
    /// `knob`: dragging around the dial sets the value (DGUS rotate adjust).
    Gauge {
        vp: Option<u16>,
        min: u16,
        max: u16,
        sweep: u16,
        needle: Rgb,
        dial: Rgb,
        #[serde(default)]
        knob: bool,
    },
    /// Area the MCU draws into with DGUS basic-graphics commands (lines, boxes, dots)
    /// written to a `words`-long buffer at VP.
    Drawing { vp: Option<u16>, words: u16, bg: Rgb },
    /// Frames played in a loop while VP = 1 (stops on 0). `autoplay` sets VP = 1 at boot.
    Animation { vp: Option<u16>, images: Vec<String>, period_ms: u16, autoplay: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    None,
    /// Write `key` to VP and send it to the MCU (DGUS return key).
    Key { vp: Option<u16>, key: u16 },
    /// Latching on/off: each tap flips VP between 0 and 1 and the button stays pressed
    /// while it is 1.
    Latch { vp: Option<u16> },
    /// Change another widget's value by `step` (DGUS increment): +/- buttons.
    Adjust { target: String, step: i16, min: u16, max: u16, wrap: bool, repeat: bool },
}

impl WidgetKind {
    /// Row `i` text VP of a list filled by the MCU.
    pub fn row_vp(vp: u16, mcu_len: u16, i: usize) -> u16 {
        vp + 0x10 + i as u16 * mcu_len.div_ceil(2)
    }

    /// The widget's own VP setting, if it has one.
    pub fn vp_mut(&mut self) -> Option<&mut Option<u16>> {
        match self {
            WidgetKind::Button { event: Event::Key { vp, .. } | Event::Latch { vp }, .. }
            | WidgetKind::Number { vp, .. }
            | WidgetKind::Text { vp, .. }
            | WidgetKind::Toggle { vp, .. }
            | WidgetKind::Lamp { vp, .. }
            | WidgetKind::Slider { vp, .. }
            | WidgetKind::Icons { vp, .. }
            | WidgetKind::Dropdown { vp, .. }
            | WidgetKind::List { vp, .. }
            | WidgetKind::Roller { vp, .. }
            | WidgetKind::Qr { vp, .. }
            | WidgetKind::Progress { vp, .. }
            | WidgetKind::Gauge { vp, .. }
            | WidgetKind::Animation { vp, .. }
            | WidgetKind::Drawing { vp, .. } => Some(vp),
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
            WidgetKind::Lamp { .. } => (32, 32),
            WidgetKind::Slider { .. } => (300, 40),
            WidgetKind::Icons { .. } => (64, 64),
            WidgetKind::Dropdown { .. } => (200, 44),
            WidgetKind::List { .. } => (260, 180),
            WidgetKind::Roller { .. } => (160, 150),
            WidgetKind::Clock { .. } => (320, 32),
            WidgetKind::Qr { .. } => (120, 120),
            WidgetKind::Progress { .. } => (300, 24),
            WidgetKind::Chart { .. } => (360, 180),
            WidgetKind::Gauge { .. } => (200, 200),
            WidgetKind::Animation { .. } => (64, 64),
            WidgetKind::Drawing { .. } => (300, 200),
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
            WidgetKind::Lamp { .. } => "Lamp",
            WidgetKind::Slider { .. } => "Slider",
            WidgetKind::Icons { .. } => "Icons",
            WidgetKind::Dropdown { .. } => "Dropdown",
            WidgetKind::List { .. } => "List",
            WidgetKind::Roller { .. } => "Roller",
            WidgetKind::Clock { .. } => "Clock",
            WidgetKind::Qr { .. } => "QR Code",
            WidgetKind::Progress { .. } => "Progress",
            WidgetKind::Chart { .. } => "Chart",
            WidgetKind::Gauge { .. } => "Gauge",
            WidgetKind::Animation { .. } => "Animation",
            WidgetKind::Drawing { .. } => "Drawing",
        }
    }

    pub fn rect(&self) -> (i32, i32, i32, i32) {
        (self.x, self.y, self.w, self.h)
    }
}

fn yes() -> bool {
    true
}

pub fn button(text: &str, event: Event) -> WidgetKind {
    WidgetKind::Button {
        text: text.into(), size: 24.0, fg: [255; 3], bg: [37, 99, 235], pressed_bg: [29, 78, 216],
        radius: 10.0, image: None, goto: None, event, invisible: false,
    }
}

/// Palette entries: (name, kind).
pub fn default_widgets() -> Vec<(&'static str, WidgetKind)> {
    vec![
        ("Button", button("Button", Event::Key { vp: None, key: 1 })),
        ("On/Off Button", button("On/Off", Event::Latch { vp: None })),
        ("+ / − Button", button("+", Event::Adjust { target: String::new(), step: 1, min: 0, max: 100, wrap: false, repeat: true })),
        ("Label", WidgetKind::Label { text: "Label".into(), size: 24.0, color: [255; 3] }),
        ("Panel", WidgetKind::Panel { color: [30, 41, 59], radius: 12.0 }),
        ("Number", WidgetKind::Number { vp: None, int_digits: 3, dec_digits: 1, color: [255; 3], input: None, smooth: false }),
        ("Number Input", WidgetKind::Number { vp: None, int_digits: 3, dec_digits: 0, color: [255; 3], input: Some((0, 999)), smooth: false }),
        ("Big Number", WidgetKind::Number { vp: None, int_digits: 3, dec_digits: 0, color: [255; 3], input: None, smooth: true }),
        ("Text", WidgetKind::Text { vp: None, len: 20, color: [255; 3], input: false, scroll: false, unicode: false }),
        ("Scrolling Text", WidgetKind::Text { vp: None, len: 40, color: [255; 3], input: false, scroll: true, unicode: false }),
        ("Text Input", WidgetKind::Text { vp: None, len: 20, color: [255; 3], input: true, scroll: false, unicode: false }),
        ("Unicode Text", WidgetKind::Text { vp: None, len: 40, color: [255; 3], input: false, scroll: false, unicode: true }),
        ("Toggle", WidgetKind::Toggle { vp: None, on: [34, 197, 94], off: [71, 85, 105], knob: [255; 3], bit: None }),
        ("Lamp", WidgetKind::Lamp { vp: None, bit: None, on: [34, 197, 94], off: [51, 65, 85] }),
        ("Slider", WidgetKind::Slider { vp: None, min: 0, max: 100, track: [71, 85, 105], knob: [255; 3], show_track: true }),
        ("Icons", WidgetKind::Icons { vp: None, images: vec![] }),
        ("Dropdown", WidgetKind::Dropdown {
            vp: None, options: vec!["Option A".into(), "Option B".into(), "Option C".into()],
            size: 22.0, fg: [255; 3], bg: [51, 65, 85],
        }),
        ("List", WidgetKind::List {
            vp: None, items: vec!["Program 1".into(), "Program 2".into(), "Program 3".into(), "Program 4".into()],
            size: 22.0, fg: [255; 3], bg: [30, 41, 59], sel: [37, 99, 235], mcu_len: 0,
        }),
        ("MCU List", WidgetKind::List {
            vp: None, items: (1..=4).map(|i| format!("(MCU row {i})")).collect(),
            size: 22.0, fg: [255; 3], bg: [30, 41, 59], sel: [37, 99, 235], mcu_len: 20,
        }),
        ("Roller", WidgetKind::Roller {
            vp: None, items: (1..=12).map(|m| format!("{m:02}")).collect(), size: 28.0, fg: [255; 3], bg: [30, 41, 59], wrap: true,
        }),
        ("Progress", WidgetKind::Progress {
            vp: None, min: 0, max: 100, fg: [34, 197, 94], bg: [51, 65, 85], border: [148, 163, 184], vertical: false,
        }),
        ("Gauge", WidgetKind::Gauge { vp: None, min: 0, max: 100, sweep: 270, needle: [239, 68, 68], dial: [148, 163, 184], knob: false }),
        ("Knob", WidgetKind::Gauge { vp: None, min: 0, max: 100, sweep: 270, needle: [56, 139, 253], dial: [148, 163, 184], knob: true }),
        ("Chart", WidgetKind::Chart { channel: 0, min: 0, max: 100, step: 2, color: [56, 189, 248], bg: [2, 6, 23] }),
        ("Clock", WidgetKind::Clock { format: "Y-M-D H:Q:S".into(), color: [255; 3], settable: false }),
        ("QR Code", WidgetKind::Qr { vp: None, len: 64, dot: 4 }),
        ("Drawing", WidgetKind::Drawing { vp: None, words: 256, bg: [2, 6, 23] }),
        ("Animation", WidgetKind::Animation { vp: None, images: vec![], period_ms: 200, autoplay: true }),
    ]
}

impl Default for Project {
    fn default() -> Self {
        Self {
            width: 800,
            height: 480,
            page_lib: page_lib(),
            icon_lib: icon_lib(),
            font: None,
            cfg: None,
            unicode_lib: unicode_lib(),
            unicode_max: unicode_max(),
            pages: vec![Page { name: "home".into(), bg: [15, 23, 42], bg_image: None, swipe: (None, None), widgets: vec![] }],
        }
    }
}
