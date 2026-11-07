//! Compile a `Project` into a `DWIN_SET` and a C header for the MCU.
//!
//! Page ids: page k normal image = k, pressed image = n + k (n = page count).
//! Addresses: VP auto-allocated from 0x1000 (0x10 steps), SP from 0x9000 (0x10 steps).

use crate::control::{show_record, Params, Value, SHOW_KINDS, TOUCH_KINDS};
use crate::icl::Icl;
use crate::picture;
use crate::project::{Action, Project, Rgb, WidgetKind};
use crate::render::{render, to_image, Mode};
use crate::set::DwinSet;
use crate::show::ShowFile;
use crate::touch::{TouchFile, TouchRecord, NONE};
use std::collections::BTreeMap;

pub const VP_START: u16 = 0x1000;
pub const SP_START: u16 = 0x9000;

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
            let (vp, words, has_sp, what) = match &w.kind {
                WidgetKind::Number { vp, .. } => (*vp, 1, true, "number"),
                WidgetKind::Text { vp, len, .. } => (*vp, len.div_ceil(2).max(1), true, "text"),
                WidgetKind::Button { action: Action::ReturnKey { vp, .. }, .. } => (*vp, 1, false, "key"),
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
            // Keys sharing a VP is a normal DGUS pattern (several buttons, one variable).
            let shared_ok = a.what == "key" && b.what == "key" && a.vp == b.vp;
            if !shared_ok && overlaps(a, b.vp, b.words) {
                warnings.push(format!("VP clash {:#06x}: '{}' and '{}'", b.vp, a.name, b.name));
            }
        }
        if a.vp + a.words > SP_START {
            warnings.push(format!("'{}' VP {:#06x} runs into SP area (0x9000+)", a.name, a.vp));
        }
    }
    (all, warnings)
}

fn n(v: u32) -> Value {
    Value::Num(v)
}

pub fn build(p: &Project) -> Result<Output, Box<dyn std::error::Error>> {
    let (allocs, mut warnings) = allocate(p);
    let find = |pi: usize, wi: usize| allocs.iter().find(|a| a.page == pi && a.widget == wi);
    let npages = p.pages.len();
    let kind = |list: &'static [crate::control::Kind], name: &str| list.iter().find(|k| k.name == name).unwrap();

    let mut icons = vec![None; 2 * npages];
    let mut touch = Vec::new();
    let mut show = ShowFile { flag: 0x10, max_page: (2 * npages - 1) as u16, pages: BTreeMap::new() };

    for (pi, page) in p.pages.iter().enumerate() {
        let has_buttons = page.widgets.iter().any(|w| matches!(w.kind, WidgetKind::Button { .. }));
        icons[pi] = Some(picture::encode(&to_image(&render(page, p.width, p.height, Mode::Normal)))?);
        if has_buttons {
            icons[npages + pi] = Some(picture::encode(&to_image(&render(page, p.width, p.height, Mode::Pressed)))?);
        }

        for (wi, w) in page.widgets.iter().enumerate() {
            let (x0, y0) = (clamp16(w.x), clamp16(w.y));
            let (x1, y1) = (clamp16(w.x + w.w).min(p.width as u32 - 1), clamp16(w.y + w.h).min(p.height as u32 - 1));
            match &w.kind {
                WidgetKind::Button { action, .. } => {
                    let mut r = TouchRecord {
                        page_raw: pi as u16, x0: x0 as u16, y0: y0 as u16, x1: x1 as u16, y1: y1 as u16,
                        next_page: NONE, pressed_page: (npages + pi) as u16, code: 0, ext: vec![],
                    };
                    match action {
                        Action::None => {}
                        Action::GoTo(t) if *t < npages => r.next_page = *t as u16,
                        Action::GoTo(t) => warnings.push(format!("'{}': page {t} does not exist", w.name)),
                        Action::ReturnKey { key, .. } => {
                            let a = find(pi, wi).unwrap();
                            let params: Params = vec![("vp", n(a.vp as u32)), ("key", n(*key as u32))];
                            r.set_params(kind(TOUCH_KINDS, "return_key"), &params);
                        }
                    }
                    touch.push(r);
                }
                WidgetKind::Number { int_digits, dec_digits, color, .. } => {
                    let a = find(pi, wi).unwrap();
                    let params: Params = vec![
                        ("sp", n(a.sp.unwrap() as u32)), ("vp", n(a.vp as u32)), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("font_size", n(w.h.clamp(0, 255) as u32)),
                        ("int_digits", n(*int_digits as u32)), ("dec_digits", n(*dec_digits as u32)),
                    ];
                    show.pages.entry(pi as u16).or_default().push(show_record(kind(SHOW_KINDS, "number"), &params));
                }
                WidgetKind::Text { len, color, .. } => {
                    let a = find(pi, wi).unwrap();
                    let size = w.h.clamp(0, 255) as u32;
                    let params: Params = vec![
                        ("sp", n(a.sp.unwrap() as u32)), ("vp", n(a.vp as u32)), ("x", n(x0)), ("y", n(y0)),
                        ("color", n(rgb565(*color))), ("box_x0", n(x0)), ("box_y0", n(y0)), ("box_x1", n(x1)),
                        ("box_y1", n(y1)), ("length", n(*len as u32)), ("font_w", n(size)), ("font_h", n(size)),
                        ("encode", n(0x80)), // ASCII, auto character spacing
                    ];
                    show.pages.entry(pi as u16).or_default().push(show_record(kind(SHOW_KINDS, "text"), &params));
                }
                WidgetKind::Label { .. } | WidgetKind::Panel { .. } => {}
            }
        }
    }

    let mut set = DwinSet {
        touch: Some(TouchFile { records: touch }),
        show: Some(show),
        icls: BTreeMap::new(),
        raw: BTreeMap::new(),
    };
    // Plain JPEGs too, like DGUS puts in DWIN_SET (NN_name.jpg).
    for (id, icon) in icons.iter().enumerate() {
        if let Some(icon) = icon {
            let name = if id < npages { &p.pages[id].name } else { "pressed" };
            set.raw.insert(format!("{id:02}_{}.jpg", sanitize(name).to_lowercase()), icon.to_jpeg()?);
        }
    }
    set.icls.insert("32.icl".into(), Icl { icons, align: 4, pad_to: 0 });
    set.raw.insert("22_Config.bin".into(), vec![0; 131076]);

    let header = header(p, &allocs);
    Ok(Output { set, header, allocs, warnings })
}

