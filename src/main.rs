#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! DWIN Studio editor: design pages, build DWIN_SET for the SD card.

use dwin_studio::build::{allocate, build, Alloc};
use dwin_studio::project::{default_widgets, Action, Page, Project, Widget, WidgetKind, PRESETS};
use dwin_studio::render::{render, Mode};
use eframe::egui::{self, Color32, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::path::PathBuf;

const GRID: i32 = 4;
const HANDLE: f32 = 10.0;

/// `dwin-studio build <project.dwin.json> <out dir>`: headless build, prints warnings.
fn cli_build(project: &str, out: &str) -> Result<(), Box<dyn std::error::Error>> {
    let p: Project = serde_json::from_str(&std::fs::read_to_string(project)?)?;
    let o = build(&p)?;
    let out = PathBuf::from(out);
    o.set.write(&out.join("DWIN_SET"))?;
    std::fs::write(out.join("ui_gen.h"), &o.header)?;
    for w in &o.warnings {
        eprintln!("warning: {w}");
    }
    println!("built {} page(s) -> {}", p.pages.len(), out.join("DWIN_SET").display());
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
        viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 860.0]),
        ..Default::default()
    };
    eframe::run_native("DWIN Studio", options, Box::new(|_| Ok(Box::new(App::open(path)))))
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
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string())) {
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
            let Some(p) = rfd::FileDialog::new().add_filter("DWIN Studio", &["json"]).set_file_name("project.dwin.json").save_file() else { return };
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
        let result = build(&self.project).and_then(|out| {
            out.set.write(&dir.join("DWIN_SET"))?;
            std::fs::write(dir.join("ui_gen.h"), &out.header)?;
            Ok(out.warnings)
        });
        self.status = match result {
            Ok(w) if w.is_empty() => format!("Built {}\\DWIN_SET + ui_gen.h", dir.display()),
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

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        let (undo, redo, del, dup, nudge) = ctx.input(|i| {
            let c = i.modifiers.command;
            let step = if i.modifiers.shift { 10 } else { 1 };
            let mut n = (0, 0);
            for (k, d) in [(Key::ArrowLeft, (-step, 0)), (Key::ArrowRight, (step, 0)), (Key::ArrowUp, (0, -step)), (Key::ArrowDown, (0, step))] {
                if i.key_pressed(k) {
                    n = d;
                }
            }
            (c && i.key_pressed(Key::Z), c && i.key_pressed(Key::Y), i.key_pressed(Key::Delete), c && i.key_pressed(Key::D), n)
        });
        if typing {
            return;
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
            w.x += 16;
            w.y += 16;
            self.page_mut().widgets.push(w);
            self.selected = Some(self.project.pages[self.page].widgets.len() - 1);
        } else if nudge != (0, 0) {
            let w = &mut self.page_mut().widgets[s];
            w.x += nudge.0;
            w.y += nudge.1;
        }
    }

    fn fix_selection(&mut self) {
        self.page = self.page.min(self.project.pages.len() - 1);
        if self.selected.is_some_and(|s| s >= self.project.pages[self.page].widgets.len()) {
            self.selected = None;
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                self.undo.push(std::mem::take(&mut self.project));
                self.path = None;
                self.page = 0;
                self.selected = None;
            }
            if ui.button("Open…").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("DWIN Studio", &["json"]).pick_file() {
                    self.load(p);
                }
            }
            if ui.button("Save").clicked() {
                self.save(false);
            }
            if ui.button("Save as…").clicked() {
                self.save(true);
            }
            ui.separator();
            if ui.button("⚙ Build DWIN_SET").clicked() {
                self.do_build();
            }
            if ui.small_button("…").on_hover_text("Change build folder").clicked() {
                self.build_dir = None;
                self.do_build();
            }
            ui.separator();
            ui.checkbox(&mut self.show_pressed, "Show pressed");
            ui.separator();
            ui.label(&self.status);
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Screen");
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

        ui.separator();
        ui.heading("Pages");
        let mut remove = None;
        for i in 0..self.project.pages.len() {
            ui.horizontal(|ui| {
                if ui.selectable_label(self.page == i, format!("{i}: {}", self.project.pages[i].name)).clicked() {
                    self.page = i;
                    self.selected = None;
                }
                if self.project.pages.len() > 1 && ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.project.pages.remove(i);
            self.fix_selection();
        }
        if ui.button("+ Page").clicked() {
            let n = self.project.pages.len();
            let bg = self.project.pages[self.page].bg;
            self.project.pages.push(Page { name: format!("page{n}"), bg, widgets: vec![] });
            self.page = n;
            self.selected = None;
        }
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.project.pages[self.page].name);
        });
        ui.horizontal(|ui| {
            ui.label("Background");
            ui.color_edit_button_srgb(&mut self.project.pages[self.page].bg);
        });

        ui.separator();
        ui.heading("Widgets");
        for (name, kind) in default_widgets() {
            if ui.button(format!("+ {name}")).clicked() {
                let (cx, cy) = (self.project.width as i32 / 2 - 60, self.project.height as i32 / 2 - 30);
                let w = Widget::new(kind, &self.unique_name(name), cx, cy);
                self.page_mut().widgets.push(w);
                self.selected = Some(self.project.pages[self.page].widgets.len() - 1);
            }
        }
        ui.separator();
        ui.small("Drag = move · corner = resize\nArrows = nudge (Shift ×10)\nDel · Ctrl+D dup · Ctrl+Z/Y");
    }

    fn properties(&mut self, ui: &mut egui::Ui, allocs: &[Alloc]) {
        let Some(s) = self.selected else {
            ui.label("Select a widget");
            return;
        };
        let pages: Vec<String> = self.project.pages.iter().map(|p| p.name.clone()).collect();
        let page = self.page;
        let alloc = allocs.iter().find(|a| a.page == page && a.widget == s).cloned();
        let w = &mut self.project.pages[page].widgets[s];
        ui.heading(w.kind_name());
        egui::Grid::new("props").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut w.name);
            ui.end_row();
            for (label, v) in [("X", &mut w.x), ("Y", &mut w.y), ("W", &mut w.w), ("H", &mut w.h)] {
                ui.label(label);
                ui.add(egui::DragValue::new(v));
                ui.end_row();
            }
            match &mut w.kind {
                WidgetKind::Label { text, size, color } => {
                    text_row(ui, "Text", text);
                    size_row(ui, size);
                    color_row(ui, "Color", color);
                }
                WidgetKind::Panel { color, radius } => {
                    color_row(ui, "Color", color);
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(radius).range(0.0..=200.0));
                    ui.end_row();
                }
                WidgetKind::Button { text, size, fg, bg, pressed_bg, radius, action } => {
                    text_row(ui, "Text", text);
                    size_row(ui, size);
                    color_row(ui, "Text color", fg);
                    color_row(ui, "Fill", bg);
                    color_row(ui, "Pressed fill", pressed_bg);
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(radius).range(0.0..=200.0));
                    ui.end_row();
                    action_rows(ui, action, &pages);
                }
                WidgetKind::Number { vp, int_digits, dec_digits, color } => {
                    vp_row(ui, vp);
                    ui.label("Integer digits");
                    ui.add(egui::DragValue::new(int_digits).range(1..=10));
                    ui.end_row();
                    ui.label("Decimals");
                    ui.add(egui::DragValue::new(dec_digits).range(0..=5));
                    ui.end_row();
                    color_row(ui, "Color", color);
                }
                WidgetKind::Text { vp, len, color } => {
                    vp_row(ui, vp);
                    ui.label("Max bytes");
                    ui.add(egui::DragValue::new(len).range(1..=255));
                    ui.end_row();
                    color_row(ui, "Color", color);
                }
            }
        });
        if let Some(a) = alloc {
            ui.separator();
            ui.monospace(format!("VP {:#06x} ({} word)", a.vp, a.words));
            if let Some(sp) = a.sp {
                ui.monospace(format!("SP {sp:#06x}"));
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (pw, ph) = (self.project.width as f32, self.project.height as f32);
        let avail = ui.available_size() - Vec2::splat(16.0);
        let scale = (avail.x / pw).min(avail.y / ph).max(0.05);
        let size = Vec2::new(pw, ph) * scale;
        let rect = Rect::from_center_size(ui.available_rect_before_wrap().center(), size);

        let key = (self.project.pages[self.page].clone(), self.project.width, self.project.height, self.show_pressed);
        if self.rendered.as_ref() != Some(&key) {
            let mode = if self.show_pressed { Mode::Pressed } else { Mode::Preview };
            let px = render(&key.0, key.1, key.2, mode);
            let img = egui::ColorImage::from_rgba_premultiplied([px.width() as usize, px.height() as usize], px.data());
            self.texture = Some(ui.ctx().load_texture("page", img, egui::TextureOptions::LINEAR));
            self.rendered = Some(key);
        }
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        if let Some(t) = &self.texture {
            painter.image(t.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let to_screen = |x: i32, y: i32| rect.min + Vec2::new(x as f32, y as f32) * scale;
        let to_proj = |p: Pos2| ((p - rect.min) / scale).to_pos2();
        let wrect = |w: &Widget| Rect::from_min_max(to_screen(w.x, w.y), to_screen(w.x + w.w, w.y + w.h));

        // Outline runtime widgets so they are findable even when empty.
        for w in &self.project.pages[self.page].widgets {
            if matches!(w.kind, WidgetKind::Number { .. } | WidgetKind::Text { .. }) {
                painter.rect_stroke(wrect(w), 0.0, Stroke::new(1.0, Color32::from_gray(110)), StrokeKind::Inside);
            }
        }

        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let widgets = &self.project.pages[self.page].widgets;
                let on_handle = self.selected.filter(|&s| {
                    let r = wrect(&widgets[s]);
                    Rect::from_center_size(r.max, Vec2::splat(HANDLE * 1.6)).contains(p)
                });
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
            let r = wrect(&self.project.pages[self.page].widgets[s]);
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_rgb(250, 180, 40)), StrokeKind::Outside);
            painter.rect_filled(Rect::from_center_size(r.max, Vec2::splat(HANDLE)), 2.0, Color32::from_rgb(250, 180, 40));
        }
        ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::DARK_GRAY), StrokeKind::Outside);
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

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [u8; 3]) {
    ui.label(label);
    ui.color_edit_button_srgb(c);
    ui.end_row();
}

