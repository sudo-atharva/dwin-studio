#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! DWIN Studio editor: design pages, build DWIN_SET for the SD card.
//! Layout follows LVGL-style editors: screens + hierarchy left, canvas center, inspector right.

use dwin_studio::build::{allocate, build, Alloc, Output};
use dwin_studio::project::{default_widgets, Page, Project, Widget, WidgetKind, PRESETS};
use dwin_studio::render::{render, Mode};
use eframe::egui::{self, Color32, CollapsingHeader, Key, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use std::path::{Path, PathBuf};

const GRID: i32 = 4;
const HANDLE: f32 = 10.0;
const ACCENT: Color32 = Color32::from_rgb(250, 180, 40);

/// Write DWIN_SET, ui_gen.h and ui_map.txt into `dir`.
fn write_output(o: &Output, dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    o.set.write(&dir.join("DWIN_SET"))?;
    std::fs::write(dir.join("ui_gen.h"), &o.header)?;
    std::fs::write(dir.join("ui_map.txt"), &o.map)?;
    Ok(())
}

/// `dwin-studio build <project.dwin.json> <out dir>`: headless build, prints warnings.
fn cli_build(project: &str, out: &str) -> Result<(), Box<dyn std::error::Error>> {
    let p: Project = serde_json::from_str(&std::fs::read_to_string(project)?)?;
    let o = build(&p)?;
    write_output(&o, Path::new(out))?;
    for w in &o.warnings {
        eprintln!("warning: {w}");
    }
    println!("built {} page(s) -> {out}", p.pages.len());
    Ok(())
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("build") {
        if let [_, _, project, out] = &args[..] {
            if let Err(e) = cli_build(project, out) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        } else {
            eprintln!("usage: dwin-studio build <project.dwin.json> <out dir>");
        }
        return Ok(());
    }
    let path = args.get(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1440.0, 880.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DWIN Studio",
        options,
        Box::new(|cc| {
            let mut v = egui::Visuals::dark();
            v.selection.bg_fill = Color32::from_rgb(120, 85, 20);
            v.selection.stroke = Stroke::new(1.0, ACCENT);
            cc.egui_ctx.set_visuals(v);
            Ok(Box::new(App::open(path)))
        }),
    )
}

#[derive(Clone, Copy)]
enum DragMode {
    Move,
    Resize,
}

struct Drag {
    mode: DragMode,
    start: Pos2,
    orig: (i32, i32, i32, i32),
}

struct App {
    project: Project,
    path: Option<PathBuf>,
    build_dir: Option<PathBuf>,
    page: usize,
    selected: Option<usize>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    drag: Option<Drag>,
    show_pressed: bool,
    texture: Option<egui::TextureHandle>,
    rendered: Option<(Page, u16, u16, bool)>,
    status: String,
    title: String,
}

impl App {
    fn open(path: Option<PathBuf>) -> Self {
        let mut app = Self {
            project: Project::default(),
            path: None,
            build_dir: None,
            page: 0,
            selected: None,
            undo: vec![],
            redo: vec![],
            drag: None,
            show_pressed: false,
            texture: None,
            rendered: None,
            status: "New project".into(),
            title: String::new(),
        };
        if let Some(p) = path {
            app.load(p);
        }
        app
    }

    fn load(&mut self, p: PathBuf) {
        let parsed = std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()));
        match parsed {
            Ok(proj) => {
                self.project = proj;
                self.status = format!("Opened {}", p.display());
                self.path = Some(p);
                self.page = 0;
                self.selected = None;
                self.undo.clear();
                self.redo.clear();
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    fn save(&mut self, ask: bool) {
        if ask || self.path.is_none() {
            let dlg = rfd::FileDialog::new().add_filter("DWIN Studio", &["json"]).set_file_name("project.dwin.json");
            let Some(p) = dlg.save_file() else { return };
            self.path = Some(p);
        }
        let p = self.path.clone().unwrap();
        let json = serde_json::to_string_pretty(&self.project).unwrap();
        self.status = match std::fs::write(&p, json) {
            Ok(()) => format!("Saved {}", p.display()),
            Err(e) => format!("Save failed: {e}"),
        };
    }

    fn do_build(&mut self) {
        if self.build_dir.is_none() {
            self.build_dir = rfd::FileDialog::new().set_title("Folder to put DWIN_SET in").pick_folder();
        }
        let Some(dir) = self.build_dir.clone() else { return };
        let result = build(&self.project).and_then(|o| write_output(&o, &dir).map(|_| o.warnings));
        self.status = match result {
            Ok(w) if w.is_empty() => format!("Built → {} (DWIN_SET, ui_gen.h, ui_map.txt)", dir.display()),
            Ok(w) => format!("Built with {} warning(s): {}", w.len(), w.join("; ")),
            Err(e) => format!("Build failed: {e}"),
        };
    }

    fn page_mut(&mut self) -> &mut Page {
        &mut self.project.pages[self.page]
    }

    fn unique_name(&self, base: &str) -> String {
        let used = |n: &str| self.project.pages.iter().flat_map(|p| &p.widgets).any(|w| w.name == n);
        (1..).map(|i| format!("{}{i}", base.to_lowercase())).find(|n| !used(n)).unwrap()
    }

    fn fix_selection(&mut self) {
        self.page = self.page.min(self.project.pages.len() - 1);
        if self.selected.is_some_and(|s| s >= self.project.pages[self.page].widgets.len()) {
            self.selected = None;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return; // typing in a field
        }
        let (undo, redo, del, dup, save, nudge) = ctx.input(|i| {
            let c = i.modifiers.command;
            let step = if i.modifiers.shift { 10 } else { 1 };
            let mut n = (0, 0);
            for (k, d) in [(Key::ArrowLeft, (-step, 0)), (Key::ArrowRight, (step, 0)), (Key::ArrowUp, (0, -step)), (Key::ArrowDown, (0, step))] {
                if i.key_pressed(k) {
                    n = d;
                }
            }
            (c && i.key_pressed(Key::Z), c && i.key_pressed(Key::Y), i.key_pressed(Key::Delete), c && i.key_pressed(Key::D), c && i.key_pressed(Key::S), n)
        });
        if save {
            self.save(false);
        }
        if undo {
            if let Some(p) = self.undo.pop() {
                self.redo.push(std::mem::replace(&mut self.project, p));
                self.fix_selection();
            }
        }
        if redo {
            if let Some(p) = self.redo.pop() {
                self.undo.push(std::mem::replace(&mut self.project, p));
                self.fix_selection();
            }
        }
        let Some(s) = self.selected else { return };
        if del {
            self.page_mut().widgets.remove(s);
            self.selected = None;
        } else if dup {
            let mut w = self.project.pages[self.page].widgets[s].clone();
            w.name = self.unique_name(w.kind_name());
            (w.x, w.y) = (w.x + 16, w.y + 16);
            // A copy gets its own address; a fixed VP would clash.
            match &mut w.kind {
                WidgetKind::Button { vp, .. } | WidgetKind::Number { vp, .. } | WidgetKind::Text { vp, .. } => *vp = None,
                _ => {}
            }
            self.page_mut().widgets.push(w);
            self.selected = Some(self.project.pages[self.page].widgets.len() - 1);
        } else if nudge != (0, 0) {
            let w = &mut self.page_mut().widgets[s];
            (w.x, w.y) = (w.x + nudge.0, w.y + nudge.1);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("DWIN Studio").strong().color(ACCENT));
            ui.separator();
            if ui.button("🗋 New").clicked() {
                self.undo.push(std::mem::take(&mut self.project));
                self.path = None;
                self.page = 0;
                self.selected = None;
            }
            if ui.button("📂 Open").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("DWIN Studio", &["json"]).pick_file() {
                    self.load(p);
                }
            }
            if ui.button("💾 Save").on_hover_text("Ctrl+S").clicked() {
                self.save(false);
            }
            if ui.button("Save as…").clicked() {
                self.save(true);
            }
            ui.separator();
            let build = egui::Button::new(RichText::new("▶ Build").strong().color(Color32::BLACK)).fill(ACCENT);
            if ui.add(build).on_hover_text("Write DWIN_SET, ui_gen.h, ui_map.txt").clicked() {
                self.do_build();
            }
            if ui.button("📁").on_hover_text("Change build folder").clicked() {
                self.build_dir = None;
                self.do_build();
            }
            ui.separator();
            ui.toggle_value(&mut self.show_pressed, "👆 Pressed view");
            ui.separator();
            ui.label(RichText::new(&self.status).weak());
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        CollapsingHeader::new("SCREEN SIZE").default_open(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.project.width).range(16..=4096));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut self.project.height).range(16..=4096));
            });
            egui::ComboBox::from_id_salt("preset").selected_text("Presets").show_ui(ui, |ui| {
                for &(w, h) in PRESETS {
                    if ui.button(format!("{w} × {h}")).clicked() {
                        (self.project.width, self.project.height) = (w, h);
                    }
                }
            });
        });

        CollapsingHeader::new("SCREENS").default_open(true).show(ui, |ui| {
            let mut remove = None;
            for i in 0..self.project.pages.len() {
                ui.horizontal(|ui| {
                    let label = format!("▣ {i}  {}", self.project.pages[i].name);
                    if ui.selectable_label(self.page == i, label).clicked() {
                        self.page = i;
                        self.selected = None;
                    }
                    if self.project.pages.len() > 1 && ui.small_button("✕").on_hover_text("Delete screen").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.project.pages.remove(i);
                self.fix_selection();
            }
            if ui.button("+ Screen").clicked() {
                let n = self.project.pages.len();
                let bg = self.project.pages[self.page].bg;
                self.project.pages.push(Page { name: format!("screen{n}"), bg, widgets: vec![] });
                self.page = n;
                self.selected = None;
            }
        });

        CollapsingHeader::new("HIERARCHY").default_open(true).show(ui, |ui| {
            let page = &self.project.pages[self.page];
            if page.widgets.is_empty() {
                ui.label(RichText::new("empty - add widgets below").weak());
            }
            // Top of list = front-most, like LVGL editors.
            let mut pick = None;
            for (i, w) in page.widgets.iter().enumerate().rev() {
                let icon = match w.kind {
                    WidgetKind::Button { .. } => "⏺",
                    WidgetKind::Label { .. } => "🗛",
                    WidgetKind::Panel { .. } => "▭",
                    WidgetKind::Number { .. } => "#",
                    WidgetKind::Text { .. } => "¶",
                };
                let r = ui.selectable_label(self.selected == Some(i), format!("{icon} {}", w.name));
                let r = if w.comment.is_empty() { r } else { r.on_hover_text(&w.comment) };
                if r.clicked() {
                    pick = Some(i);
                }
            }
            if pick.is_some() {
                self.selected = pick;
            }
            if let Some(s) = self.selected {
                ui.horizontal(|ui| {
                    let n = self.project.pages[self.page].widgets.len();
                    if ui.small_button("⬆ Front").clicked() && s + 1 < n {
                        self.page_mut().widgets.swap(s, s + 1);
                        self.selected = Some(s + 1);
                    }
                    if ui.small_button("⬇ Back").clicked() && s > 0 {
                        self.page_mut().widgets.swap(s, s - 1);
                        self.selected = Some(s - 1);
                    }
                });
            }
        });

        CollapsingHeader::new("WIDGETS").default_open(true).show(ui, |ui| {
            egui::Grid::new("palette").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
                for (i, (name, kind)) in default_widgets().into_iter().enumerate() {
                    if ui.add_sized([92.0, 28.0], egui::Button::new(name)).clicked() {
                        let (cx, cy) = (self.project.width as i32 / 2 - 60, self.project.height as i32 / 2 - 30);
                        let w = Widget::new(kind, &self.unique_name(name), cx, cy);
                        self.page_mut().widgets.push(w);
                        self.selected = Some(self.project.pages[self.page].widgets.len() - 1);
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        });
        ui.add_space(8.0);
        ui.small(RichText::new("Drag = move · corner = resize\nArrows nudge (Shift ×10)\nDel · Ctrl+D · Ctrl+Z/Y · Ctrl+S").weak());
    }

    fn inspector(&mut self, ui: &mut egui::Ui, allocs: &[Alloc]) {
        ui.heading("Inspector");
        let Some(s) = self.selected else {
            let page = &mut self.project.pages[self.page];
            CollapsingHeader::new("SCREEN").default_open(true).show(ui, |ui| {
                egui::Grid::new("screen").num_columns(2).show(ui, |ui| {
                    text_row(ui, "Name", &mut page.name);
                    color_row(ui, "Background", &mut page.bg);
                });
            });
            ui.label(RichText::new("Select a widget to edit it").weak());
            return;
        };
        let pages: Vec<String> = self.project.pages.iter().map(|p| p.name.clone()).collect();
        let page = self.page;
        let alloc = allocs.iter().find(|a| a.page == page && a.widget == s).cloned();
        let w = &mut self.project.pages[page].widgets[s];

        CollapsingHeader::new(format!("{}", w.kind_name().to_uppercase())).default_open(true).show(ui, |ui| {
            egui::Grid::new("widget").num_columns(2).show(ui, |ui| {
                text_row(ui, "Name", &mut w.name);
                ui.label("Comment");
                ui.add(egui::TextEdit::multiline(&mut w.comment).desired_rows(2).hint_text("What does this do?"));
                ui.end_row();
            });
        });

        CollapsingHeader::new("LAYOUT").default_open(true).show(ui, |ui| {
            egui::Grid::new("layout").num_columns(4).show(ui, |ui| {
                ui.label("X");
                ui.add(egui::DragValue::new(&mut w.x));
                ui.label("Y");
                ui.add(egui::DragValue::new(&mut w.y));
                ui.end_row();
                ui.label("W");
                ui.add(egui::DragValue::new(&mut w.w).range(1..=4096));
                ui.label("H");
                ui.add(egui::DragValue::new(&mut w.h).range(1..=4096));
                ui.end_row();
            });
        });

        CollapsingHeader::new("STYLE").default_open(true).show(ui, |ui| {
            egui::Grid::new("style").num_columns(2).show(ui, |ui| match &mut w.kind {
                WidgetKind::Label { text, size, color } => {
                    text_row(ui, "Text", text);
                    size_row(ui, size);
                    color_row(ui, "Color", color);
                }
                WidgetKind::Panel { color, radius } => {
                    color_row(ui, "Color", color);
                    radius_row(ui, radius);
                }
                WidgetKind::Button { text, size, fg, bg, pressed_bg, radius, .. } => {
                    text_row(ui, "Text", text);
                    size_row(ui, size);
                    color_row(ui, "Text color", fg);
                    color_row(ui, "Fill", bg);
                    color_row(ui, "Pressed fill", pressed_bg);
                    radius_row(ui, radius);
                }
                WidgetKind::Number { int_digits, dec_digits, color, .. } => {
                    ui.label("Integer digits");
                    ui.add(egui::DragValue::new(int_digits).range(1..=10));
                    ui.end_row();
                    ui.label("Decimals");
                    ui.add(egui::DragValue::new(dec_digits).range(0..=5));
                    ui.end_row();
                    color_row(ui, "Color", color);
                    ui.label("Font size");
                    ui.label("= height (H)");
                    ui.end_row();
                }
                WidgetKind::Text { len, color, .. } => {
                    ui.label("Max bytes");
                    ui.add(egui::DragValue::new(len).range(1..=255));
                    ui.end_row();
                    color_row(ui, "Color", color);
                    ui.label("Font size");
                    ui.label("= height (H)");
                    ui.end_row();
                }
            });
        });

        if let WidgetKind::Button { goto, send_key, key, .. } = &mut w.kind {
            CollapsingHeader::new("EVENTS").default_open(true).show(ui, |ui| {
                ui.checkbox(send_key, "Send key to MCU on press");
                if *send_key {
                    ui.horizontal(|ui| {
                        ui.label("Key value");
                        ui.add(egui::DragValue::new(key).hexadecimal(4, false, true));
                    });
                }
                let mut jump = goto.is_some();
                if ui.checkbox(&mut jump, "Go to screen on release").changed() {
                    *goto = jump.then_some(0);
                }
                if let Some(t) = goto {
                    egui::ComboBox::from_id_salt("goto").selected_text(pages.get(*t).map_or("?", |s| s)).show_ui(ui, |ui| {
                        for (i, n) in pages.iter().enumerate() {
                            ui.selectable_value(t, i, format!("{i}: {n}"));
                        }
                    });
                }
            });
        }

        // Read-only bits for the UART example, taken before borrowing the VP mutably.
        let button_key = match &w.kind {
            WidgetKind::Button { key, .. } => Some(*key),
            _ => None,
        };
        let is_number = matches!(w.kind, WidgetKind::Number { .. });
        let vp = match &mut w.kind {
            WidgetKind::Button { vp, send_key: true, .. } | WidgetKind::Number { vp, .. } | WidgetKind::Text { vp, .. } => Some(vp),
            _ => None,
        };
        if let Some(vp) = vp {
            CollapsingHeader::new("DWIN ADDRESS").default_open(true).show(ui, |ui| {
                vp_editor(ui, vp, alloc.as_ref().map(|a| a.vp));
                if let Some(a) = &alloc {
                    if let Some(sp) = a.sp {
                        ui.horizontal(|ui| {
                            ui.label("SP");
                            ui.monospace(format!("{sp:#06x}")).on_hover_text("Description pointer: change position/color at runtime");
                        });
                    }
                    let (hi, lo) = (a.vp >> 8, a.vp & 0xFF);
                    let frame = match button_key {
                        Some(key) => format!("Screen → MCU on press:\n5A A5 06 83 {hi:02X} {lo:02X} 01 {:02X} {:02X}", key >> 8, key & 0xFF),
                        None if is_number => format!("MCU → screen (value 123):\n5A A5 05 82 {hi:02X} {lo:02X} 00 7B"),
                        None => format!("MCU → screen (text \"Hi\"):\n5A A5 05 82 {hi:02X} {lo:02X} 48 69"),
                    };
                    ui.add_space(4.0);
                    ui.label(RichText::new(frame).monospace().small().weak());
                }
            });
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (pw, ph) = (self.project.width as f32, self.project.height as f32);
        let avail = ui.available_size() - Vec2::splat(24.0);
        let scale = (avail.x / pw).min(avail.y / ph).max(0.05);
        let rect = Rect::from_center_size(ui.available_rect_before_wrap().center(), Vec2::new(pw, ph) * scale);

        let key = (self.project.pages[self.page].clone(), self.project.width, self.project.height, self.show_pressed);
        if self.rendered.as_ref() != Some(&key) {
            let mode = if self.show_pressed { Mode::Pressed } else { Mode::Preview };
            let px = render(&key.0, key.1, key.2, mode);
            let img = egui::ColorImage::from_rgba_premultiplied([px.width() as usize, px.height() as usize], px.data());
            self.texture = Some(ui.ctx().load_texture("page", img, egui::TextureOptions::LINEAR));
            self.rendered = Some(key);
        }
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect.expand(HANDLE));
        if let Some(t) = &self.texture {
            painter.image(t.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let to_screen = |x: i32, y: i32| rect.min + Vec2::new(x as f32, y as f32) * scale;
        let to_proj = |p: Pos2| ((p - rect.min) / scale).to_pos2();
        let wrect = |w: &Widget| Rect::from_min_max(to_screen(w.x, w.y), to_screen(w.x + w.w, w.y + w.h));

        for w in &self.project.pages[self.page].widgets {
            if matches!(w.kind, WidgetKind::Number { .. } | WidgetKind::Text { .. }) {
                painter.rect_stroke(wrect(w), 0.0, Stroke::new(1.0, Color32::from_gray(110)), StrokeKind::Inside);
            }
        }

        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let widgets = &self.project.pages[self.page].widgets;
                let on_handle = self.selected.filter(|&s| Rect::from_center_size(wrect(&widgets[s]).max, Vec2::splat(HANDLE * 1.6)).contains(p));
                let hit = on_handle.or_else(|| widgets.iter().rposition(|w| wrect(w).contains(p)));
                self.selected = hit;
                if let Some(i) = hit {
                    let w = &widgets[i];
                    self.drag = Some(Drag {
                        mode: if on_handle.is_some() { DragMode::Resize } else { DragMode::Move },
                        start: to_proj(p),
                        orig: (w.x, w.y, w.w, w.h),
                    });
                    self.undo.push(self.project.clone());
                    self.redo.clear();
                }
            }
        }
        if let (Some(d), Some(s), Some(p)) = (&self.drag, self.selected, resp.interact_pointer_pos()) {
            if resp.dragged() {
                let delta = to_proj(p) - d.start;
                let snap = |v: f32| (v as i32 / GRID) * GRID;
                let (x, y, w0, h0) = d.orig;
                let w = &mut self.project.pages[self.page].widgets[s];
                match d.mode {
                    DragMode::Move => (w.x, w.y) = (x + snap(delta.x), y + snap(delta.y)),
                    DragMode::Resize => (w.w, w.h) = ((w0 + snap(delta.x)).max(GRID), (h0 + snap(delta.y)).max(GRID)),
                }
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if resp.clicked() {
            let p = resp.interact_pointer_pos().unwrap();
            self.selected = self.project.pages[self.page].widgets.iter().rposition(|w| wrect(w).contains(p));
        }
        if let Some(s) = self.selected {
            let w = &self.project.pages[self.page].widgets[s];
            let r = wrect(w);
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, ACCENT), StrokeKind::Outside);
            painter.rect_filled(Rect::from_center_size(r.max, Vec2::splat(HANDLE)), 2.0, ACCENT);
            painter.text(r.left_top() - Vec2::new(0.0, 2.0), egui::Align2::LEFT_BOTTOM, format!("{}  {}×{}", w.name, w.w, w.h), egui::FontId::proportional(11.0), ACCENT);
        }
        ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::DARK_GRAY), StrokeKind::Outside);
    }

    fn address_map(&self, ui: &mut egui::Ui, allocs: &[Alloc], warnings: &[String]) {
        ui.horizontal(|ui| {
            ui.strong("ADDRESS MAP");
            ui.label(RichText::new(format!("{} item(s)", allocs.len())).weak());
        });
        for w in warnings {
            ui.colored_label(Color32::from_rgb(240, 90, 80), format!("⚠ {w}"));
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("vpmap").striped(true).show(ui, |ui| {
                for h in ["Screen", "Name", "Type", "VP", "Key", "SP", "Comment"] {
                    ui.strong(h);
                }
                ui.end_row();
                for a in allocs {
                    let w = &self.project.pages[a.page].widgets[a.widget];
                    ui.label(&self.project.pages[a.page].name);
                    ui.label(&a.name);
                    ui.label(a.what);
                    let fixed = matches!(&w.kind, WidgetKind::Button { vp: Some(_), .. } | WidgetKind::Number { vp: Some(_), .. } | WidgetKind::Text { vp: Some(_), .. });
                    ui.monospace(format!("{:#06x}{}", a.vp, if fixed { "" } else { " auto" }));
                    ui.monospace(match &w.kind {
                        WidgetKind::Button { key, .. } => format!("{key:#06x}"),
                        _ => "-".into(),
                    });
                    ui.monospace(a.sp.map_or("-".into(), |s| format!("{s:#06x}")));
                    ui.label(RichText::new(&w.comment).weak());
                    ui.end_row();
                }
            });
        });
    }
}

