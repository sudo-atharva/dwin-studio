//! Compile a `Project` into a `DWIN_SET` and a C header for the MCU.
//!
//! Page ids: page k normal image = k, pressed image = n + k (n = page count) in 32.icl.
//! Icons (toggle states, slider knobs, icon states) are cut from the page and go in 48.icl.
//! Addresses: VP auto-allocated from 0x1000 (0x10 steps), SP from 0x9000 (0x10 steps).

use crate::control::{show_record, Kind, Params, Value, SHOW_KINDS, TOUCH_KINDS};
use crate::icl::{Icl, Icon};
use crate::picture;
use crate::project::{Event, Project, Rgb, Widget, WidgetKind};
use crate::render::{icon, render, to_image, Mode};
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

/// `base` = folder that relative image paths are resolved against (the project file's folder).
pub fn build(p: &Project, base: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    let (allocs, mut warnings) = allocate(p);
    let find = |pi: usize, wi: usize| allocs.iter().find(|a| a.page == pi && a.widget == wi);
    let by_name = |name: &str| allocs.iter().find(|a| a.name == name);
    let npages = p.pages.len();

    let mut pages = vec![None; 2 * npages];
    let mut icons = Icons(vec![]);
    let mut touch = Vec::new();
    let mut show = ShowFile { flag: 0x10, max_page: (2 * npages - 1) as u16, pages: BTreeMap::new() };

    for (pi, page) in p.pages.iter().enumerate() {
        let has_buttons = page.widgets.iter().any(|w| matches!(w.kind, WidgetKind::Button { .. }));
        pages[pi] = Some(picture::encode(&to_image(&render(page, p.width, p.height, Mode::Normal, base)))?);
        if has_buttons {
            pages[npages + pi] = Some(picture::encode(&to_image(&render(page, p.width, p.height, Mode::Pressed, base)))?);
        }
        let base_px = render(page, p.width, p.height, Mode::Base, base);
        let shows = show.pages.entry(pi as u16).or_default();

        for (wi, w) in page.widgets.iter().enumerate() {
            let (x0, y0) = (clamp16(w.x), clamp16(w.y));
            let (x1, y1) = (clamp16(w.x + w.w).min(p.width as u32 - 1), clamp16(w.y + w.h).min(p.height as u32 - 1));
            let area = |r: &mut TouchRecord| (r.x0, r.y0, r.x1, r.y1) = (x0 as u16, y0 as u16, x1 as u16, y1 as u16);
            let mut rec = TouchRecord {
                page_raw: pi as u16, x0: 0, y0: 0, x1: 0, y1: 0, next_page: NONE, pressed_page: NONE, code: 0, ext: vec![],
            };
            area(&mut rec);
            let a = find(pi, wi);
            match &w.kind {
                WidgetKind::Button { goto, event, .. } => {
                    rec.pressed_page = (npages + pi) as u16;
                    match goto {
                        Some(t) if *t < npages => rec.next_page = *t as u16,
                        Some(t) => warnings.push(format!("'{}': page {t} does not exist", w.name)),
                        None => {}
                    }
                    match event {
                        Event::None => {}
                        Event::Key { key, .. } => {
                            let params: Params = vec![("vp", n(a.unwrap().vp as u32)), ("key", n(*key as u32))];
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
                WidgetKind::Number { int_digits, dec_digits, color, .. } => {
                    let a = a.unwrap();
                    let params: Params = vec![
                        ("sp", n(a.sp.unwrap() as u32)), ("vp", n(a.vp as u32)), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("font_size", n(w.h.clamp(0, 255) as u32)),
                        ("int_digits", n(*int_digits as u32)), ("dec_digits", n(*dec_digits as u32)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "number"), &params));
                }
                WidgetKind::Text { len, color, .. } => {
                    let a = a.unwrap();
                    let size = w.h.clamp(0, 255) as u32;
                    let params: Params = vec![
                        ("sp", n(a.sp.unwrap() as u32)), ("vp", n(a.vp as u32)), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("box_x0", n(x0)), ("box_y0", n(y0)), ("box_x1", n(x1)),
                        ("box_y1", n(y1)), ("length", n(*len as u32)), ("font_w", n(size)), ("font_h", n(size)),
                        ("encode", n(0x80)), // ASCII, auto character spacing
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "text"), &params));
                }
                WidgetKind::Toggle { .. } | WidgetKind::Icons { .. } => {
                    let a = a.unwrap();
                    let states = match &w.kind {
                        WidgetKind::Icons { images, .. } => images.len().max(1),
                        _ => 2,
                    };
                    let first = icons.0.len() as u32;
                    for s in 0..states {
                        icons.add(&icon(&base_px, w, s, (w.w, w.h), base))?;
                    }
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", n(a.vp as u32)), ("x", n(x0)), ("y", n(y0)),
                        ("v_min", n(0)), ("v_max", n(states as u32 - 1)), ("icon_min", n(first)),
                        ("icon_max", n(first + states as u32 - 1)), ("lib", n(ICON_LIB as u32)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "var_icon"), &params));
                    if matches!(w.kind, WidgetKind::Toggle { .. }) {
                        // Same as DGUS demo 2: increment, step 1, 0..1, wrap.
                        let params: Params = vec![
                            ("vp", n(a.vp as u32)), ("adj_mode", n(1)), ("return_mode", n(1)),
                            ("step", n(1)), ("min", n(0)), ("max", n(1)),
                        ];
                        rec.set_params(kind(TOUCH_KINDS, "increment"), &params);
                        touch.push(rec);
                    }
                }
                WidgetKind::Slider { min, max, .. } => {
                    let a = a.unwrap();
                    let d = w.h;
                    // Knob background is cut from the middle of the track, where it looks like everywhere else.
                    let mut mid = w.clone();
                    (mid.x, mid.w) = (w.x + (w.w - d) / 2, d);
                    let knob = icons.add(&icon(&base_px, &mid, 0, (d, d), base))?;
                    let (kx0, kx1) = (x0, clamp16(w.x + w.w - d));
                    let params: Params = vec![
                        ("sp", n(0xFFFF)), ("vp", n(a.vp as u32)), ("v_begin", n(*min as u32)), ("v_end", n(*max as u32)),
                        ("x_begin", n(kx0)), ("x_end", n(kx1)), ("icon", n(knob)), ("y", n(y0)),
                        ("lib", n(ICON_LIB as u32)),
                    ];
                    shows.push(show_record(kind(SHOW_KINDS, "slider_show"), &params));
                    // Touch: value follows the knob centre between the track ends.
                    let (cx0, cx1) = (clamp16(w.x + d / 2), clamp16(w.x + w.w - d / 2));
                    let params: Params = vec![
                        ("vp", n(a.vp as u32)), ("adj_mode", n(0)), ("x0", n(cx0)), ("y0", n(y0)), ("x1", n(cx1)),
                        ("y1", n(y1)), ("v_begin", n(*min as u32)), ("v_end", n(*max as u32)),
                    ];
                    rec.set_params(kind(TOUCH_KINDS, "slider"), &params);
                    touch.push(rec);
                }
                WidgetKind::Label { .. } | WidgetKind::Panel { .. } | WidgetKind::Image { .. } => {}
            }
        }
    }
    show.pages.retain(|_, v| !v.is_empty());

    let mut set = DwinSet { touch: Some(TouchFile { records: touch }), show: Some(show), icls: BTreeMap::new(), raw: BTreeMap::new() };
    // Plain JPEGs too, like DGUS puts in DWIN_SET (NN_name.jpg).
    for (id, img) in pages.iter().enumerate() {
        if let Some(img) = img {
            let name = if id < npages { &p.pages[id].name } else { "pressed" };
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
    set.raw.insert("22_Config.bin".into(), vec![0; 131076]);

    let header = header(p, &allocs);
    let map = ui_map(p, &allocs);
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
        WidgetKind::Number { int_digits, dec_digits, .. } => format!(
            "NUMBER  VP {:#06x} (int16, {int_digits}.{dec_digits} digits), SP {:#06x}",
            vp(),
            a.and_then(|a| a.sp).unwrap_or(0)
        ),
        WidgetKind::Text { len, .. } => {
            format!("TEXT    VP {:#06x} ({len} bytes ASCII), SP {:#06x}", vp(), a.and_then(|a| a.sp).unwrap_or(0))
        }
        WidgetKind::Toggle { .. } => format!("TOGGLE  VP {:#06x} = 0 off / 1 on; tap flips it and sends it", vp()),
        WidgetKind::Slider { min, max, .. } => format!("SLIDER  VP {:#06x} = {min}..{max}; drag sends it, MCU write moves knob", vp()),
        WidgetKind::Icons { images, .. } => format!("ICONS   VP {:#06x} = 0..{} picks the picture", vp(), images.len().max(1) - 1),
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