/// VP: auto (allocated at build) or fixed hex address.
fn vp_row(ui: &mut egui::Ui, vp: &mut Option<u16>) {
    ui.label("VP");
    ui.horizontal(|ui| {
        let mut auto = vp.is_none();
        if ui.checkbox(&mut auto, "auto").changed() {
            *vp = if auto { None } else { Some(0x2000) };
        }
        if let Some(v) = vp {
            let mut s = format!("{v:04X}");
            if ui.add(egui::TextEdit::singleline(&mut s).desired_width(50.0)).changed() {
                if let Ok(n) = u16::from_str_radix(s.trim_start_matches("0x"), 16) {
                    *v = n;
                }
            }
        }
    });
    ui.end_row();
}

fn action_rows(ui: &mut egui::Ui, action: &mut Action, pages: &[String]) {
    ui.label("On press");
    let label = match action {
        Action::None => "Nothing",
        Action::GoTo(_) => "Go to page",
        Action::ReturnKey { .. } => "Return key",
    };
    egui::ComboBox::from_id_salt("action").selected_text(label).show_ui(ui, |ui| {
        if ui.selectable_label(matches!(action, Action::None), "Nothing").clicked() {
            *action = Action::None;
        }
        if ui.selectable_label(matches!(action, Action::GoTo(_)), "Go to page").clicked() {
            *action = Action::GoTo(0);
        }
        if ui.selectable_label(matches!(action, Action::ReturnKey { .. }), "Return key").clicked() {
            *action = Action::ReturnKey { vp: None, key: 1 };
        }
    });
    ui.end_row();
    match action {
        Action::None => {}
        Action::GoTo(t) => {
            ui.label("Page");
            egui::ComboBox::from_id_salt("goto").selected_text(pages.get(*t).map_or("?", |s| s)).show_ui(ui, |ui| {
                for (i, n) in pages.iter().enumerate() {
                    ui.selectable_value(t, i, format!("{i}: {n}"));
                }
            });
            ui.end_row();
        }
        Action::ReturnKey { vp, key } => {
            vp_row(ui, vp);
            ui.label("Key value");
            ui.add(egui::DragValue::new(key).hexadecimal(4, false, true));
            ui.end_row();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let before = self.project.clone();
        self.shortcuts(&ui.ctx().clone());
        let (allocs, warnings) = allocate(&self.project);

        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("left").default_size(210.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.left_panel(ui));
        });
        egui::Panel::right("right").default_size(280.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.properties(ui, &allocs));
        });
        egui::Panel::bottom("bottom").resizable(true).default_size(130.0).show(ui, |ui| {
            for w in &warnings {
                ui.colored_label(Color32::from_rgb(240, 90, 80), format!("⚠ {w}"));
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("vpmap").striped(true).show(ui, |ui| {
                    for h in ["Page", "Name", "Type", "VP", "Words", "SP"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for a in &allocs {
                        ui.label(&self.project.pages[a.page].name);
                        ui.label(&a.name);
                        ui.label(a.what);
                        ui.monospace(format!("{:#06x}", a.vp));
                        ui.label(a.words.to_string());
                        ui.monospace(a.sp.map_or("-".into(), |s| format!("{s:#06x}")));
                        ui.end_row();
                    }
                });
            });
        });
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
