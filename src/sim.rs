//! Run a project the way the screen does: VP memory, touches, DGUS UART frames.
//!
//! Screen -> MCU: `5A A5 len 83 VP n data` on touch. MCU -> screen: `82` write (acked
//! `4F 4B`), `83` read (answered), page switch through VP 0x0084, curves through 0x0310.
//! ponytail: no CRC mode; add when a project enables CRC in T5LCFG.

use crate::build::{allocate, Alloc};
use crate::project::{Event, Project, Widget, WidgetKind};
use crate::render::{self, draw_state, frect, gauge_geom, line, needle_len_width, polar, rounded, text, Mode};
use std::path::Path;
use std::time::Instant;
use tiny_skia::Pixmap;

/// Input the UI has to collect from the user (the screen would pop a keypad/list).
#[derive(Debug, Clone, PartialEq)]
pub enum Prompt {
    Number { vp: u16, dec: u8, min: i32, max: i32 },
    Text { vp: u16, len: u16 },
    Menu { vp: u16, options: Vec<String> },
}

pub struct Sim {
    pub page: usize,
    /// 64K words of VP memory, big-endian like the screen.
    mem: Vec<u8>,
    pub allocs: Vec<Alloc>,
    /// Frames for the MCU, drained by the caller (serial port).
    pub out: Vec<Vec<u8>>,
    pub log: Vec<String>,
    rx: Vec<u8>,
    curves: [Vec<i16>; 8],
    /// Widget index being held (slider drag, button highlight).
    pub held: Option<usize>,
    /// Where the finger went down (swipe detection).
    down_x: i32,
    start: Instant,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

impl Sim {
    pub fn new(p: &Project) -> Self {
        let mut s = Sim {
            page: 0, mem: vec![0; 0x20000], allocs: allocate(p).0, out: vec![], log: vec![], rx: vec![],
            curves: Default::default(), held: None, down_x: 0, start: Instant::now(),
        };
        for a in s.allocs.clone() {
            if let WidgetKind::Animation { autoplay: true, .. } = p.pages[a.page].widgets[a.widget].kind {
                s.set(a.vp, 1);
            }
        }
        s
    }

    pub fn get(&self, vp: u16) -> u16 {
        let i = vp as usize * 2;
        u16::from_be_bytes([self.mem[i], self.mem[i + 1]])
    }

    /// Write memory without telling the MCU (what an MCU write does).
    pub fn set(&mut self, vp: u16, v: u16) {
        self.write_bytes(vp, &v.to_be_bytes());
    }

    fn write_bytes(&mut self, vp: u16, b: &[u8]) {
        let i = vp as usize * 2;
        let end = (i + b.len()).min(self.mem.len());
        self.mem[i..end].copy_from_slice(&b[..end - i]);
    }

    /// ASCII at `vp`, up to `len` bytes, ending at 0x00 or 0xFF.
    pub fn text_at(&self, vp: u16, len: u16) -> String {
        let i = vp as usize * 2;
        self.mem[i..(i + len as usize).min(self.mem.len())]
            .iter()
            .take_while(|&&c| c != 0 && c != 0xFF)
            .map(|&c| c as char)
            .collect()
    }

    fn vp(&self, page: usize, wi: usize) -> Option<u16> {
        self.allocs.iter().find(|a| a.page == page && a.widget == wi).map(|a| a.vp)
    }

    fn emit(&mut self, frame: Vec<u8>) {
        self.log.push(format!("→ MCU  {}", hex(&frame)));
        self.out.push(frame);
    }

    /// Tell the MCU a VP changed: 5A A5 len 83 VP n words.
    fn send(&mut self, vp: u16) {
        self.send_words(vp, 1);
    }

    fn send_words(&mut self, vp: u16, n: u8) {
        let i = vp as usize * 2;
        let data = self.mem[i..i + 2 * n as usize].to_vec();
        let mut f = vec![0x5A, 0xA5, 4 + data.len() as u8, 0x83, (vp >> 8) as u8, vp as u8, n];
        f.extend(data);
        self.emit(f);
    }

