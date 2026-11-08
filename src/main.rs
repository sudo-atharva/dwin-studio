#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! DWIN Studio editor: design pages, build DWIN_SET for the SD card.
//! Layout follows LVGL-style editors: screens / hierarchy / widgets / assets left,
//! canvas center (zoom, pan, multi-select, align), inspector right, address map bottom.

use dwin_studio::build::{allocate, build, Alloc, Output};
use dwin_studio::project::{default_widgets, Event, Page, Project, Widget, WidgetKind, PRESETS};
use dwin_studio::render::{load_image, render, Mode};
use dwin_studio::sim::Sim;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;
use eframe::egui::{self, Color32, CollapsingHeader, Key, PointerButton, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const HANDLE: f32 = 10.0;
const ACCENT: Color32 = Color32::from_rgb(56, 139, 253);
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "bmp"];

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
    let base = Path::new(project).parent().unwrap_or(Path::new("."));
    let o = build(&p, base)?;
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
        viewport: egui::ViewportBuilder::default().with_inner_size([1480.0, 900.0]).with_maximized(true),
        ..Default::default()
    };
    eframe::run_native(
        "DWIN Studio",
        options,
        Box::new(|cc| {
            let mut v = egui::Visuals::dark();
            v.selection.bg_fill = Color32::from_rgb(30, 70, 130);
            v.selection.stroke = Stroke::new(1.0, ACCENT);
            v.panel_fill = Color32::from_rgb(37, 37, 38);
            v.window_fill = Color32::from_rgb(45, 45, 48);
            v.extreme_bg_color = Color32::from_rgb(30, 30, 30);
            v.hyperlink_color = ACCENT;
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(62, 62, 66);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(51, 51, 55);
            v.window_corner_radius = egui::CornerRadius::same(6);
            cc.egui_ctx.set_visuals(v);
            Ok(Box::new(App::open(path)))
        }),
    )
}

enum Drag {
    /// Moving the selection: original rects.
    Move { start: Pos2, orig: Vec<(usize, i32, i32)> },
    Resize { start: Pos2, idx: usize, orig: (i32, i32) },
    /// Rubber-band selection from this canvas position.
    Box { start: Pos2 },
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
    DistH,
    DistV,
}

struct App {
    project: Project,
    path: Option<PathBuf>,
    build_dir: Option<PathBuf>,
    page: usize,
    selected: Vec<usize>,
    clipboard: Vec<Widget>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    drag: Option<Drag>,
    show_pressed: bool,
    grid: i32,
    /// None = fit to window.
    zoom: Option<f32>,
    pan: Vec2,
    assets: Vec<String>,
    thumbs: HashMap<String, egui::TextureHandle>,
    texture: Option<egui::TextureHandle>,
    rendered: Option<(Page, u16, u16, bool)>,
    status: String,
    title: String,
    /// Some = Simulate mode (EEZ "Run"): the page behaves like the screen.
    sim: Option<Sim>,
    port: Option<Box<dyn serialport::SerialPort>>,
    port_rx: Option<mpsc::Receiver<Vec<u8>>>,
    ports: Vec<String>,
    port_name: String,
    baud: u32,
}

impl App {
    fn open(path: Option<PathBuf>) -> Self {
        let mut app = Self {
            project: Project::default(),
            path: None,
            build_dir: None,
            page: 0,
            selected: vec![],
            clipboard: vec![],
            undo: vec![],
            redo: vec![],
            drag: None,
            show_pressed: false,
            grid: 4,
            zoom: None,
            pan: Vec2::ZERO,
            assets: vec![],
            thumbs: HashMap::new(),
            texture: None,
            rendered: None,
            status: "New project - save it to start importing images".into(),
            title: String::new(),
            sim: None,
            port: None,
            port_rx: None,
            ports: vec![],
            port_name: String::new(),
            baud: 115200,
        };
        if let Some(p) = path {
            app.load(p);
        }
        app
    }

    /// Folder that image paths are relative to.
    fn base(&self) -> PathBuf {
        self.path.as_ref().and_then(|p| p.parent()).map_or(PathBuf::from("."), Path::to_path_buf)
    }

