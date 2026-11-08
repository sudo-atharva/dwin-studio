//! Writes examples/showcase/showcase.uwin: one of every widget UWIN Studio has,
//! over 4 screens, with comments. The Arduino sketch next to it drives all of them.
//! cargo run --example showcase

use uwin_studio::project::{button, default_widgets, Event, Page, Project, Widget, WidgetKind};

fn pal(name: &str) -> WidgetKind {
    default_widgets().into_iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("{name}")).1
}

fn w(kind: WidgetKind, name: &str, (x, y, ww, h): (i32, i32, i32, i32), comment: &str) -> Widget {
    Widget { w: ww, h, comment: comment.into(), ..Widget::new(kind, name, x, y) }
}

fn label(name: &str, text: &str, size: f32, at: (i32, i32, i32, i32)) -> Widget {
    w(WidgetKind::Label { text: text.into(), size, color: [203, 213, 225] }, name, at, "")
}

fn panel(name: &str, at: (i32, i32, i32, i32)) -> Widget {
    w(pal("Panel"), name, at, "")
}

/// Title, page dots and previous / next buttons shared by every screen.
fn chrome(i: usize, n: usize, title: &str) -> Vec<Widget> {
    let mut v = vec![label(&format!("title{i}"), title, 28.0, (24, 14, 520, 44))];
    if i > 0 {
        let mut b = w(button("‹", Event::None),&format!("prev{i}"), (604, 14, 80, 44), "Previous screen");
        if let WidgetKind::Button { goto, .. } = &mut b.kind {
            *goto = Some(i - 1);
        }
        v.push(b);
    }
    if i + 1 < n {
        let mut b = w(button("›", Event::None), &format!("next{i}"), (696, 14, 80, 44), "Next screen");
        if let WidgetKind::Button { goto, .. } = &mut b.kind {
            *goto = Some(i + 1);
        }
        v.push(b);
    }
    v
}

/// MCU status line, same VP on every screen (shared address = manual override).
fn status(name: &str, at: (i32, i32, i32, i32)) -> Widget {
    let mut k = pal("Text");
    if let WidgetKind::Text { vp, len, color, .. } = &mut k {
        (*vp, *len, *color) = (Some(0x3000), 32, [134, 239, 172]);
    }
    w(k, name, at, "Status line from the MCU (32 bytes ASCII), on every screen")
}

fn set_text(k: &mut WidgetKind, t: &str) {
    if let WidgetKind::Button { text, .. } = k {
        *text = t.into();
    }
}