    fn interactive(w: &Widget) -> bool {
        matches!(
            w.kind,
            WidgetKind::Button { .. } | WidgetKind::Toggle { .. } | WidgetKind::Slider { .. } | WidgetKind::Dropdown { .. }
                | WidgetKind::Number { input: Some(_), .. } | WidgetKind::Text { input: true, .. }
        )
    }

    /// Finger down at screen pixel (x, y).
    pub fn press(&mut self, p: &Project, x: i32, y: i32) -> Option<Prompt> {
        self.down_x = x;
        let ws = &p.pages[self.page].widgets;
        let wi = ws.iter().rposition(|w| Self::interactive(w) && x >= w.x && y >= w.y && x < w.x + w.w && y < w.y + w.h)?;
        let w = &ws[wi];
        let vp = self.vp(self.page, wi);
        self.held = Some(wi);
        match &w.kind {
            WidgetKind::Button { event, goto, .. } => {
                match event {
                    Event::None => {}
                    Event::Key { key, .. } => {
                        self.set(vp?, *key);
                        self.send(vp?);
                    }
                    Event::Adjust { target, step, min, max, wrap, .. } => {
                        let t = self.allocs.iter().find(|a| &a.name == target)?.vp;
                        let v = self.get(t) as i32 + *step as i32;
                        let (lo, hi) = (*min as i32, *max as i32);
                        let v = match (v < lo, v > hi) {
                            (true, _) => if *wrap { hi } else { lo },
                            (_, true) => if *wrap { lo } else { hi },
                            _ => v,
                        };
                        self.set(t, v as u16);
                        self.send(t);
                    }
                }
                if let Some(g) = goto.filter(|g| *g < p.pages.len()) {
                    self.page = g;
                    self.held = None;
                }
            }
            WidgetKind::Toggle { bit: Some(b), .. } => {
                let v = self.get(vp?) ^ (1 << (b % 16));
                self.set(vp?, v);
                self.send(vp?);
            }
            WidgetKind::Toggle { .. } => {
                let v = (self.get(vp?) == 0) as u16;
                self.set(vp?, v);
                self.send(vp?);
            }
            WidgetKind::Slider { .. } => self.drag(p, x),
            WidgetKind::Number { dec_digits, input: Some((min, max)), .. } => {
                return Some(Prompt::Number { vp: vp?, dec: *dec_digits, min: *min, max: *max })
            }
            WidgetKind::Text { len, .. } => return Some(Prompt::Text { vp: vp?, len: *len }),
            WidgetKind::Dropdown { options, .. } => return Some(Prompt::Menu { vp: vp?, options: options.clone() }),
            _ => {}
        }
        None
    }

    /// Finger moved: only sliders follow it.
    pub fn drag(&mut self, p: &Project, x: i32) {
        let Some(wi) = self.held else { return };
        let Some(w) = p.pages[self.page].widgets.get(wi) else { return };
        if let WidgetKind::Slider { min, max, .. } = w.kind {
            let (x0, x1) = (w.x + w.h / 2, w.x + w.w - w.h / 2);
            let f = ((x - x0) as f32 / (x1 - x0).max(1) as f32).clamp(0.0, 1.0);
            let v = min as f32 + f * (max as f32 - min as f32);
            if let Some(vp) = self.vp(self.page, wi) {
                self.set(vp, v.round() as u16);
            }
        }
    }