fn text_row(ui: &mut egui::Ui, label: &str, s: &mut String) {
    ui.label(label);
    ui.text_edit_singleline(s);
    ui.end_row();
}

fn size_row(ui: &mut egui::Ui, size: &mut f32) {
    ui.label("Font size");
    ui.add(egui::DragValue::new(size).range(6.0..=200.0));
    ui.end_row();
}

fn radius_row(ui: &mut egui::Ui, r: &mut f32) {
    ui.label("Radius");
    ui.add(egui::DragValue::new(r).range(0.0..=200.0));
    ui.end_row();
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [u8; 3]) {
    ui.label(label);
    ui.color_edit_button_srgb(c);
    ui.end_row();
}

/// Auto shows the address the build will use; switching to manual starts from it.
fn vp_editor(ui: &mut egui::Ui, vp: &mut Option<u16>, resolved: Option<u16>) {
    ui.horizontal(|ui| {
        ui.label("VP");
        let mut auto = vp.is_none();
        if ui.checkbox(&mut auto, "Auto").on_hover_text("Assigned at build, never clashes").changed() {
            *vp = if auto { None } else { Some(resolved.unwrap_or(0x2000)) };
        }
        match vp {
            None => {
                ui.monospace(resolved.map_or("-".into(), |v| format!("{v:#06x}")));
            }
            Some(v) => {
                ui.label("0x");
                let mut s = format!("{v:04X}");
                if ui.add(egui::TextEdit::singleline(&mut s).desired_width(48.0).font(egui::TextStyle::Monospace)).changed() {
                    if let Ok(n) = u16::from_str_radix(s.trim(), 16) {
                        *v = n;
                    }
                }
            }
        }
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let before = self.project.clone();
        self.shortcuts(&ui.ctx().clone());
        let (allocs, warnings) = allocate(&self.project);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::left("left").default_size(230.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.left_panel(ui));
        });
        egui::Panel::right("inspector").default_size(300.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui, &allocs));
        });
        egui::Panel::bottom("map").resizable(true).default_size(150.0).show(ui, |ui| self.address_map(ui, &allocs, &warnings));
        egui::CentralPanel::default_margins().show(ui, |ui| self.canvas(ui));

        // Property/page edits: one undo step per change (drags push their own at start).
        if self.drag.is_none() && self.project != before && self.undo.last() != Some(&before) {
            self.undo.push(before);
            self.redo.clear();
        }
        let title = format!("DWIN Studio - {}", self.path.as_ref().map_or("untitled".into(), |p| p.display().to_string()));
        if title != self.title {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}