    fn refresh_assets(&mut self) {
        let dir = self.base().join("assets");
        let mut list: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()).is_some_and(|x| IMAGE_EXT.contains(&x.to_lowercase().as_str())))
            .map(|e| format!("assets/{}", e.file_name().to_string_lossy()))
            .collect();
        list.sort();
        self.assets = list;
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
                self.selected.clear();
                self.undo.clear();
                self.redo.clear();
                self.refresh_assets();
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    fn save(&mut self, ask: bool) -> bool {
        if ask || self.path.is_none() {
            let dlg = rfd::FileDialog::new().add_filter("DWIN Studio", &["json"]).set_file_name("project.dwin.json");
            let Some(p) = dlg.save_file() else { return false };
            self.path = Some(p);
            self.refresh_assets();
        }
        let p = self.path.clone().unwrap();
        let json = serde_json::to_string_pretty(&self.project).unwrap();
        match std::fs::write(&p, json) {
            Ok(()) => {
                self.status = format!("Saved {}", p.display());
                true
            }
            Err(e) => {
                self.status = format!("Save failed: {e}");
                false
            }
        }
    }

    fn import_assets(&mut self) {
        if self.path.is_none() && !self.save(true) {
            self.status = "Save the project first: images are copied next to it in assets/".into();
            return;
        }
        let Some(files) = rfd::FileDialog::new().add_filter("Images", IMAGE_EXT).pick_files() else { return };
        let dir = self.base().join("assets");
        let mut n = 0;
        for f in files {
            let Some(name) = f.file_name() else { continue };
            if std::fs::create_dir_all(&dir).and_then(|_| std::fs::copy(&f, dir.join(name))).is_ok() {
                n += 1;
            }
        }
        self.status = format!("Imported {n} image(s) into {}", dir.display());
        self.refresh_assets();
    }

    fn do_build(&mut self) {
        if self.build_dir.is_none() {
            self.build_dir = rfd::FileDialog::new().set_title("Folder to put DWIN_SET in").pick_folder();
        }
        let Some(dir) = self.build_dir.clone() else { return };
        let result = build(&self.project, &self.base()).and_then(|o| write_output(&o, &dir).map(|_| o.warnings));
        self.status = match result {
            Ok(w) if w.is_empty() => format!("Built → {} (DWIN_SET, ui_gen.h, ui_map.txt)", dir.display()),
            Ok(w) => format!("Built with {} warning(s): {}", w.len(), w.join("; ")),
            Err(e) => format!("Build failed: {e}"),
        };
    }

    fn widgets(&self) -> &Vec<Widget> {
        &self.project.pages[self.page].widgets
    }

    fn widgets_mut(&mut self) -> &mut Vec<Widget> {
        &mut self.project.pages[self.page].widgets
    }

    fn unique_name(&self, base: &str) -> String {
        let base: String = base.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        let base = if base.is_empty() { "item".to_string() } else { base };
        let used = |n: &str| self.project.pages.iter().flat_map(|p| &p.widgets).any(|w| w.name == n);
        (1..).map(|i| format!("{base}{i}")).find(|n| !used(n)).unwrap()
    }

    fn add_widget(&mut self, mut w: Widget) {
        w.name = self.unique_name(if w.name.is_empty() { w.kind_name() } else { &w.name });
        self.widgets_mut().push(w);
        self.selected = vec![self.widgets().len() - 1];
    }

    fn fix_selection(&mut self) {
        self.page = self.page.min(self.project.pages.len() - 1);
        let n = self.widgets().len();
        self.selected.retain(|&s| s < n);
    }

    fn paste(&mut self, items: Vec<Widget>) {
        let start = self.widgets().len();
        for mut w in items {
            w.name = self.unique_name(&w.name.trim_end_matches(|c: char| c.is_ascii_digit()).to_string());
            (w.x, w.y) = (w.x + 16, w.y + 16);
            if let Some(vp) = w.kind.vp_mut() {
                *vp = None; // a copy gets its own address
            }
            self.widgets_mut().push(w);
        }
        self.selected = (start..self.widgets().len()).collect();
    }

    fn align(&mut self, how: Align) {
        let sel = self.selected.clone();
        if sel.is_empty() {
            return;
        }
        // One widget aligns to the screen, several to their bounding box.
        let (l, t, r, b) = if sel.len() == 1 {
            (0, 0, self.project.width as i32, self.project.height as i32)
        } else {
            let ws: Vec<_> = sel.iter().map(|&i| &self.widgets()[i]).collect();
            (
                ws.iter().map(|w| w.x).min().unwrap(),
                ws.iter().map(|w| w.y).min().unwrap(),
                ws.iter().map(|w| w.x + w.w).max().unwrap(),
                ws.iter().map(|w| w.y + w.h).max().unwrap(),
            )
        };
        let mut order = sel.clone();
        match how {
            Align::DistH | Align::DistV if sel.len() >= 3 => {
                let horiz = matches!(how, Align::DistH);
                order.sort_by_key(|&i| if horiz { self.widgets()[i].x } else { self.widgets()[i].y });
                let total: i32 = order.iter().map(|&i| if horiz { self.widgets()[i].w } else { self.widgets()[i].h }).sum();
                let gap = ((if horiz { r - l } else { b - t }) - total) / (order.len() as i32 - 1);
                let mut pos = if horiz { l } else { t };
                for i in order {
                    let w = &mut self.widgets_mut()[i];
                    if horiz {
                        w.x = pos;
                        pos += w.w + gap;
                    } else {
                        w.y = pos;
                        pos += w.h + gap;
                    }
                }
            }
            Align::DistH | Align::DistV => self.status = "Distribute needs 3+ widgets selected".into(),
            _ => {
                for i in sel {
                    let w = &mut self.widgets_mut()[i];
                    match how {
                        Align::Left => w.x = l,
                        Align::HCenter => w.x = (l + r - w.w) / 2,
                        Align::Right => w.x = r - w.w,
                        Align::Top => w.y = t,
                        Align::VCenter => w.y = (t + b - w.h) / 2,
                        Align::Bottom => w.y = b - w.h,
                        _ => {}
                    }
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return; // typing in a field
        }
        let (c, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        if c && pressed(Key::S) {
            self.save(false);
        }
        if c && pressed(Key::Z) {
            if let Some(p) = self.undo.pop() {
                self.redo.push(std::mem::replace(&mut self.project, p));
                self.fix_selection();
            }
        }
        if c && pressed(Key::Y) {
            if let Some(p) = self.redo.pop() {
                self.undo.push(std::mem::replace(&mut self.project, p));
                self.fix_selection();
            }
        }
        if c && pressed(Key::A) {
            self.selected = (0..self.widgets().len()).collect();
        }
        if c && pressed(Key::V) && !self.clipboard.is_empty() {
            self.paste(self.clipboard.clone());
        }
        if self.selected.is_empty() {
            return;
        }
        let picked: Vec<Widget> = self.selected.iter().map(|&i| self.widgets()[i].clone()).collect();
        if c && pressed(Key::C) {
            self.clipboard = picked;
        } else if c && pressed(Key::X) {
            self.clipboard = picked;
            self.delete_selected();
        } else if c && pressed(Key::D) {
            self.paste(picked);
        } else if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selected();
        } else {
            let step = if shift { 10 } else { 1 };
            let mut d = (0, 0);
            for (k, v) in [(Key::ArrowLeft, (-step, 0)), (Key::ArrowRight, (step, 0)), (Key::ArrowUp, (0, -step)), (Key::ArrowDown, (0, step))] {
                if pressed(k) {
                    d = v;
                }
            }
            for i in self.selected.clone() {
                let w = &mut self.widgets_mut()[i];
                (w.x, w.y) = (w.x + d.0, w.y + d.1);
            }
        }
    }

    fn delete_selected(&mut self) {
        let mut sel = std::mem::take(&mut self.selected);
        sel.sort_unstable_by(|a, b| b.cmp(a));
        for i in sel {
            self.widgets_mut().remove(i);
        }
    }

    fn thumb(&mut self, ctx: &egui::Context, rel: &str) -> Option<egui::TextureId> {
        if !self.thumbs.contains_key(rel) {
            let img = load_image(&self.base(), rel)?;
            let t = image::imageops::thumbnail(img.as_ref(), 48, 48);
            let ci = egui::ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw());
            self.thumbs.insert(rel.to_string(), ctx.load_texture(rel, ci, egui::TextureOptions::LINEAR));
        }
        self.thumbs.get(rel).map(|t| t.id())
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("DWIN Studio").strong().color(ACCENT));
            ui.separator();
            if ui.button("🗋 New").clicked() {
                self.undo.push(std::mem::take(&mut self.project));
                self.path = None;
                self.page = 0;
                self.selected.clear();
                self.assets.clear();
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
            let running = self.sim.is_some();
            if ui.selectable_label(!running, RichText::new("✏ Edit").strong()).clicked() && running {
                self.stop_sim();
            }
            let run = ui.selectable_label(running, RichText::new("▶ Simulate").strong());
            if run.on_hover_text("Run the screen: touch it, watch the UART frames, connect a COM port").clicked() && !running {
                self.start_sim();
            }
            ui.separator();
            let build = egui::Button::new(RichText::new("⚙ Build").strong().color(Color32::WHITE)).fill(ACCENT);
            if ui.add(build).on_hover_text("Write DWIN_SET, ui_gen.h, ui_map.txt").clicked() {
                self.do_build();
            }
            if ui.button("📁").on_hover_text("Change build folder").clicked() {
                self.build_dir = None;
                self.do_build();
            }
            ui.separator();
            if self.sim.is_some() {
                self.com_controls(ui);
                ui.separator();
                ui.label(RichText::new(&self.status).weak());
                return;
            }
            // Align / distribute
            for (icon, how, tip) in [
                ("⇤", Align::Left, "Align left"), ("↔", Align::HCenter, "Center horizontally"), ("⇥", Align::Right, "Align right"),
                ("⤒", Align::Top, "Align top"), ("↕", Align::VCenter, "Center vertically"), ("⤓", Align::Bottom, "Align bottom"),
                ("⋯", Align::DistH, "Distribute horizontally (3+)"), ("⋮", Align::DistV, "Distribute vertically (3+)"),
            ] {
                let tip = format!("{tip} - one widget aligns to the screen");
                if ui.add_enabled(!self.selected.is_empty(), egui::Button::new(icon)).on_hover_text(tip).clicked() {
                    self.align(how);
                }
            }
            ui.separator();
            ui.toggle_value(&mut self.show_pressed, "👆 Pressed");
            ui.label("Grid");
            ui.add(egui::DragValue::new(&mut self.grid).range(1..=64));
            let zoom_text = self.zoom.map_or("Fit".into(), |z| format!("{:.0}%", z * 100.0));
            if ui.button(zoom_text).on_hover_text("Ctrl+wheel zoom, middle-drag or wheel pans. Click = fit").clicked() {
                (self.zoom, self.pan) = (None, Vec2::ZERO);
            }
            if ui.button("100%").clicked() {
                (self.zoom, self.pan) = (Some(1.0), Vec2::ZERO);
            }
            ui.separator();
            ui.label(RichText::new(&self.status).weak());
        });
    }

    fn start_sim(&mut self) {
        let mut s = Sim::new(&self.project);
        s.page = self.page;
        self.sim = Some(s);
        self.drag = None;
        self.refresh_ports();
    }

    fn stop_sim(&mut self) {
        if let Some(s) = self.sim.take() {
            self.page = s.page.min(self.project.pages.len() - 1);
        }
    }

    fn refresh_ports(&mut self) {
        self.ports = serialport::available_ports().unwrap_or_default().into_iter().map(|p| p.port_name).collect();
        if !self.ports.contains(&self.port_name) {
            self.port_name = self.ports.first().cloned().unwrap_or_default();
        }
    }

    fn connect(&mut self, ctx: &egui::Context) {
        let opened = serialport::new(&self.port_name, self.baud).timeout(Duration::from_millis(50)).open();
        let (port, mut reader) = match opened.and_then(|p| p.try_clone().map(|r| (p, r))) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("{}: {e}", self.port_name);
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        // Reader thread; ends when the UI drops the receiver (disconnect) or the port fails.
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            loop {
                let chunk = match reader.read(&mut buf) {
                    Ok(n) => buf[..n].to_vec(),
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => vec![],
                    Err(_) => break,
                };
                let got = !chunk.is_empty();
                if tx.send(chunk).is_err() {
                    break;
                }
                if got {
                    ctx.request_repaint();
                }
            }
        });
        self.status = format!("Simulator on {} @ {} baud", self.port_name, self.baud);
        (self.port, self.port_rx) = (Some(port), Some(rx));
    }

    /// Move bytes between the serial port and the simulator.
    fn pump(&mut self) {
        let Some(sim) = &mut self.sim else { return };
        if let Some(rx) = &self.port_rx {
            while let Ok(b) = rx.try_recv() {
                sim.feed(&b);
            }
        }
        for f in sim.out.drain(..) {
            if let Some(p) = &mut self.port {
                if p.write_all(&f).is_err() {
                    self.status = "Serial write failed - disconnected".into();
                    (self.port, self.port_rx) = (None, None);
                }
            }
        }
        let n = sim.log.len();
        if n > 1000 {
            sim.log.drain(..n - 1000);
        }
    }

    fn sim_canvas(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        let (pw, ph) = (self.project.width as f32, self.project.height as f32);
        let scale = self.zoom.unwrap_or(((area.width() - 24.0) / pw).min((area.height() - 24.0) / ph).max(0.05));
        let rect = Rect::from_center_size(area.center() + self.pan, Vec2::new(pw, ph) * scale);
        let base = self.base();
        let Some(sim) = &mut self.sim else { return };
        let to_proj = |p: Pos2| ((p - rect.min) / scale).to_pos2();

        let (down, up, pos) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.interact_pos()));
        if let Some(p) = pos {
            let q = to_proj(p);
            if down && resp.hovered() {
                sim.press(&self.project, q.x as i32, q.y as i32);
            } else if resp.dragged() {
                sim.drag(&self.project, q.x as i32, q.y as i32);
            }
        }
        // PC keyboard types into the on-screen keypad/keyboard.
        if sim.popup.is_some() {
            for e in ui.input(|i| i.events.clone()) {
                match e {
                    egui::Event::Text(t) => t.chars().for_each(|c| sim.type_char(c)),
                    egui::Event::Key { key: Key::Enter, pressed: true, .. } => sim.key(0xF1),
                    egui::Event::Key { key: Key::Backspace, pressed: true, .. } => sim.key(0xF2),
                    egui::Event::Key { key: Key::Escape, pressed: true, .. } => sim.key(0xF0),
                    _ => {}
                }
            }
        }
        if up {
            let x = pos.map_or(0, |p| to_proj(p).x as i32);
            sim.release(&self.project, x);
        }

        let px = sim.render(&self.project, &base);
        let img = egui::ColorImage::from_rgba_premultiplied([px.width() as usize, px.height() as usize], px.data());
        match &mut self.texture {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => self.texture = Some(ui.ctx().load_texture("page", img, egui::TextureOptions::LINEAR)),
        }
        self.rendered = None; // editor view re-renders when we leave
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, Color32::from_gray(24));
        if let Some(t) = &self.texture {
            painter.image(t.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        // Pressed feedback, like DWIN copying from the pressed page image.
        let page = &self.project.pages[sim.page.min(self.project.pages.len() - 1)];
        if let Some(w) = sim.held.and_then(|i| page.widgets.get(i)).filter(|w| matches!(w.kind, WidgetKind::Button { .. })) {
            let r = Rect::from_min_size(rect.min + Vec2::new(w.x as f32, w.y as f32) * scale, Vec2::new(w.w as f32, w.h as f32) * scale);
            painter.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
        }
        let green = Color32::from_rgb(34, 197, 94);
        painter.rect_stroke(rect, 0.0, Stroke::new(2.0, green), StrokeKind::Outside);
        let label = format!("SIMULATING  page {}: {}", sim.page, page.name);
        painter.text(area.left_top() + Vec2::new(8.0, 6.0), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(12.0), green);
        ui.ctx().request_repaint_after(Duration::from_millis(50)); // clock, animations
    }

    /// Bottom panel in Simulate mode: COM port + UART log.
    /// COM port picker: the PC plays the HMI screen for the MCU board on this port.
    fn com_controls(&mut self, ui: &mut egui::Ui) {
        ui.strong("🔌 COM");
        let shown = if self.port_name.is_empty() { "(no port)".to_string() } else { self.port_name.clone() };
        egui::ComboBox::from_id_salt("port").selected_text(shown).show_ui(ui, |ui| {
            for p in &self.ports {
                ui.selectable_value(&mut self.port_name, p.clone(), p);
            }
        });
        if ui.small_button("⟳").on_hover_text("Rescan COM ports").clicked() {
            self.refresh_ports();
        }
        egui::ComboBox::from_id_salt("baud").selected_text(self.baud.to_string()).show_ui(ui, |ui| {
            for b in [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600] {
                ui.selectable_value(&mut self.baud, b, b.to_string());
            }
        });
        if self.port.is_some() {
            if ui.button("Disconnect").clicked() {
                (self.port, self.port_rx) = (None, None);
                self.status = "Serial disconnected".into();
            }
            ui.label(RichText::new("● connected").color(Color32::from_rgb(34, 197, 94)));
        } else if ui.add_enabled(!self.port_name.is_empty(), egui::Button::new("Connect")).clicked() {
            self.connect(&ui.ctx().clone());
        }
    }

    fn sim_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("UART log");
            ui.label(RichText::new("Connect your MCU board with any USB-UART adapter (COM in the toolbar): this window is its screen.").weak());
            if ui.small_button("Clear").clicked() {
                if let Some(s) = &mut self.sim {
                    s.log.clear();
                }
            }
        });
        ui.separator();
        let Some(sim) = &self.sim else { return };
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
            for l in &sim.log {
                let c = if l.starts_with('→') { Color32::from_rgb(250, 180, 40) } else { Color32::from_rgb(96, 165, 250) };
                ui.label(RichText::new(l).monospace().color(c));
            }
            if sim.log.is_empty() {
                ui.label(RichText::new("Touch the screen to see frames. → = screen to MCU, ← = MCU to screen.").weak());
            }
        });
    }

    /// Right panel in Simulate mode: every VP, editable like an MCU write.
    fn sim_vars(&mut self, ui: &mut egui::Ui) {
        ui.heading("Variables");
        ui.label(RichText::new("Edit a value = MCU writes it").weak());
        let Some(sim) = &mut self.sim else { return };
        ui.horizontal(|ui| {
            ui.label("Page");
            let mut pg = sim.page;
            egui::ComboBox::from_id_salt("simpage").selected_text(format!("{pg}")).show_ui(ui, |ui| {
                for (i, p) in self.project.pages.iter().enumerate() {
                    ui.selectable_value(&mut pg, i, format!("{i}: {}", p.name));
                }
            });
            sim.page = pg;
        });
        egui::Grid::new("simvars").striped(true).num_columns(3).show(ui, |ui| {
            for a in sim.allocs.clone() {
                let w = &self.project.pages[a.page].widgets[a.widget];
                ui.label(&a.name).on_hover_text(&w.comment);
                ui.monospace(format!("{:#06x}", a.vp));
                match &w.kind {
                    WidgetKind::Text { len, .. } | WidgetKind::Qr { len, .. } => {
                        let mut t = sim.text_at(a.vp, *len);
                        if ui.add(egui::TextEdit::singleline(&mut t).desired_width(110.0)).changed() {
                            let mut b = t.into_bytes();
                            b.truncate(*len as usize);
                            b.extend([0xFF, 0xFF]);
                            for (i, pair) in b.chunks(2).enumerate() {
                                sim.set(a.vp + i as u16, u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0xFF)]));
                            }
                        }
                    }
                    _ => {
                        let mut v = sim.get(a.vp) as i16 as i32;
                        if ui.add(egui::DragValue::new(&mut v).range(-32768..=65535)).changed() {
                            sim.set(a.vp, v as u16);
                        }
                    }
                }
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Chart samples: MCU writes 0x0310. Clock shows PC time (UTC).").weak().small());
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
                        self.selected.clear();
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
            ui.horizontal(|ui| {
                if ui.button("+ Screen").clicked() {
                    let n = self.project.pages.len();
                    let bg = self.project.pages[self.page].bg;
                    self.project.pages.push(Page { name: format!("screen{n}"), bg, bg_image: None, swipe: (None, None), widgets: vec![] });
                    self.page = n;
                    self.selected.clear();
                }
                if ui.button("⧉ Duplicate").clicked() {
                    let mut p = self.project.pages[self.page].clone();
                    p.name += "_copy";
                    for w in &mut p.widgets {
                        w.name = format!("{}_c", w.name);
                        if let Some(vp) = w.kind.vp_mut() {
                            *vp = None;
                        }
                    }
                    self.project.pages.push(p);
                    self.page = self.project.pages.len() - 1;
                    self.selected.clear();
                }
            });
        });

        CollapsingHeader::new("HIERARCHY").default_open(true).show(ui, |ui| {
            if self.widgets().is_empty() {
                ui.label(RichText::new("empty - add widgets below").weak());
            }
            let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            let mut pick = None;
            // Top of list = front-most, like LVGL editors.
            for (i, w) in self.widgets().iter().enumerate().rev() {
                let icon = kind_icon(&w.kind);
                let r = ui.selectable_label(self.selected.contains(&i), format!("{icon} {}", w.name));
                let r = if w.comment.is_empty() { r } else { r.on_hover_text(&w.comment) };
                if r.clicked() {
                    pick = Some(i);
                }
            }
            if let Some(i) = pick {
                if shift {
                    if let Some(pos) = self.selected.iter().position(|&s| s == i) {
                        self.selected.remove(pos);
                    } else {
                        self.selected.push(i);
                    }
                } else {
                    self.selected = vec![i];
                }
            }
            if let [s] = self.selected[..] {
                ui.horizontal(|ui| {
                    let n = self.widgets().len();
                    if ui.small_button("⬆ Forward").clicked() && s + 1 < n {
                        self.widgets_mut().swap(s, s + 1);
                        self.selected = vec![s + 1];
                    }
                    if ui.small_button("⬇ Backward").clicked() && s > 0 {
                        self.widgets_mut().swap(s, s - 1);
                        self.selected = vec![s - 1];
                    }
                });
            }
        });

        let all = default_widgets();
        let known: Vec<&str> = GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect();
        let more: Vec<&str> = all.iter().map(|(n, _)| *n).filter(|n| !known.contains(n)).collect();
        for (group, names) in GROUPS.iter().copied().chain([("More", &more[..])]) {
            if names.is_empty() {
                continue;
            }
            CollapsingHeader::new(group.to_uppercase()).id_salt(group).default_open(true).show(ui, |ui| {
                egui::Grid::new(group).num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        let Some((_, kind)) = all.iter().find(|(n, _)| n == name) else { continue };
                        let label = format!("{} {name}", kind_icon(kind));
                        if ui.add_sized([108.0, 26.0], egui::Button::new(label)).clicked() {
                            let (cx, cy) = (self.project.width as i32 / 2 - 60, self.project.height as i32 / 2 - 30);
                            self.add_widget(Widget::new(kind.clone(), "", cx, cy));
                        }
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            });
        }

        CollapsingHeader::new("ASSETS").default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⬆ Import images…").clicked() {
                    self.import_assets();
                }
                if ui.small_button("⟳").on_hover_text("Rescan assets folder").clicked() {
                    self.thumbs.clear();
                    self.refresh_assets();
                }
            });
            if self.assets.is_empty() {
                ui.label(RichText::new("PNG/JPG/BMP go to <project>/assets").weak());
            }
            let ctx = ui.ctx().clone();
            for rel in self.assets.clone() {
                ui.horizontal(|ui| {
                    if let Some(t) = self.thumb(&ctx, &rel) {
                        ui.image((t, Vec2::splat(32.0)));
                    }
                    let name = rel.trim_start_matches("assets/");
                    let r = ui.add(egui::Button::new(name).truncate()).on_hover_text("Click: add as Image widget");
                    if r.clicked() {
                        self.add_image(&rel);
                    }
                    r.context_menu(|ui| {
                        if ui.button("Use as screen background").clicked() {
                            self.project.pages[self.page].bg_image = Some(rel.clone());
                            ui.close();
                        }
                    });
                });
            }
        });
        ui.add_space(8.0);
        ui.small(
            RichText::new("Drag move · corner resize · drag empty = box select\nShift+click multi · Ctrl+A/C/X/V/D · Del\nArrows nudge (Shift ×10) · Ctrl+Z/Y · Ctrl+S\nCtrl+wheel zoom · middle-drag/wheel pan")
                .weak(),
        );
    }

    fn add_image(&mut self, rel: &str) {
        let Some(img) = load_image(&self.base(), rel) else { return };
        let (sw, sh) = (self.project.width as f32, self.project.height as f32);
        let k = (sw / img.width() as f32).min(sh / img.height() as f32).min(1.0);
        let (w, h) = ((img.width() as f32 * k) as i32, (img.height() as f32 * k) as i32);
        let stem = Path::new(rel).file_stem().map_or("image".into(), |s| s.to_string_lossy().to_string());
        let mut wd = Widget::new(WidgetKind::Image { path: rel.into() }, &stem, (sw as i32 - w) / 2, (sh as i32 - h) / 2);
        (wd.w, wd.h) = (w, h);
        self.add_widget(wd);
    }

    fn inspector(&mut self, ui: &mut egui::Ui, allocs: &[Alloc]) {
        ui.heading("Inspector");
        let assets = self.assets.clone();
        let [s] = self.selected[..] else {
            if self.selected.len() > 1 {
                ui.label(format!("{} widgets selected - use the align buttons in the toolbar", self.selected.len()));
                return;
            }
            let names: Vec<String> = self.project.pages.iter().map(|p| p.name.clone()).collect();
            let page = &mut self.project.pages[self.page];
            CollapsingHeader::new("SCREEN").default_open(true).show(ui, |ui| {
                egui::Grid::new("screen").num_columns(2).show(ui, |ui| {
                    text_row(ui, "Name", &mut page.name);
                    color_row(ui, "Background", &mut page.bg);
                    ui.label("Picture");
                    asset_pick(ui, "bgimg", &mut page.bg_image, &assets);
                    ui.end_row();
                    for (label, t, id) in [("Swipe right →", &mut page.swipe.0, "swl"), ("Swipe left →", &mut page.swipe.1, "swr")] {
                        ui.label(label);
                        let text = t.and_then(|i| names.get(i)).map_or("(none)".to_string(), |n| n.clone());
                        egui::ComboBox::from_id_salt(id).selected_text(text).show_ui(ui, |ui| {
                            ui.selectable_value(t, None, "(none)");
                            for (i, n) in names.iter().enumerate() {
                                ui.selectable_value(t, Some(i), format!("{i}: {n}"));
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            ui.label(RichText::new("Select a widget to edit it").weak());
            return;
        };
        let pages: Vec<String> = self.project.pages.iter().map(|p| p.name.clone()).collect();
        let page = self.page;
        let alloc = allocs.iter().find(|a| a.page == page && a.widget == s).cloned();
        // Things +/- buttons can point at: anything with a value.
        let targets: Vec<String> = allocs.iter().filter(|a| !["button", "text", "qr"].contains(&a.what)).map(|a| a.name.clone()).collect();
        let w = &mut self.project.pages[page].widgets[s];

        CollapsingHeader::new(w.kind_name().to_uppercase()).default_open(true).show(ui, |ui| {
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
                WidgetKind::Image { path } => {
                    ui.label("Picture");
                    let mut p = Some(path.clone());
                    asset_pick(ui, "img", &mut p, &assets);
                    if let Some(p) = p {
                        *path = p;
                    }
                    ui.end_row();
                }
                WidgetKind::Button { text, size, fg, bg, pressed_bg, radius, image, .. } => {
                    text_row(ui, "Text", text);
                    size_row(ui, size);
                    color_row(ui, "Text color", fg);
                    ui.label("Picture");
                    asset_pick(ui, "btnimg", image, &assets);
                    ui.end_row();
                    if image.is_none() {
                        color_row(ui, "Fill", bg);
                        color_row(ui, "Pressed fill", pressed_bg);
                        radius_row(ui, radius);
                    }
                }
                WidgetKind::Number { int_digits, dec_digits, color, input, smooth, .. } => {
                    drag_row(ui, "Integer digits", int_digits, 1..=10);
                    drag_row(ui, "Decimals", dec_digits, 0..=5);
                    color_row(ui, "Color", color);
                    ui.label("Font size");
                    ui.label("= height (H)");
                    ui.end_row();
                    ui.label("Tap to edit");
                    let mut on = input.is_some();
                    if ui.checkbox(&mut on, "keypad").changed() {
                        *input = on.then(|| (0, 10i32.pow(*int_digits as u32) - 1));
                    }
                    ui.end_row();
                    if let Some((lo, hi)) = input {
                        drag_row(ui, "Min", lo, -32768..=32767);
                        drag_row(ui, "Max", hi, -32768..=32767);
                    }
                    ui.label("Font");
                    ui.checkbox(smooth, "smooth (image digits)").on_hover_text("Digits drawn with this editor's font instead of the screen's built-in one");
                    ui.end_row();
                }
                WidgetKind::Text { len, color, input, scroll, .. } => {
                    drag_row(ui, "Max bytes", len, 1..=255);
                    color_row(ui, "Color", color);
                    ui.label("Font size");
                    ui.label("= height (H)");
                    ui.end_row();
                    ui.label("Tap to edit");
                    ui.checkbox(input, "keyboard");
                    ui.end_row();
                    ui.label("Scroll");
                    ui.checkbox(scroll, "marquee");
                    ui.end_row();
                }
                WidgetKind::Dropdown { options, size, fg, bg, .. } => {
                    ui.label("Options\n(one per line)");
                    let mut s = options.join("\n");
                    if ui.add(egui::TextEdit::multiline(&mut s).desired_rows(4)).changed() {
                        *options = s.lines().map(str::to_string).collect();
                    }
                    ui.end_row();
                    size_row(ui, size);
                    color_row(ui, "Text color", fg);
                    color_row(ui, "Fill", bg);
                }
                WidgetKind::Clock { format, color } => {
                    text_row(ui, "Format", format);
                    ui.label("");
                    ui.label(RichText::new("Y year · M month · D day\nH hour · Q minute · S second · W weekday").small().weak());
                    ui.end_row();
                    color_row(ui, "Color", color);
                }
                WidgetKind::Qr { len, dot, .. } => {
                    drag_row(ui, "Max bytes", len, 1..=458);
                    drag_row(ui, "Pixels per dot", dot, 1..=16);
                }
                WidgetKind::Progress { min, max, fg, bg, border, vertical, .. } => {
                    drag_row(ui, "Min", min, 0..=65535);
                    drag_row(ui, "Max", max, 0..=65535);
                    color_row(ui, "Bar", fg);
                    color_row(ui, "Background", bg);
                    color_row(ui, "Border", border);
                    ui.label("Direction");
                    ui.checkbox(vertical, "vertical");
                    ui.end_row();
                }
                WidgetKind::Chart { channel, min, max, step, color, bg } => {
                    drag_row(ui, "Channel", channel, 0..=7);
                    drag_row(ui, "Min", min, -32768..=32767);
                    drag_row(ui, "Max", max, -32768..=32767);
                    drag_row(ui, "Pixels per sample", step, 1..=50);
                    color_row(ui, "Line", color);
                    color_row(ui, "Background", bg);
                }
                WidgetKind::Gauge { min, max, sweep, needle, dial, knob, .. } => {
                    drag_row(ui, "Min", min, 0..=65535);
                    drag_row(ui, "Max", max, 0..=65535);
                    drag_row(ui, "Sweep (°)", sweep, 30..=355);
                    color_row(ui, "Needle", needle);
                    color_row(ui, "Dial", dial);
                    ui.label("Touch");
                    ui.checkbox(knob, "knob: drag to set");
                    ui.end_row();
                }
                WidgetKind::Drawing { words, bg, .. } => {
                    drag_row(ui, "Command buffer (words)", words, 8..=4096);
                    color_row(ui, "Background", bg);
                }
                WidgetKind::Animation { images, period_ms, autoplay, .. } => {
                    drag_row(ui, "Frame time (ms)", period_ms, 8..=2000);
                    ui.label("Start");
                    ui.checkbox(autoplay, "playing at boot");
                    ui.end_row();
                    image_list(ui, images, &assets);
                }
                WidgetKind::Toggle { on, off, knob, bit, .. } => {
                    color_row(ui, "On", on);
                    color_row(ui, "Off", off);
                    color_row(ui, "Knob", knob);
                    bit_row(ui, bit);
                }
                WidgetKind::Lamp { on, off, bit, .. } => {
                    color_row(ui, "On", on);
                    color_row(ui, "Off", off);
                    bit_row(ui, bit);
                }
                WidgetKind::Slider { min, max, track, knob, show_track, .. } => {
                    drag_row(ui, "Min", min, 0..=65535);
                    drag_row(ui, "Max", max, 0..=65535);
                    ui.label("Bar");
                    ui.checkbox(show_track, "show bar").on_hover_text("Off = transparent: only the knob is drawn");
                    ui.end_row();
                    if *show_track {
                        color_row(ui, "Track", track);
                    }
                    color_row(ui, "Knob", knob);
                    ui.label("Knob size");
                    ui.label("= height (H)");
                    ui.end_row();
                }
                WidgetKind::Icons { images, .. } => {
                    ui.label("Value → picture");
                    ui.end_row();
                    image_list(ui, images, &assets);
                }
            });
        });

        if let WidgetKind::Button { goto, event, .. } = &mut w.kind {
            CollapsingHeader::new("EVENTS").default_open(true).show(ui, |ui| {
                let label = match event {
                    Event::None => "Nothing",
                    Event::Key { .. } => "Send key to MCU",
                    Event::Adjust { .. } => "Change a value (+/−)",
                };
                ui.horizontal(|ui| {
                    ui.label("On press");
                    egui::ComboBox::from_id_salt("event").selected_text(label).show_ui(ui, |ui| {
                        if ui.selectable_label(matches!(event, Event::None), "Nothing").clicked() {
                            *event = Event::None;
                        }
                        if ui.selectable_label(matches!(event, Event::Key { .. }), "Send key to MCU").clicked() {
                            *event = Event::Key { vp: None, key: 1 };
                        }
                        if ui.selectable_label(matches!(event, Event::Adjust { .. }), "Change a value (+/−)").clicked() {
                            let target = targets.first().cloned().unwrap_or_default();
                            *event = Event::Adjust { target, step: 1, min: 0, max: 100, wrap: false, repeat: true };
                        }
                    });
                });
                match event {
                    Event::None => {}
                    Event::Key { key, .. } => {
                        ui.horizontal(|ui| {
                            ui.label("Key value");
                            ui.add(egui::DragValue::new(key).hexadecimal(4, false, true));
                        });
                    }
                    Event::Adjust { target, step, min, max, wrap, repeat } => {
                        ui.horizontal(|ui| {
                            ui.label("Target");
                            egui::ComboBox::from_id_salt("target").selected_text(target.as_str()).show_ui(ui, |ui| {
                                for t in &targets {
                                    ui.selectable_value(target, t.clone(), t);
                                }
                            });
                        });
                        egui::Grid::new("adj").num_columns(2).show(ui, |ui| {
                            drag_row(ui, "Step (− = down)", step, -1000..=1000);
                            drag_row(ui, "Min", min, 0..=65535);
                            drag_row(ui, "Max", max, 0..=65535);
                        });
                        ui.checkbox(wrap, "Wrap around at the ends");
                        ui.checkbox(repeat, "Repeat while held");
                    }
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

        // UART example, built before borrowing the VP mutably.
        let example = alloc.as_ref().map(|a| {
            let (hi, lo) = (a.vp >> 8, a.vp & 0xFF);
            match &w.kind {
                WidgetKind::Button { event: Event::Key { key, .. }, .. } => {
                    format!("Screen → MCU on press:\n5A A5 06 83 {hi:02X} {lo:02X} 01 {:02X} {:02X}", key >> 8, key & 0xFF)
                }
                WidgetKind::Text { .. } => format!("MCU → screen (text \"Hi\"):\n5A A5 05 82 {hi:02X} {lo:02X} 48 69"),
                WidgetKind::Toggle { .. } | WidgetKind::Slider { .. } => format!(
                    "Screen → MCU when changed:\n5A A5 06 83 {hi:02X} {lo:02X} 01 00 <v>\nMCU → screen (set 1):\n5A A5 05 82 {hi:02X} {lo:02X} 00 01"
                ),
                _ => format!("MCU → screen (value 1):\n5A A5 05 82 {hi:02X} {lo:02X} 00 01"),
            }
        });
        if let Some(vp) = w.kind.vp_mut() {
            CollapsingHeader::new("DWIN ADDRESS").default_open(true).show(ui, |ui| {
                vp_editor(ui, vp, alloc.as_ref().map(|a| a.vp));
                if let Some(sp) = alloc.as_ref().and_then(|a| a.sp) {
                    ui.horizontal(|ui| {
                        ui.label("SP");
                        ui.monospace(format!("{sp:#06x}")).on_hover_text("Description pointer: change position/color at runtime");
                    });
                }
                if let Some(e) = example {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).monospace().small().weak());
                }
            });
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        let (pw, ph) = (self.project.width as f32, self.project.height as f32);

        // Zoom (Ctrl+wheel / pinch) around the pointer; wheel and middle-drag pan.
        if resp.hovered() {
            let (zd, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
            let fit = ((area.width() - 24.0) / pw).min((area.height() - 24.0) / ph).max(0.05);
            if zd != 1.0 {
                self.zoom = Some((self.zoom.unwrap_or(fit) * zd).clamp(0.1, 8.0));
            } else if scroll != Vec2::ZERO {
                self.pan += scroll;
            }
        }
        if resp.dragged_by(PointerButton::Middle) {
            self.pan += resp.drag_delta();
        }
        let fit = ((area.width() - 24.0) / pw).min((area.height() - 24.0) / ph).max(0.05);
        let scale = self.zoom.unwrap_or(fit);
        let rect = Rect::from_center_size(area.center() + self.pan, Vec2::new(pw, ph) * scale);

        let key = (self.project.pages[self.page].clone(), self.project.width, self.project.height, self.show_pressed);
        if self.rendered.as_ref() != Some(&key) {
            let mode = if self.show_pressed { Mode::Pressed } else { Mode::Preview };
            let px = render(&key.0, key.1, key.2, mode, &self.base());
            let img = egui::ColorImage::from_rgba_premultiplied([px.width() as usize, px.height() as usize], px.data());
            self.texture = Some(ui.ctx().load_texture("page", img, egui::TextureOptions::LINEAR));
            self.rendered = Some(key);
        }
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, Color32::from_gray(24));
        if let Some(t) = &self.texture {
            painter.image(t.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let to_screen = |x: i32, y: i32| rect.min + Vec2::new(x as f32, y as f32) * scale;
        let to_proj = |p: Pos2| ((p - rect.min) / scale).to_pos2();
        let wrect = |w: &Widget| Rect::from_min_max(to_screen(w.x, w.y), to_screen(w.x + w.w, w.y + w.h));
        let grid = self.grid.max(1);
        let snap = move |v: f32| (v / grid as f32).round() as i32 * grid;

        for w in self.widgets() {
            if matches!(w.kind, WidgetKind::Number { .. } | WidgetKind::Text { .. } | WidgetKind::Clock { .. } | WidgetKind::Qr { .. }) {
                painter.rect_stroke(wrect(w), 0.0, Stroke::new(1.0, Color32::from_gray(110)), StrokeKind::Inside);
            }
        }

        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let ws = &self.project.pages[self.page].widgets; // field borrow, disjoint from self.selected
                let handle = match self.selected[..] {
                    [s] if Rect::from_center_size(wrect(&ws[s]).max, Vec2::splat(HANDLE * 1.6)).contains(p) => Some(s),
                    _ => None,
                };
                let hit = ws.iter().rposition(|w| wrect(w).contains(p));
                self.drag = if let Some(s) = handle {
                    Some(Drag::Resize { start: to_proj(p), idx: s, orig: (ws[s].w, ws[s].h) })
                } else if let Some(i) = hit {
                    if !self.selected.contains(&i) {
                        if !shift {
                            self.selected.clear();
                        }
                        self.selected.push(i);
                    }
                    let orig = self.selected.iter().map(|&s| (s, ws[s].x, ws[s].y)).collect();
                    Some(Drag::Move { start: to_proj(p), orig })
                } else {
                    if !shift {
                        self.selected.clear();
                    }
                    Some(Drag::Box { start: p })
                };
                if !matches!(self.drag, Some(Drag::Box { .. })) {
                    self.undo.push(self.project.clone());
                    self.redo.clear();
                }
            }
        }
        if let (Some(d), Some(p)) = (&self.drag, resp.interact_pointer_pos()) {
            match d {
                Drag::Move { start, orig } if resp.dragged() => {
                    let delta = to_proj(p) - *start;
                    let (dx, dy) = (snap(delta.x), snap(delta.y));
                    for &(i, x, y) in orig.clone().iter() {
                        let w = &mut self.widgets_mut()[i];
                        (w.x, w.y) = (x + dx, y + dy);
                    }
                }
                Drag::Resize { start, idx, orig } if resp.dragged() => {
                    let delta = to_proj(p) - *start;
                    let (i, (w0, h0)) = (*idx, *orig);
                    let w = &mut self.widgets_mut()[i];
                    (w.w, w.h) = ((w0 + snap(delta.x)).max(grid), (h0 + snap(delta.y)).max(grid));
                }
                Drag::Box { start } => {
                    let band = Rect::from_two_pos(*start, p);
                    painter.rect(band, 0.0, Color32::from_rgba_unmultiplied(56, 139, 253, 25), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
                    if resp.drag_stopped() {
                        let hits: Vec<usize> = self.widgets().iter().enumerate().filter(|(_, w)| band.intersects(wrect(w))).map(|(i, _)| i).collect();
                        for i in hits {
                            if !self.selected.contains(&i) {
                                self.selected.push(i);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if resp.clicked() {
            let p = resp.interact_pointer_pos().unwrap();
            let hit = self.widgets().iter().rposition(|w| wrect(w).contains(p));
            match (hit, shift) {
                (Some(i), true) => {
                    if let Some(pos) = self.selected.iter().position(|&s| s == i) {
                        self.selected.remove(pos);
                    } else {
                        self.selected.push(i);
                    }
                }
                (Some(i), false) => self.selected = vec![i],
                (None, _) => self.selected.clear(),
            }
        }
        for &s in &self.selected {
            let w = &self.widgets()[s];
            let r = wrect(w);
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, ACCENT), StrokeKind::Outside);
            if self.selected.len() == 1 {
                painter.rect_filled(Rect::from_center_size(r.max, Vec2::splat(HANDLE)), 2.0, ACCENT);
                let info = format!("{}  {},{}  {}×{}", w.name, w.x, w.y, w.w, w.h);
                painter.text(r.left_top() - Vec2::new(0.0, 3.0), egui::Align2::LEFT_BOTTOM, info, egui::FontId::proportional(11.0), ACCENT);
            }
        }
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::DARK_GRAY), StrokeKind::Outside);
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
                    let fixed = w.kind.vp().flatten().is_some();
                    ui.monospace(format!("{:#06x}{}", a.vp, if fixed { "" } else { " auto" }));
                    ui.monospace(match &w.kind {
                        WidgetKind::Button { event: Event::Key { key, .. }, .. } => format!("{key:#06x}"),
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
    drag_row(ui, "Font size", size, 6.0..=200.0);
}

fn radius_row(ui: &mut egui::Ui, r: &mut f32) {
    drag_row(ui, "Radius", r, 0.0..=200.0);
}

fn drag_row<N: egui::emath::Numeric>(ui: &mut egui::Ui, label: &str, v: &mut N, range: std::ops::RangeInclusive<N>) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).range(range));
    ui.end_row();
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [u8; 3]) {
    ui.label(label);
    ui.color_edit_button_srgb(c);
    ui.end_row();
}

/// Pick an imported asset (or none).
fn asset_pick(ui: &mut egui::Ui, id: &str, v: &mut Option<String>, assets: &[String]) {
    let text = v.as_deref().map_or("(none)", |s| s.trim_start_matches("assets/"));
    egui::ComboBox::from_id_salt(id).selected_text(text).show_ui(ui, |ui| {
        ui.selectable_value(v, None, "(none)");
        for a in assets {
            ui.selectable_value(v, Some(a.clone()), a.trim_start_matches("assets/"));
        }
        if assets.is_empty() {
            ui.label("Import images in ASSETS first");
        }
    });
}

fn kind_icon(k: &WidgetKind) -> &'static str {
    match k {
        WidgetKind::Button { .. } => "⏺",
        WidgetKind::Label { .. } => "A",
        WidgetKind::Panel { .. } => "▭",
        WidgetKind::Image { .. } => "▣",
        WidgetKind::Number { .. } => "#",
        WidgetKind::Text { .. } => "¶",
        WidgetKind::Toggle { .. } => "◑",
        WidgetKind::Lamp { .. } => "●",
        WidgetKind::Slider { .. } => "━",
        WidgetKind::Icons { .. } => "◈",
        WidgetKind::Dropdown { .. } => "▾",
        WidgetKind::Clock { .. } => "◷",
        WidgetKind::Qr { .. } => "▦",
        WidgetKind::Progress { .. } => "▬",
        WidgetKind::Chart { .. } => "∿",
        WidgetKind::Gauge { .. } => "◔",
        WidgetKind::Animation { .. } => "▶",
        WidgetKind::Drawing { .. } => "✎",
    }
}

/// Palette groups, EEZ-style. Names match `default_widgets()`; anything new lands in "More".
const GROUPS: &[(&str, &[&str])] = &[
    ("Basic", &["Label", "Panel", "Button", "+ / − Button"]),
    ("Input", &["Number Input", "Text Input", "Toggle", "Slider", "Knob", "Dropdown"]),
    ("Display", &["Number", "Big Number", "Text", "Scrolling Text", "Lamp", "Icons", "Progress", "Gauge", "Clock", "QR Code"]),
    ("Graphics", &["Chart", "Drawing", "Animation"]),
];

/// "Use one bit of the VP" option (bit flags packed in one word).
fn bit_row(ui: &mut egui::Ui, bit: &mut Option<u8>) {
    ui.label("Bit");
    ui.horizontal(|ui| {
        let mut on = bit.is_some();
        if ui.checkbox(&mut on, "one bit of VP").changed() {
            *bit = on.then_some(0);
        }
        if let Some(b) = bit {
            ui.add(egui::DragValue::new(b).range(0..=15));
        }
    });
    ui.end_row();
}

/// Ordered picture list (grid rows): index, name, move up, remove, then an add picker.
fn image_list(ui: &mut egui::Ui, images: &mut Vec<String>, assets: &[String]) {
    let mut remove = None;
    let mut up = None;
    for (i, p) in images.iter().enumerate() {
        ui.monospace(format!("{i}"));
        ui.horizontal(|ui| {
            ui.label(p.trim_start_matches("assets/"));
            if i > 0 && ui.small_button("⬆").clicked() {
                up = Some(i);
            }
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
        ui.end_row();
    }
    if let Some(i) = remove {
        images.remove(i);
    }
    if let Some(i) = up {
        images.swap(i, i - 1);
    }
    ui.label("");
    let mut add = None;
    asset_pick(ui, "addicon", &mut add, assets);
    if let Some(a) = add {
        images.push(a);
    }
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
        if self.sim.is_some() {
            self.pump();
            egui::Panel::top("toolbar").show(ui, |ui| {
                ui.add_space(4.0);
                self.toolbar(ui);
                ui.add_space(4.0);
            });
            egui::Panel::right("simvars").default_size(300.0).show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.sim_vars(ui));
            });
            egui::Panel::bottom("simlog").resizable(true).default_size(180.0).show(ui, |ui| self.sim_panel(ui));
            egui::CentralPanel::no_frame().show(ui, |ui| self.sim_canvas(ui));
            self.pump();
            return;
        }
        self.shortcuts(&ui.ctx().clone());
        let (allocs, warnings) = allocate(&self.project);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::left("left").default_size(250.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.left_panel(ui));
        });
        egui::Panel::right("inspector").default_size(310.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui, &allocs));
        });
        egui::Panel::bottom("map").resizable(true).default_size(150.0).show(ui, |ui| self.address_map(ui, &allocs, &warnings));
        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));

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