    /// Finger up at x: a slider reports its final value; a long sideways move on a page
    /// with swipe targets changes page.
    pub fn release(&mut self, p: &Project, x: i32) {
        let page = &p.pages[self.page];
        let dx = x - self.down_x;
        let target = if dx < -50 { page.swipe.1 } else if dx > 50 { page.swipe.0 } else { None };
        if let Some(t) = target.filter(|&t| t < p.pages.len() && self.held.is_none()) {
            self.page = t;
            return;
        }
        if let Some(wi) = self.held.take() {
            if matches!(p.pages[self.page].widgets.get(wi).map(|w| &w.kind), Some(WidgetKind::Slider { .. })) {
                if let Some(vp) = self.vp(self.page, wi) {
                    self.send(vp);
                }
            }
        }
    }

    /// Keypad OK with the typed text ("12.5"). Out of range = rejected like the screen does.
    pub fn answer_number(&mut self, vp: u16, dec: u8, min: i32, max: i32, typed: &str) -> bool {
        let Ok(f) = typed.trim().parse::<f64>() else { return false };
        let v = (f * 10f64.powi(dec as i32)).round() as i32;
        if v < min || v > max {
            return false;
        }
        self.set(vp, v as i16 as u16);
        self.send(vp);
        true
    }

    /// Keyboard OK: text then FF FF, like DGUS text input.
    pub fn answer_text(&mut self, vp: u16, len: u16, s: &str) {
        let mut b: Vec<u8> = s.bytes().take(len as usize).collect();
        b.extend([0xFF, 0xFF]);
        if b.len() % 2 == 1 {
            b.push(0xFF);
        }
        self.write_bytes(vp, &b);
        self.send_words(vp, (b.len() / 2) as u8);
    }

    pub fn answer_menu(&mut self, vp: u16, i: usize) {
        self.set(vp, i as u16);
        self.send(vp);
    }

    /// Bytes from the MCU. Complete frames are applied; the rest waits for more.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.rx.extend_from_slice(bytes);
        loop {
            match self.rx.windows(2).position(|w| w == [0x5A, 0xA5]) {
                Some(0) => {}
                Some(i) => drop(self.rx.drain(..i)),
                None => {
                    let keep = self.rx.last() == Some(&0x5A);
                    self.rx.clear();
                    if keep {
                        self.rx.push(0x5A);
                    }
                    return;
                }
            }
            if self.rx.len() < 3 || self.rx.len() < 3 + self.rx[2] as usize {
                return;
            }
            let frame: Vec<u8> = self.rx.drain(..3 + self.rx[2] as usize).collect();
            self.log.push(format!("← MCU  {}", hex(&frame)));
            self.apply(&frame[3..]);
        }
    }

    fn apply(&mut self, f: &[u8]) {
        if f.len() < 3 {
            return;
        }
        let vp = u16::from_be_bytes([f[1], f[2]]);
        match f[0] {
            0x82 => {
                let data = &f[3..];
                self.write_bytes(vp, data);
                if vp == 0x0084 && data.len() >= 4 && data[..2] == [0x5A, 0x01] {
                    self.page = u16::from_be_bytes([data[2], data[3]]) as usize;
                }
                if vp == 0x0310 && data.len() >= 4 && data[..2] == [0x5A, 0xA5] {
                    self.curve_data(data);
                }
                self.emit(vec![0x5A, 0xA5, 0x03, 0x82, 0x4F, 0x4B]);
            }
            0x83 if f.len() >= 4 => self.send_words(vp, f[3]),
            _ => {}
        }
    }

    /// 5A A5 <channels> 00 then per channel: <id> <count> <int16 samples>.
    fn curve_data(&mut self, d: &[u8]) {
        let mut i = 4;
        for _ in 0..d[2] {
            let (Some(&ch), Some(&n)) = (d.get(i), d.get(i + 1)) else { return };
            i += 2;
            for _ in 0..n {
                let Some(s) = d.get(i..i + 2) else { return };
                let c = &mut self.curves[ch as usize % 8];
                c.push(i16::from_be_bytes([s[0], s[1]]));
                if c.len() > 4096 {
                    c.drain(..1024);
                }
                i += 2;
            }
        }
    }