fn main() {
    let n = 4;
    let bg = [15, 23, 42];
    let mut pages = vec![];

    // ---------- 0: buttons and switches
    let mut ws = chrome(0, n, "1  Buttons & switches");
    ws.push(panel("card_left", (16, 72, 372, 392)));
    ws.push(panel("card_right", (404, 72, 380, 392)));
    let mut beep = pal("Button");
    set_text(&mut beep, "Beep");
    ws.push(w(beep, "beep", (36, 92, 156, 60), "Push button: sends key 0x0001 each tap"));
    let mut pump = pal("On/Off Button");
    set_text(&mut pump, "Pump");
    ws.push(w(pump, "pump", (212, 92, 156, 60), "On/off button: 1 = pump on (stays pressed), 0 = off"));
    ws.push(label("level_label", "Level %", 20.0, (36, 176, 200, 30)));
    for (name, text, step, x) in [("level_down", "−", -5, 36), ("level_up", "+", 5, 304)] {
        let mut b = button(text, Event::Adjust { target: "level".into(), step, min: 0, max: 100, wrap: false, repeat: true });
        if let WidgetKind::Button { size, .. } = &mut b {
            *size = 30.0;
        }
        ws.push(w(b, name, (x, 212, 64, 56), "+/- button: steps 'level' by 5, hold to repeat"));
    }
    let mut level = pal("Number");
    if let WidgetKind::Number { dec_digits, .. } = &mut level {
        *dec_digits = 0;
    }
    ws.push(w(level, "level", (112, 212, 180, 56), "Level 0..100, changed by the +/- buttons"));
    ws.push(label("dimmer_label", "Dimmer (slider)", 20.0, (36, 290, 300, 30)));
    ws.push(w(pal("Slider"), "dimmer", (36, 326, 332, 44), "Slider 0..100: drag sends it, MCU write moves the knob"));
    ws.push(label("note0", "Swipe left/right to change screens", 16.0, (36, 400, 332, 28)));
    ws.push(status("status_buttons", (300, 22, 290, 28)));

    ws.push(label("fan_label", "Fan", 22.0, (424, 96, 80, 40)));
    ws.push(w(pal("Toggle"), "fan", (520, 96, 90, 44), "Toggle: 1 = fan on; MCU mirrors it to fan_lamp"));
    ws.push(w(pal("Lamp"), "fan_lamp", (634, 102, 32, 32), "Lamp: MCU writes 1 to light it"));
    // Two toggles and two lamps sharing one VP, one bit each (fixed address = manual override).
    for (i, name) in ["heater", "light"].iter().enumerate() {
        let y = 156 + i as i32 * 52;
        ws.push(label(&format!("{name}_label"), &format!("{name} (bit {i})"), 20.0, (424, y, 140, 40)));
        let mut t = pal("Toggle");
        if let WidgetKind::Toggle { vp, bit, .. } = &mut t {
            (*vp, *bit) = (Some(0x2000), Some(i as u8));
        }
        ws.push(w(t, name, (580, y, 80, 40), &format!("Bit {i} of VP 0x2000 (flags word)")));
        let mut l = pal("Lamp");
        if let WidgetKind::Lamp { vp, bit, .. } = &mut l {
            (*vp, *bit) = (Some(0x2000), Some(i as u8));
        }
        ws.push(w(l, &format!("{name}_lamp"), (684, y + 4, 32, 32), &format!("Lamp on bit {i} of VP 0x2000")));
    }
    ws.push(w(pal("Big Number"), "rpm", (424, 264, 200, 70), "Big number with smooth count-up, MCU writes rpm"));
    let mut volts = pal("Number");
    if let WidgetKind::Number { int_digits, dec_digits, color, .. } = &mut volts {
        (*int_digits, *dec_digits, *color) = (2, 2, [250, 204, 21]);
    }
    ws.push(w(volts, "volts", (640, 280, 128, 44), "Number with 2 decimals: MCU sends volts x100"));
    let mut bat = pal("Icons");
    if let WidgetKind::Icons { images, .. } = &mut bat {
        *images = (0..4).map(|i| format!("assets/battery_{i}.png")).collect();
    }
    ws.push(w(bat, "battery", (424, 372, 64, 64), "Icon picked by value: 0 empty .. 3 full"));
    let mut fan = pal("Animation");
    if let WidgetKind::Animation { images, .. } = &mut fan {
        *images = (0..4).map(|i| format!("assets/fan_{i}.png")).collect();
    }
    ws.push(w(fan, "spinner", (512, 372, 64, 64), "Animation: plays its frames on its own"));
    ws.push(w(WidgetKind::Image { path: "assets/logo.png".into() }, "logo", (700, 372, 64, 64), "Static picture"));
    pages.push(Page { name: "buttons".into(), bg, bg_image: None, swipe: (None, Some(1)), widgets: ws });

    // ---------- 1: inputs
    let mut ws = chrome(1, n, "2  Inputs");
    ws.push(status("status_inputs", (300, 22, 290, 28)));
    ws.push(panel("card_in", (16, 72, 380, 392)));
    ws.push(panel("card_lists", (404, 72, 380, 392)));
    ws.push(label("setpoint_label", "Setpoint", 20.0, (32, 88, 130, 44)));
    ws.push(w(pal("Number Input"), "setpoint", (170, 88, 200, 44), "Tap: keypad, 0..999, sent when OK is pressed"));
    ws.push(label("name_label", "Name", 20.0, (32, 144, 130, 44)));
    ws.push(w(pal("Text Input"), "name", (170, 144, 200, 44), "Tap: keyboard, ASCII text sent on OK"));
    let mut mode = pal("Dropdown");
    if let WidgetKind::Dropdown { options, .. } = &mut mode {
        *options = vec!["Auto".into(), "Manual".into(), "Off".into()];
    }
    ws.push(label("mode_label", "Mode", 20.0, (32, 200, 130, 44)));
    ws.push(w(mode, "mode", (170, 200, 200, 44), "Dropdown: 0 Auto, 1 Manual, 2 Off"));
    ws.push(label("hour_label", "Hour", 20.0, (32, 300, 130, 44)));
    let mut hour = pal("Roller");
    if let WidgetKind::Roller { items, .. } = &mut hour {
        *items = (0..24).map(|h| format!("{h:02}")).collect();
    }
    ws.push(w(hour, "hour", (170, 256, 120, 132), "Roller: tap top/bottom to step, value 0..23"));
    let mut clock = pal("Clock");
    if let WidgetKind::Clock { settable, format, .. } = &mut clock {
        (*settable, *format) = (true, "Y-M-D H:Q".into());
    }
    ws.push(w(clock, "clock", (32, 408, 340, 40), "Screen clock; tap to set it with the keypad"));
    ws.push(w(pal("List"), "recipe", (420, 88, 348, 168), "Fixed list: picked row 1..4 sent, 0 = none"));
    ws.push(w(pal("MCU List"), "files", (420, 268, 236, 180), "List whose rows the MCU writes (20 bytes each)"));
    let mut load = pal("Button");
    set_text(&mut load, "Load");
    ws.push(w(load, "load", (668, 268, 100, 56), "Key 0x0001: MCU reads VP_FILES and loads that row"));
    pages.push(Page { name: "inputs".into(), bg, bg_image: None, swipe: (Some(0), Some(2)), widgets: ws });

    // ---------- 2: displays
    let mut ws = chrome(2, n, "3  Displays");
    ws.push(status("status", (24, 80, 360, 28)));
    let mut ticker = pal("Scrolling Text");
    if let WidgetKind::Text { color, .. } = &mut ticker {
        *color = [250, 204, 21];
    }
    ws.push(w(ticker, "ticker", (24, 120, 360, 36), "Marquee text that scrolls inside the box"));
    ws.push(w(pal("Unicode Text"), "greeting", (24, 164, 360, 36), "Unicode text: MCU writes UTF-16BE (any language)"));
    ws.push(label("tank_label", "Tank", 20.0, (24, 212, 80, 30)));
    ws.push(w(pal("Progress"), "tank", (104, 214, 280, 26), "Progress bar 0..100"));
    ws.push(w(pal("Gauge"), "pressure", (24, 256, 200, 200), "Gauge needle 0..100"));
    ws.push(w(pal("Knob"), "volume", (412, 76, 200, 200), "Knob: turn by dragging, sends 0..100"));
    let mut vbar = pal("Progress");
    if let WidgetKind::Progress { vertical, fg, .. } = &mut vbar {
        (*vertical, *fg) = (true, [56, 189, 248]);
    }
    ws.push(w(vbar, "volume_bar", (640, 76, 30, 200), "Vertical progress: MCU echoes the knob here"));
    ws.push(label("note2", "Turn the knob: the MCU copies it to the bar", 16.0, (412, 300, 360, 28)));
    pages.push(Page { name: "displays".into(), bg, bg_image: None, swipe: (Some(1), Some(3)), widgets: ws });

    // ---------- 3: graphics
    let mut ws = chrome(3, n, "4  Graphics");
    ws.push(w(pal("Chart"), "trend", (24, 76, 480, 220), "Chart channel 0: MCU pushes samples 0..100"));
    ws.push(w(pal("QR Code"), "qr", (560, 76, 160, 160), "QR code of the text the MCU writes"));
    ws.push(w(pal("Drawing"), "canvas", (24, 316, 480, 148), "Drawing area: MCU writes basic-graphics commands"));
    ws.push(label("note3", "MCU draws boxes here", 16.0, (520, 316, 260, 28)));
    pages.push(Page { name: "graphics".into(), bg, bg_image: None, swipe: (Some(2), None), widgets: ws });

    let p = Project { pages, ..Project::default() };
    std::fs::write("examples/showcase/showcase.uwin", serde_json::to_string_pretty(&p).unwrap()).unwrap();
    println!("wrote examples/showcase/showcase.uwin");
}