pub fn sanitize(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn header(p: &Project, allocs: &[Alloc]) -> String {
    let mut h = String::from("// Generated by dwin-studio. Do not edit; rebuild instead.\n#pragma once\n\n");
    for (i, page) in p.pages.iter().enumerate() {
        h += &format!("#define PAGE_{:<24} {i}\n", sanitize(&page.name));
    }
    h += "\n";
    for a in allocs {
        let name = sanitize(&a.name);
        h += &format!("#define VP_{name:<26} {:#06x}  // {}, {} word(s)\n", a.vp, a.what, a.words);
        if let Some(sp) = a.sp {
            h += &format!("#define SP_{name:<26} {sp:#06x}  // move/recolor at runtime\n");
        }
        if let WidgetKind::Button { action: Action::ReturnKey { key, .. }, .. } = &p.pages[a.page].widgets[a.widget].kind {
            h += &format!("#define KEY_{name:<25} {key:#06x}\n");
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::show_params;
    use crate::project::{default_widgets, Page, Widget};

    fn sample() -> Project {
        let mut kinds = default_widgets().into_iter();
        let (_, mut button) = kinds.next().unwrap();
        if let WidgetKind::Button { action, .. } = &mut button {
            *action = Action::ReturnKey { vp: None, key: 1 };
        }
        let mut goto = default_widgets().remove(0).1;
        if let WidgetKind::Button { action, .. } = &mut goto {
            *action = Action::GoTo(1);
        }
        let number = default_widgets().remove(3).1;
        let text = default_widgets().remove(4).1;
        Project {
            width: 1024,
            height: 600,
            pages: vec![
                Page { name: "home".into(), bg: [10, 20, 30], widgets: vec![
                    Widget::new(button, "start", 20, 20),
                    Widget::new(goto, "settings", 20, 100),
                    Widget::new(number, "temp", 300, 20),
                    Widget::new(text, "status", 300, 100),
                ]},
                Page { name: "settings".into(), bg: [0, 0, 0], widgets: vec![] },
            ],
        }
    }

    #[test]
    fn build_parses_back() {
        let out = build(&sample()).unwrap();
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let dir = std::env::temp_dir().join("dwin_studio_build_test");
        let _ = std::fs::remove_dir_all(&dir);
        out.set.write(&dir).unwrap();
        let back = DwinSet::load(&dir).unwrap();

        let icl = &back.icls["32.icl"];
        assert_eq!(icl.icons.iter().flatten().count(), 3, "2 pages + 1 pressed");
        assert_eq!((icl.icons[0].as_ref().unwrap().width, icl.icons[0].as_ref().unwrap().height), (1024, 600));

        let t = &back.touch.unwrap().records;
        assert_eq!(t.len(), 2);
        let key = t[0].params().unwrap();
        assert!(key.contains(&("vp", Value::Num(0x1000))) && key.contains(&("key", Value::Num(1))));
        assert_eq!((t[0].pressed_page, t[1].next_page), (2, 1));

        let recs = &back.show.unwrap().pages[&0];
        let num = show_params(&recs[0]).unwrap();
        assert!(num.contains(&("vp", Value::Num(0x1010))) && num.contains(&("sp", Value::Num(0x9000))));
        assert!(out.header.contains("#define VP_TEMP") && out.header.contains("#define PAGE_SETTINGS"));
    }

    #[test]
    fn clash_reported() {
        let mut p = sample();
        if let WidgetKind::Number { vp, .. } = &mut p.pages[0].widgets[2].kind {
            *vp = Some(0x2000);
        }
        if let WidgetKind::Text { vp, .. } = &mut p.pages[0].widgets[3].kind {
            *vp = Some(0x2000);
        }
        let (_, w) = allocate(&p);
        assert_eq!(w.len(), 1, "{w:?}");
    }
}