    /// The current page as the screen would show it now.
    pub fn render(&self, p: &Project, base: &Path) -> Pixmap {
        let page = &p.pages[self.page.min(p.pages.len() - 1)];
        let mut px = render::render(page, p.width, p.height, Mode::Base, base);
        let ms = self.start.elapsed().as_millis() as u64;
        for (wi, w) in page.widgets.iter().enumerate() {
            let v = self.vp(self.page, wi).map_or(0, |vp| self.get(vp));
            let frac = |min: f32, max: f32| ((v as f32 - min) / (max - min).max(1.0)).clamp(0.0, 1.0);
            let (x, y, ww, hh) = frect(w);
            match &w.kind {
                WidgetKind::Toggle { bit: Some(b), .. } | WidgetKind::Lamp { bit: Some(b), .. } => {
                    draw_state(&mut px, w, (v >> (b % 16) & 1) as usize, base)
                }
                WidgetKind::Toggle { .. } | WidgetKind::Lamp { .. } => draw_state(&mut px, w, (v != 0) as usize, base),
                WidgetKind::Icons { .. } | WidgetKind::Dropdown { .. } => draw_state(&mut px, w, v as usize, base),
                WidgetKind::Animation { images, period_ms, .. } => {
                    let n = images.len().max(1) as u64;
                    let f = if v == 1 { ms / (*period_ms).max(8) as u64 % n } else { 0 };
                    draw_state(&mut px, w, f as usize, base);
                }
                WidgetKind::Slider { min, max, .. } => {
                    let mut k = w.clone();
                    k.x += (frac(*min as f32, *max as f32) * (w.w - w.h) as f32) as i32;
                    draw_state(&mut px, &k, 0, base);
                }
                WidgetKind::Number { dec_digits, color, .. } => {
                    let n = v as i16 as f64 / 10f64.powi(*dec_digits as i32);
                    text(&mut px, &format!("{n:.*}", *dec_digits as usize), x, y, hh, *color);
                }
                WidgetKind::Text { len, color, .. } => {
                    let s = self.vp(self.page, wi).map_or(String::new(), |vp| self.text_at(vp, *len));
                    text(&mut px, &s, x, y, hh, *color);
                }
                WidgetKind::Clock { format, color } => text(&mut px, &clock(format), x, y, hh, *color),
                WidgetKind::Progress { min, max, fg, bg, border, vertical, .. } => {
                    let f = frac(*min as f32, *max as f32);
                    rounded(&mut px, (x, y, ww, hh), 0.0, *border);
                    rounded(&mut px, (x + 1.0, y + 1.0, ww - 2.0, hh - 2.0), 0.0, *bg);
                    let r = if *vertical {
                        (x + 1.0, y + 1.0 + (hh - 2.0) * (1.0 - f), ww - 2.0, (hh - 2.0) * f)
                    } else {
                        (x + 1.0, y + 1.0, (ww - 2.0) * f, hh - 2.0)
                    };
                    if f > 0.0 {
                        rounded(&mut px, r, 0.0, *fg);
                    }
                }
                WidgetKind::Gauge { min, max, sweep, needle, .. } => {
                    let (cx, cy, r, start) = gauge_geom(w, *sweep);
                    let (len, nw) = needle_len_width(r);
                    let a = start + *sweep as f32 * frac(*min as f32, *max as f32);
                    line(&mut px, &[(cx, cy), polar(cx, cy, len, a)], nw, *needle);
                }
                WidgetKind::Chart { channel, min, max, step, color, .. } => {
                    let c = &self.curves[*channel as usize % 8];
                    let step = (*step).max(1) as f32;
                    let n = ((ww / step) as usize + 1).min(c.len());
                    let span = (*max as f32 - *min as f32).max(1.0);
                    let pts: Vec<_> = c[c.len() - n..]
                        .iter()
                        .enumerate()
                        .map(|(i, &s)| (x + ww - (n - 1 - i) as f32 * step, y + hh * (1.0 - (s as f32 - *min as f32) / span)))
                        .collect();
                    if pts.len() > 1 {
                        line(&mut px, &pts, 2.0, *color);
                    }
                }
                WidgetKind::Qr { len, .. } => {
                    rounded(&mut px, (x, y, ww, hh), 0.0, [255; 3]);
                    let s = self.vp(self.page, wi).map_or(String::new(), |vp| self.text_at(vp, *len));
                    text(&mut px, if s.is_empty() { "QR" } else { &s }, x + 4.0, y + 4.0, 12.0, [0; 3]);
                }
                _ => {}
            }
        }
        px
    }
}

