//! Turn an existing DGUS `DWIN_SET` into an editable project.
//!
//! Page pictures become screen backgrounds, touch areas become invisible buttons (same
//! VP and key, so MCU firmware keeps working), and common display controls become widgets.
//! Anything not understood is listed in the report instead of being guessed.

use crate::control::{show_kind, show_params, Value};
use crate::project::{Event, Page, Project, Widget, WidgetKind};
use crate::set::DwinSet;
use std::collections::BTreeMap;
use std::path::Path;

fn num(p: &[(&'static str, Value)], name: &str) -> u32 {
    p.iter().find(|(n, _)| *n == name).map_or(0, |(_, v)| if let Value::Num(n) = v { *n } else { 0 })
}

fn rgb(c: u32) -> [u8; 3] {
    [((c >> 11 & 0x1F) << 3) as u8, ((c >> 5 & 0x3F) << 2) as u8, ((c & 0x1F) << 3) as u8]
}

fn widget(kind: WidgetKind, name: String, x: u32, y: u32, w: u32, h: u32, comment: String) -> Widget {
    let mut wd = Widget::new(kind, &name, x as i32, y as i32);
    (wd.w, wd.h, wd.comment) = (w.max(4) as i32, h.max(4) as i32, comment);
    wd
}

fn hotspot(event: Event, goto: Option<usize>) -> WidgetKind {
    WidgetKind::Button {
        text: String::new(), size: 20.0, fg: [255; 3], bg: [0; 3], pressed_bg: [0; 3], radius: 0.0,
        image: None, goto, event, invisible: true,
    }
}

/// Import `dir` (a DWIN_SET). Pictures are written to `<project_dir>/assets/`.
/// Returns the project and a human-readable report of what was skipped.
pub fn import(dir: &Path, project_dir: &Path) -> Result<(Project, Vec<String>), Box<dyn std::error::Error>> {
    let set = DwinSet::load(dir)?;
    let mut report = vec![];
    // Page pictures: 32.icl in every DGUS sample; otherwise the library with the biggest first icon.
    let (page_name, pics) = set
        .icls
        .iter()
        .max_by_key(|(n, icl)| (n.as_str() == "32.icl", icl.icons.iter().flatten().next().map_or(0, |i| i.width as u32 * i.height as u32)))
        .ok_or("no .icl files in this DWIN_SET")?;
    let first = pics.icons.iter().flatten().next().ok_or("page library is empty")?;
    let (sw, sh) = (first.width, first.height);
    let assets = project_dir.join("assets");
    std::fs::create_dir_all(&assets)?;

    let save = |file: String, icon: &crate::icl::Icon| -> Result<String, Box<dyn std::error::Error>> {
        std::fs::write(assets.join(&file), icon.to_jpeg()?)?;
        Ok(format!("assets/{file}"))
    };

    // Pages = ids that have a picture and are used (or below the highest used id).
    let touch = set.touch.as_ref().map_or(&[][..], |t| &t.records[..]);
    let shows = set.show.as_ref().map(|s| &s.pages);
    let used: Vec<u16> = touch.iter().map(|r| r.page()).chain(shows.into_iter().flat_map(|s| s.keys().copied())).collect();
    let last = used.iter().copied().max().unwrap_or(0) as usize;
    let mut pages = vec![];
    let mut index = BTreeMap::new(); // DGUS page id -> our page index
    for (id, icon) in pics.icons.iter().enumerate().take(last + 1) {
        let Some(icon) = icon else { continue };
        // Skip other sizes and pictures that only serve as a button's pressed look.
        let pressed_only = touch.iter().any(|r| r.pressed_page as usize == id) && !used.contains(&(id as u16));
        if (icon.width, icon.height) != (sw, sh) || pressed_only {
            continue;
        }
        index.insert(id, pages.len());
        let bg = save(format!("page_{id:02}.jpg"), icon)?;
        pages.push(Page { name: format!("page{id}"), bg: [0; 3], bg_image: Some(bg), swipe: (None, None), widgets: vec![] });
    }

    for (pid, recs) in shows.into_iter().flatten() {
        let Some(&pi) = index.get(&(*pid as usize)) else { continue };
        for r in recs {
            let (Some(k), Some(p)) = (show_kind(r), show_params(r)) else {
                report.push(format!("page {pid}: display type 0x{:02X} not imported", r[1]));
                continue;
            };
            let vp = num(&p, "vp") as u16;
            let (x, y) = (num(&p, "x"), num(&p, "y"));
            let w = match k.name {
                "number" => {
                    let f = num(&p, "font_size");
                    let digits = num(&p, "int_digits") + num(&p, "dec_digits") + 1;
                    let kind = WidgetKind::Number {
                        vp: Some(vp), int_digits: num(&p, "int_digits") as u8, dec_digits: num(&p, "dec_digits") as u8,
                        color: rgb(num(&p, "color")), input: None, smooth: false,
                    };
                    widget(kind, format!("num_{vp:04x}"), x, y, f * digits, 2 * f, String::new())
                }
                "text" => {
                    let (x0, y0) = (num(&p, "box_x0"), num(&p, "box_y0"));
                    let kind = WidgetKind::Text { vp: Some(vp), len: num(&p, "length") as u16, color: rgb(num(&p, "color")), input: false, scroll: false };
                    widget(kind, format!("text_{vp:04x}"), x0, y0, num(&p, "box_x1") - x0, num(&p, "box_y1") - y0, String::new())
                }
                "var_icon" => {
                    let lib = format!("{}.icl", num(&p, "lib"));
                    let Some(icl) = set.icls.get(&lib) else {
                        report.push(format!("page {pid}: icon VP {vp:#06x} uses missing {lib}"));
                        continue;
                    };
                    let (lo, hi) = (num(&p, "icon_min") as usize, num(&p, "icon_max") as usize);
                    let mut images = vec![];
                    let mut size = (32, 32);
                    for i in lo..=hi.min(lo + 255) {
                        if let Some(Some(icon)) = icl.icons.get(i) {
                            size = (icon.width as u32, icon.height as u32);
                            images.push(save(format!("icon{}_{i}.jpg", num(&p, "lib")), icon)?);
                        }
                    }
                    // ponytail: assumes v_min = 0 (icon index = VP value); offset ranges are rare.
                    widget(WidgetKind::Icons { vp: Some(vp), images }, format!("icons_{vp:04x}"), x, y, size.0, size.1, String::new())
                }
                "rtc_text" => {
                    let fmt = match p.iter().find(|(n, _)| *n == "format") {
                        Some((_, Value::Text(t))) => t.clone(),
                        _ => "Y-M-D H:Q:S".into(),
                    };
                    let f = num(&p, "font_size");
                    let kind = WidgetKind::Clock { format: fmt.clone(), color: rgb(num(&p, "color")) };
                    widget(kind, format!("clock{pid}"), x, y, f * fmt.len() as u32, 2 * f, String::new())
                }
                "progress_bar" => {
                    let (x0, y0) = (num(&p, "x0"), num(&p, "y0"));
                    let kind = WidgetKind::Progress {
                        vp: Some(vp), min: num(&p, "v_min") as u16, max: num(&p, "v_max") as u16, fg: rgb(num(&p, "fore_color")),
                        bg: rgb(num(&p, "back_color")), border: rgb(num(&p, "border_color")), vertical: num(&p, "orientation") != 0,
                    };
                    widget(kind, format!("bar_{vp:04x}"), x0, y0, num(&p, "x1") - x0, num(&p, "y1") - y0, String::new())
                }
                other => {
                    report.push(format!("page {pid}: '{other}' (VP {vp:#06x}) not imported yet"));
                    continue;
                }
            };
            pages[pi].widgets.push(w);
        }
    }

    for r in touch {
        let Some(&pi) = index.get(&(r.page() as usize)) else { continue };
        let (x, y, w, h) = (r.x0 as u32, r.y0 as u32, (r.x1 as u32).saturating_sub(r.x0 as u32), (r.y1 as u32).saturating_sub(r.y0 as u32));
        let goto = index.get(&(r.next_page as usize)).copied().filter(|_| r.next_page != crate::touch::NONE);
        let p = r.params().unwrap_or_default();
        let vp = num(&p, "vp") as u16;
        let kind_name = r.kind().map_or("basic", |k| k.name);
        let page = &mut pages[pi];
        // Inputs edit the matching display widget; buttons become invisible hotspots.
        let field = page.widgets.iter_mut().find(|wd| wd.kind.vp() == Some(Some(vp)));
        match (kind_name, field) {
            ("var_input", Some(f)) => {
                if let WidgetKind::Number { input, .. } = &mut f.kind {
                    *input = Some((num(&p, "min") as i32, num(&p, "max") as i32));
                }
            }
            ("text_input", Some(f)) => {
                if let WidgetKind::Text { input, .. } = &mut f.kind {
                    *input = true;
                }
            }
            ("basic", _) if goto.is_some() || r.code == 0 => {
                page.widgets.push(widget(hotspot(Event::None, goto), format!("go{}_{}", pi, page.widgets.len()), x, y, w, h, String::new()));
            }
            ("return_key", _) => {
                let ev = Event::Key { vp: Some(vp), key: num(&p, "key") as u16 };
                page.widgets.push(widget(hotspot(ev, goto), format!("key_{vp:04x}_{}", num(&p, "key")), x, y, w, h, String::new()));
            }
            ("increment", f) => {
                let target = f.map(|f| f.name.clone()).unwrap_or_else(|| format!("vp_{vp:04x}"));
                let step = num(&p, "step") as i16;
                let ev = Event::Adjust {
                    target, step: if num(&p, "adj_mode") == 0 { -step } else { step }, min: num(&p, "min") as u16,
                    max: num(&p, "max") as u16, wrap: num(&p, "return_mode") != 0, repeat: num(&p, "key_mode") != 0,
                };
                page.widgets.push(widget(hotspot(ev, goto), format!("adj_{vp:04x}_{}", page.widgets.len()), x, y, w, h, String::new()));
            }
            (other, _) => report.push(format!("page {}: touch '{other}' at ({x},{y}) not imported", r.page())),
        }
    }
    report.insert(0, format!("Imported {} page(s) from {page_name} ({sw}x{sh})", pages.len()));
    if pages.is_empty() {
        return Err("no page pictures found".into());
    }
    let mut proj = Project { width: sw, height: sh, pages, ..Project::default() };
    // Imported VPs are fixed so existing firmware keeps working; names must be unique.
    let mut seen = std::collections::HashSet::new();
    for w in proj.pages.iter_mut().flat_map(|p| &mut p.widgets) {
        while !seen.insert(w.name.clone()) {
            w.name.push('_');
        }
    }
    proj.page_lib = page_name.trim_end_matches(".icl").parse().unwrap_or(32);
    if let Some(cfg) = set.raw.get("T5LCFG.CFG") {
        std::fs::write(assets.join("T5LCFG.CFG"), cfg)?;
        proj.cfg = Some("assets/T5LCFG.CFG".into());
    }
    Ok((proj, report))
}

#[cfg(test)]
mod tests {
    #[test]
    fn imports_dgus_demos() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let out = std::env::temp_dir().join("dwin_import_test");
        for demo in ["Demo_2", "Demo_5"] {
            let dir = root.join(format!("DWIN_Training_Demos-master/{demo}/HMI/DWIN_SET"));
            if !dir.exists() {
                continue; // samples not present on this machine
            }
            let (p, report) = super::import(&dir, &out).unwrap();
            assert!(!p.pages.is_empty(), "{report:?}");
            let widgets: usize = p.pages.iter().map(|pg| pg.widgets.len()).sum();
            assert!(widgets > 0, "{report:?}");
            // Rebuilding the import must not fail.
            crate::build::build(&p, &out).unwrap();
        }
    }
    use std::path::Path;
}