/// System clock in UTC, in the DGUS RTC format letters.
/// ponytail: UTC only (no tz crate); the real screen shows whatever the MCU set.
pub fn clock(format: &str) -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let days = secs.div_euclid(86400);
    let t = secs.rem_euclid(86400);
    // Civil date from day count (H. Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    let wd = ["THU", "FRI", "SAT", "SUN", "MON", "TUE", "WED"][days.rem_euclid(7) as usize];
    format
        .chars()
        .map(|c| match c {
            'Y' => format!("{y:04}"),
            'M' => format!("{m:02}"),
            'D' => format!("{d:02}"),
            'H' => format!("{:02}", t / 3600),
            'Q' => format!("{:02}", t / 60 % 60),
            'S' => format!("{:02}", t % 60),
            'W' => wd.into(),
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{button, default_widgets, Page};

    fn kind(name: &str) -> WidgetKind {
        default_widgets().into_iter().find(|(n, _)| *n == name).unwrap().1
    }

    #[test]
    fn touches_and_mcu_frames() {
        let mut go = button("Next", Event::None);
        if let WidgetKind::Button { goto, .. } = &mut go {
            *goto = Some(1);
        }
        let p = Project {
            width: 800,
            height: 480,
            pages: vec![
                Page { name: "a".into(), bg: [0; 3], bg_image: None, swipe: (None, None), widgets: vec![
                    Widget::new(kind("Button"), "key", 0, 0),          // VP 0x1000, key 1
                    Widget::new(kind("Toggle"), "t", 200, 0),          // VP 0x1010
                    Widget::new(go, "go", 0, 200),
                ]},
                Page { name: "b".into(), bg: [0; 3], bg_image: None, swipe: (None, None), widgets: vec![Widget::new(kind("Number Input"), "n", 0, 0)] },
            ],
        };
        let mut s = Sim::new(&p);
        s.press(&p, 10, 10);
        assert_eq!(s.out.pop().unwrap(), [0x5A, 0xA5, 0x06, 0x83, 0x10, 0x00, 0x01, 0x00, 0x01]);
        s.press(&p, 210, 10);
        assert_eq!(s.get(0x1010), 1);
        s.press(&p, 10, 210);
        assert_eq!(s.page, 1);
        assert_eq!(s.press(&p, 5, 5), Some(Prompt::Number { vp: 0x1020, dec: 0, min: 0, max: 999 }));
        assert!(!s.answer_number(0x1020, 0, 0, 999, "1000"));
        assert!(s.answer_number(0x1020, 0, 0, 999, "42"));

        // MCU: page switch split across two reads, then a read request.
        s.out.clear();
        s.feed(&[0x5A, 0xA5, 0x07, 0x82, 0x00, 0x84]);
        s.feed(&[0x5A, 0x01, 0x00, 0x00, 0x5A, 0xA5, 0x04, 0x83, 0x10, 0x20, 0x01]);
        assert_eq!(s.page, 0);
        assert_eq!(s.out[0], [0x5A, 0xA5, 0x03, 0x82, 0x4F, 0x4B]);
        assert_eq!(s.out[1], [0x5A, 0xA5, 0x06, 0x83, 0x10, 0x20, 0x01, 0x00, 42]);
    }
}
