//! MCP server (`uwin-studio mcp`): lets an AI assistant (Claude Desktop, Claude Code, ...)
//! create and edit projects, build DWIN_SET and drive the simulator.
//! JSON-RPC 2.0 over stdio, one message per line.

use crate::build::build;
use crate::project::{default_widgets, Page, Project, Widget, WidgetKind};
use crate::render::{render, to_image, Mode};
use crate::sim::Sim;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const INSTRUCTIONS: &str = r#"UWIN Studio designs touch screens for DWIN T5L (DGUS II) displays and builds the DWIN_SET folder that goes on the display's SD card.

Typical job: the user gives a flow chart / list of screens and what each button does. Then:
1. new_project (path ending in .uwin, screen size) or open_project.
2. One add_screen per screen in the flow chart. Screens are numbered from 0 in creation order.
3. add_widget for everything on each screen. widget_types lists every type with its default properties.
   - Navigation button: type "Button", props {"text": "...", "goto": <screen index>, "event": "None"}.
   - Button that tells the MCU something: props {"event": {"Key": {"vp": null, "key": 1}}} (sends key to its VP).
   - On/off button that stays pressed: type "On/Off Button".
   - +/- button changing a number: type "+ / − Button", props {"event": {"Adjust": {"target": "<number widget name>", "step": 1, "min": 0, "max": 100, "wrap": false, "repeat": true}}}.
   - Values the MCU shows: Number, Text, Progress, Gauge, Lamp, Icons. Values the user enters: Number Input, Text Input, Toggle, Slider, Knob, Dropdown, List, Roller.
   - Put a short "comment" on every widget saying what it does for the MCU; it goes into ui_map.txt and ui_gen.h.
   - Widget names must be unique in the project (they become C #defines). Use snake_case.
   - Coordinates are pixels, origin top-left. Keep widgets inside the screen and not overlapping (Panels are backgrounds and may contain others).
   - Leave "vp" null: addresses are assigned automatically. Colors are [r, g, b].
   - Layout: use the whole screen with even margins (about 24 px), line things up on a grid, centre titles,
     group related items on a Panel, make touch targets at least 60 px tall, keep navigation buttons in the
     same place on every screen, and use the default dark colors unless the user asks otherwise.
4. screenshot each screen to check the layout, fix with update_widget.
5. build: writes DWIN_SET, ui_map.txt (all addresses + UART frames) and ui_gen.h. Fix any warnings.
6. Optionally test: sim_start, sim_tap / sim_drag / sim_type, sim_read, sim_send (frames from the MCU).
Edits are saved to the .uwin file right away; the UWIN Studio editor shows them after reopening the file."#;

struct Server {
    project: Project,
    path: Option<PathBuf>,
    sim: Option<Sim>,
}

/// Run until stdin closes.
pub fn serve(input: impl BufRead, mut output: impl Write) {
    let mut s = Server { project: Project::default(), path: None, sim: None };
    for line in input.lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue }; // notification: nothing to answer
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let reply = match msg["method"].as_str().unwrap_or("") {
            "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "uwin-studio", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }}),
            "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}),
            "tools/call" => {
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = match s.call(params["name"].as_str().unwrap_or(""), &args) {
                    Ok(content) => json!({"content": content}),
                    Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
                };
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            m => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("unknown method {m}")}}),
        };
        let _ = writeln!(output, "{reply}");
        let _ = output.flush();
    }
}

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": description,
           "inputSchema": {"type": "object", "properties": props, "required": required}})
}

fn tools() -> Vec<Value> {
    let screen = json!({"type": ["integer", "string"], "description": "Screen index (0, 1, ...) or name"});
    let int = |d: &str| json!({"type": "integer", "description": d});
    let props = json!({"type": "object", "description": "Type-specific properties to set, e.g. {\"text\": \"Start\", \"goto\": 2} (see widget_types)"});
    vec![
        tool("new_project", "Start a new project and save it to `path` (.uwin).", json!({
            "path": {"type": "string"}, "width": int("Screen width in pixels (default 800)"), "height": int("Screen height (default 480)")}), &["path"]),
        tool("open_project", "Open an existing .uwin project.", json!({"path": {"type": "string"}}), &["path"]),
        tool("get_project", "Summary of screens and widgets. full=true returns the whole project JSON.", json!({"full": {"type": "boolean"}}), &[]),
        tool("set_project", "Replace the whole project with this JSON (same format as get_project full=true). Fastest way to create many screens at once.",
            json!({"project": {"type": "object"}}), &["project"]),
        tool("widget_types", "Every widget type with its default properties and size.", json!({}), &[]),
        tool("add_screen", "Add a screen; returns its index (used by button goto).", json!({
            "name": {"type": "string"}, "bg": {"type": "array", "items": {"type": "integer"}, "description": "Background [r,g,b]"}}), &["name"]),
        tool("update_screen", "Rename a screen, change its background, or set which screens a finger swipe opens.", json!({
            "screen": screen, "name": {"type": "string"}, "bg": {"type": "array", "items": {"type": "integer"}},
            "swipe_left": int("Screen opened by swiping left (-1 = none)"), "swipe_right": int("Screen opened by swiping right (-1 = none)")}), &["screen"]),
        tool("remove_screen", "Delete a screen (later screens move down by one; goto targets are fixed up).", json!({"screen": screen}), &["screen"]),
        tool("add_widget", "Add a widget to a screen.", json!({
            "screen": screen, "type": {"type": "string", "description": "Name from widget_types, e.g. \"Button\""},
            "name": {"type": "string", "description": "Unique snake_case name"}, "x": int("Left"), "y": int("Top"),
            "w": int("Width (default: type's default)"), "h": int("Height"), "comment": {"type": "string"}, "props": props}),
            &["screen", "type", "name", "x", "y"]),
        tool("update_widget", "Change a widget found by name: position, size, comment, new name, or type properties.", json!({
            "name": {"type": "string"}, "new_name": {"type": "string"}, "x": int(""), "y": int(""), "w": int(""), "h": int(""),
            "comment": {"type": "string"}, "props": props}), &["name"]),
        tool("remove_widget", "Delete a widget by name.", json!({"name": {"type": "string"}}), &["name"]),
        tool("build", "Build DWIN_SET + ui_map.txt + ui_gen.h into out_dir (default: 'build' next to the project). Returns warnings and the address map.",
            json!({"out_dir": {"type": "string"}}), &[]),
        tool("screenshot", "PNG of a screen as designed, or of the running simulator when screen is omitted and sim_start was called.",
            json!({"screen": screen}), &[]),
        tool("sim_start", "Start the simulator (the screen behaves like the real display).", json!({"screen": screen}), &[]),
        tool("sim_tap", "Tap at x,y on the simulated screen. Returns frames sent to the MCU, the current screen and any open keypad/list.",
            json!({"x": int(""), "y": int("")}), &["x", "y"]),
        tool("sim_drag", "Press at x0,y0, move to x1,y1, release (sliders, knobs, swipes).", json!({
            "x0": int(""), "y0": int(""), "x1": int(""), "y1": int("")}), &["x0", "y0", "x1", "y1"]),
        tool("sim_type", "Type into the open keypad/keyboard; enter=true presses OK.", json!({
            "text": {"type": "string"}, "enter": {"type": "boolean"}}), &["text"]),
        tool("sim_send", "Bytes from the MCU to the screen, hex like \"5A A5 05 82 10 00 00 2A\". Returns the screen's replies.",
            json!({"hex": {"type": "string"}}), &["hex"]),
        tool("sim_read", "Current screen and the value of every widget in the simulator.", json!({}), &[]),
    ]
}

fn text(s: impl Into<String>) -> Value {
    json!({"type": "text", "text": s.into()})
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Set fields inside the kind's JSON object ({"Button": {...}}) and parse it back.
fn merge_kind(kind: &WidgetKind, props: &Value) -> Result<WidgetKind, String> {
    let Some(props) = props.as_object() else { return Ok(kind.clone()) };
    let mut v = serde_json::to_value(kind).map_err(|e| e.to_string())?;
    let (tag, inner) = v.as_object_mut().and_then(|o| o.iter_mut().next()).ok_or("bad kind")?;
    let tag = tag.clone();
    let inner = inner.as_object_mut().ok_or("bad kind")?;
    for (k, val) in props {
        if !inner.contains_key(k) {
            let known: Vec<_> = inner.keys().cloned().collect();
            return Err(format!("{tag} has no property '{k}'; it has: {}", known.join(", ")));
        }
        inner.insert(k.clone(), val.clone());
    }
    serde_json::from_value(v).map_err(|e| format!("bad value for {tag}: {e}"))
}

fn int(a: &Value, k: &str) -> Option<i32> {
    a.get(k).and_then(Value::as_i64).map(|v| v as i32)
}

fn rgb(v: &Value) -> Option<[u8; 3]> {
    serde_json::from_value(v.clone()).ok()
}

impl Server {
    fn base(&self) -> PathBuf {
        self.path.as_deref().and_then(Path::parent).map_or(PathBuf::from("."), Path::to_path_buf)
    }

    fn save(&self) -> Result<String, String> {
        let Some(p) = &self.path else { return Ok(" (not saved: call new_project or open_project first)".into()) };
        std::fs::write(p, serde_json::to_string_pretty(&self.project).unwrap()).map_err(|e| format!("save {}: {e}", p.display()))?;
        Ok(String::new())
    }

    fn screen(&self, v: &Value) -> Result<usize, String> {
        let pages = &self.project.pages;
        let i = match v {
            Value::Number(n) => n.as_u64().map(|n| n as usize),
            Value::String(s) => pages.iter().position(|p| p.name == *s).or(s.parse().ok()),
            _ => None,
        };
        i.filter(|&i| i < pages.len()).ok_or_else(|| {
            let names: Vec<_> = pages.iter().enumerate().map(|(i, p)| format!("{i}={}", p.name)).collect();
            format!("no screen {v}; screens: {}", names.join(", "))
        })
    }

    fn find(&self, name: &str) -> Result<(usize, usize), String> {
        self.project.pages.iter().enumerate()
            .find_map(|(pi, p)| p.widgets.iter().position(|w| w.name == name).map(|wi| (pi, wi)))
            .ok_or_else(|| format!("no widget named '{name}'"))
    }

    fn summary(&self) -> String {
        let p = &self.project;
        let mut s = format!("{} x {}, {} screen(s){}\n", p.width, p.height, p.pages.len(),
            self.path.as_ref().map_or(String::new(), |f| format!(", file {}", f.display())));
        for (i, pg) in p.pages.iter().enumerate() {
            s += &format!("\nscreen {i} '{}' bg {:?} swipe right->{:?} left->{:?}\n", pg.name, pg.bg, pg.swipe.0, pg.swipe.1);
            for w in &pg.widgets {
                let extra = match &w.kind {
                    WidgetKind::Button { text, goto, event, .. } => format!(" \"{text}\" goto={goto:?} event={}", serde_json::to_string(event).unwrap()),
                    WidgetKind::Label { text, .. } => format!(" \"{text}\""),
                    _ => String::new(),
                };
                s += &format!("  {} [{}] {},{} {}x{}{extra}{}\n", w.name, w.kind_name(), w.x, w.y, w.w, w.h,
                    if w.comment.is_empty() { String::new() } else { format!("  // {}", w.comment) });
            }
        }
        s
    }

    fn sim_state(&mut self) -> String {
        let Some(sim) = &mut self.sim else { return String::new() };
        let frames: Vec<_> = sim.out.drain(..).map(|f| hex(&f)).collect();
        let page = &self.project.pages[sim.page.min(self.project.pages.len() - 1)];
        let mut s = format!("screen {} '{}'", sim.page, page.name);
        if let Some(p) = &sim.popup {
            s += &format!("; keypad/list open (typed \"{}\"): use sim_type, or sim_tap on it", p.typed);
        }
        s += &if frames.is_empty() { "\nno frames to MCU".into() } else { format!("\nto MCU:\n{}", frames.join("\n")) };
        s
    }

    fn call(&mut self, name: &str, a: &Value) -> Result<Vec<Value>, String> {
        let s = |a: &Value, k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
        let msg = match name {
            "new_project" => {
                let path = PathBuf::from(s(a, "path").ok_or("path missing")?);
                let mut p = Project::default();
                p.width = int(a, "width").unwrap_or(800) as u16;
                p.height = int(a, "height").unwrap_or(480) as u16;
                p.pages[0].name = "main".into();
                (self.project, self.path, self.sim) = (p, Some(path.clone()), None);
                format!("new {}x{} project with screen 0 'main'{}", self.project.width, self.project.height, self.save()?)
            }
            "open_project" => {
                let path = PathBuf::from(s(a, "path").ok_or("path missing")?);
                let txt = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                self.project = serde_json::from_str(&txt).map_err(|e| format!("not a UWIN Studio project: {e}"))?;
                (self.path, self.sim) = (Some(path), None);
                self.summary()
            }
            "get_project" if a["full"].as_bool() == Some(true) => serde_json::to_string(&self.project).unwrap(),
            "get_project" => self.summary(),
            "set_project" => {
                self.project = serde_json::from_value(a["project"].clone()).map_err(|e| format!("bad project JSON: {e}"))?;
                if self.project.pages.is_empty() {
                    self.project.pages.push(Page { name: "main".into(), bg: [15, 23, 42], bg_image: None, swipe: (None, None), widgets: vec![] });
                }
                self.sim = None;
                format!("project replaced{}\n{}", self.save()?, self.summary())
            }
            "widget_types" => {
                let list: Vec<Value> = default_widgets().into_iter().map(|(n, k)| {
                    let w = Widget::new(k.clone(), "x", 0, 0);
                    json!({"type": n, "default_size": [w.w, w.h], "props": serde_json::to_value(&k).unwrap().as_object().unwrap().values().next()})
                }).collect();
                let mut out = serde_json::to_string(&list).unwrap();
                out += "\nAlso: \"Image\" with props {\"path\": \"assets/picture.png\"} (relative to the project file).";
                out += "\nButton event: \"None\" | {\"Key\": {\"vp\": null, \"key\": n}} | {\"Latch\": {\"vp\": null}} | {\"Adjust\": {\"target\": name, \"step\": n, \"min\": n, \"max\": n, \"wrap\": bool, \"repeat\": bool}}; goto = screen index or null.";
                out
            }
            "add_screen" => {
                let name = s(a, "name").ok_or("name missing")?;
                let bg = rgb(&a["bg"]).unwrap_or([15, 23, 42]);
                self.project.pages.push(Page { name: name.clone(), bg, bg_image: None, swipe: (None, None), widgets: vec![] });
                format!("added screen {} '{name}'{}", self.project.pages.len() - 1, self.save()?)
            }
            "update_screen" => {
                let i = self.screen(&a["screen"])?;
                let n = self.project.pages.len() as i32;
                let pg = &mut self.project.pages[i];
                if let Some(v) = s(a, "name") {
                    pg.name = v;
                }
                if let Some(v) = rgb(&a["bg"]) {
                    pg.bg = v;
                }
                let target = |v: i32| (0..n).contains(&v).then_some(v as usize);
                if let Some(v) = int(a, "swipe_right") {
                    pg.swipe.0 = target(v);
                }
                if let Some(v) = int(a, "swipe_left") {
                    pg.swipe.1 = target(v);
                }
                format!("screen {i} updated{}", self.save()?)
            }
            "remove_screen" => {
                let i = self.screen(&a["screen"])?;
                if self.project.pages.len() == 1 {
                    return Err("can't remove the only screen".into());
                }
                self.project.pages.remove(i);
                let fix = |t: &mut Option<usize>| {
                    *t = match *t {
                        Some(g) if g == i => None,
                        Some(g) if g > i => Some(g - 1),
                        g => g,
                    }
                };
                for pg in &mut self.project.pages {
                    fix(&mut pg.swipe.0);
                    fix(&mut pg.swipe.1);
                    for w in &mut pg.widgets {
                        if let WidgetKind::Button { goto, .. } = &mut w.kind {
                            fix(goto);
                        }
                    }
                }
                self.sim = None;
                format!("removed screen {i}{}", self.save()?)
            }
            "add_widget" => {
                let pi = self.screen(&a["screen"])?;
                let ty = s(a, "type").ok_or("type missing")?;
                let name = s(a, "name").ok_or("name missing")?;
                if self.find(&name).is_ok() {
                    return Err(format!("a widget named '{name}' already exists"));
                }
                let kind = match default_widgets().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(&ty)) {
                    Some((_, k)) => k,
                    None if ty.eq_ignore_ascii_case("image") => WidgetKind::Image { path: String::new() },
                    None => {
                        let names: Vec<_> = default_widgets().into_iter().map(|(n, _)| n).collect();
                        return Err(format!("unknown type '{ty}'; types: {}, Image", names.join(", ")));
                    }
                };
                let kind = merge_kind(&kind, &a["props"])?;
                let mut w = Widget::new(kind, &name, int(a, "x").unwrap_or(0), int(a, "y").unwrap_or(0));
                w.w = int(a, "w").unwrap_or(w.w);
                w.h = int(a, "h").unwrap_or(w.h);
                w.comment = s(a, "comment").unwrap_or_default();
                let msg = format!("added {} '{name}' at {},{} {}x{} on screen {pi}", w.kind_name(), w.x, w.y, w.w, w.h);
                self.project.pages[pi].widgets.push(w);
                format!("{msg}{}", self.save()?)
            }
            "update_widget" => {
                let name = s(a, "name").ok_or("name missing")?;
                let (pi, wi) = self.find(&name)?;
                if let Some(n) = s(a, "new_name").filter(|n| *n != name) {
                    if self.find(&n).is_ok() {
                        return Err(format!("a widget named '{n}' already exists"));
                    }
                }
                let w = &mut self.project.pages[pi].widgets[wi];
                w.kind = merge_kind(&w.kind, &a["props"])?;
                for (k, f) in [("x", &mut w.x), ("y", &mut w.y), ("w", &mut w.w), ("h", &mut w.h)] {
                    if let Some(v) = int(a, k) {
                        *f = v;
                    }
                }
                if let Some(c) = s(a, "comment") {
                    w.comment = c;
                }
                if let Some(n) = s(a, "new_name") {
                    w.name = n;
                }
                let msg = format!("updated '{}': {},{} {}x{}", w.name, w.x, w.y, w.w, w.h);
                format!("{msg}{}", self.save()?)
            }
            "remove_widget" => {
                let name = s(a, "name").ok_or("name missing")?;
                let (pi, wi) = self.find(&name)?;
                self.project.pages[pi].widgets.remove(wi);
                format!("removed '{name}'{}", self.save()?)
            }
            "build" => {
                let out = s(a, "out_dir").map_or(self.base().join("build"), PathBuf::from);
                let o = build(&self.project, &self.base()).map_err(|e| format!("build failed: {e}"))?;
                o.set.write(&out.join("DWIN_SET")).map_err(|e| e.to_string())?;
                std::fs::write(out.join("ui_gen.h"), &o.header).map_err(|e| e.to_string())?;
                std::fs::write(out.join("ui_map.txt"), &o.map).map_err(|e| e.to_string())?;
                let warn = if o.warnings.is_empty() { "no warnings".into() } else { format!("WARNINGS:\n{}", o.warnings.join("\n")) };
                format!("built {} screen(s) into {}\nCopy DWIN_SET to the SD card; include ui_gen.h in the MCU code.\n{warn}\n\n{}",
                    self.project.pages.len(), out.display(), o.map)
            }
            "screenshot" => {
                let base = self.base();
                let (px, what) = match (&self.sim, a.get("screen")) {
                    (Some(sim), None) => (sim.render(&self.project, &base), format!("simulator, screen {}", sim.page)),
                    _ => {
                        let i = a.get("screen").map_or(Ok(0), |v| self.screen(v))?;
                        let p = &self.project;
                        (render(&p.pages[i], p.width, p.height, Mode::Preview, &base), format!("screen {i} '{}'", p.pages[i].name))
                    }
                };
                let mut png = std::io::Cursor::new(vec![]);
                to_image(&px).write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
                return Ok(vec![text(what), json!({"type": "image", "data": base64(png.get_ref()), "mimeType": "image/png"})]);
            }
            "sim_start" => {
                let mut sim = Sim::new(&self.project);
                sim.page = a.get("screen").map_or(Ok(0), |v| self.screen(v))?;
                self.sim = Some(sim);
                format!("simulator running. {}", self.sim_state())
            }
            "sim_tap" | "sim_drag" | "sim_type" | "sim_send" | "sim_read" => {
                let p = &self.project;
                let sim = self.sim.as_mut().ok_or("call sim_start first")?;
                match name {
                    "sim_tap" => {
                        let (x, y) = (int(a, "x").ok_or("x missing")?, int(a, "y").ok_or("y missing")?);
                        sim.press(p, x, y);
                        sim.release(p, x);
                    }
                    "sim_drag" => {
                        let g = |k| int(a, k).ok_or(format!("{k} missing"));
                        let (x0, y0, x1, y1) = (g("x0")?, g("y0")?, g("x1")?, g("y1")?);
                        sim.press(p, x0, y0);
                        for i in 1..=8 {
                            sim.drag(p, x0 + (x1 - x0) * i / 8, y0 + (y1 - y0) * i / 8);
                        }
                        sim.release(p, x1);
                    }
                    "sim_type" => {
                        if sim.popup.is_none() {
                            return Err("no keypad/keyboard open: sim_tap an input field first".into());
                        }
                        s(a, "text").unwrap_or_default().chars().for_each(|c| sim.type_char(c));
                        if a["enter"].as_bool() == Some(true) {
                            sim.key(0xF1);
                        }
                    }
                    "sim_send" => {
                        let raw = s(a, "hex").ok_or("hex missing")?;
                        let clean: String = raw.chars().filter(char::is_ascii_hexdigit).collect();
                        if clean.len() % 2 == 1 {
                            return Err("odd number of hex digits".into());
                        }
                        let bytes: Vec<u8> = (0..clean.len()).step_by(2).map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap()).collect();
                        sim.feed(&bytes);
                    }
                    _ => {
                        let mut s = String::new();
                        for al in &sim.allocs {
                            let w = &p.pages[al.page].widgets[al.widget];
                            let v = match &w.kind {
                                WidgetKind::Text { len, unicode: true, .. } => format!("\"{}\"", sim.utf16_at(al.vp, *len)),
                                WidgetKind::Text { len, .. } | WidgetKind::Qr { len, .. } => format!("\"{}\"", sim.text_at(al.vp, *len)),
                                _ => sim.get(al.vp).to_string(),
                            };
                            s += &format!("{} (screen {}, VP {:#06x}) = {v}\n", al.name, al.page, al.vp);
                        }
                        return Ok(vec![text(format!("{}\n{s}", self.sim_state()))]);
                    }
                }
                self.sim_state()
            }
            _ => return Err(format!("unknown tool {name}")),
        };
        Ok(vec![text(msg)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(lines: &[Value]) -> Vec<Value> {
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut out = vec![];
        serve(input.as_bytes(), &mut out);
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn call(id: u32, name: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
    }

    #[test]
    fn flow_chart_to_working_screens() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        let dir = std::env::temp_dir().join("uwin_mcp_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.uwin");
        let r = rpc(&[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            call(3, "new_project", json!({"path": path})),
            call(4, "add_screen", json!({"name": "settings"})),
            call(5, "add_widget", json!({"screen": 0, "type": "Button", "name": "to_settings", "x": 10, "y": 10,
                "props": {"text": "Settings", "goto": 1, "event": "None"}})),
            call(6, "add_widget", json!({"screen": "settings", "type": "button", "name": "start", "x": 10, "y": 10,
                "comment": "Start the pump"})),
            call(7, "add_widget", json!({"screen": 0, "type": "Button", "name": "bad", "x": 0, "y": 0, "props": {"colour": 1}})),
            call(8, "sim_start", json!({})),
            call(9, "sim_tap", json!({"x": 20, "y": 20})),
            call(10, "sim_tap", json!({"x": 20, "y": 20})),
            call(11, "build", json!({})),
            call(12, "screenshot", json!({"screen": 1})),
            call(13, "update_widget", json!({"name": "start", "x": 40, "props": {"text": "GO"}})),
            call(14, "sim_read", json!({})),
        ]);
        assert_eq!(r.len(), 14, "one reply per request, none for the notification");
        assert_eq!(r[0]["result"]["serverInfo"]["name"], "uwin-studio");
        assert!(r[1]["result"]["tools"].as_array().unwrap().len() >= 15);
        let txt = |i: usize| r[i]["result"]["content"][0]["text"].as_str().unwrap().to_string();
        assert!(txt(4).contains("added Button 'to_settings'"), "{}", txt(4));
        assert_eq!(r[6]["result"]["isError"], true);
        assert!(txt(6).contains("no property 'colour'"));
        // Tap 1 goes to screen 1; tap 2 hits 'start' there and sends its key.
        assert!(txt(8).contains("screen 1 'settings'"), "{}", txt(8));
        assert!(txt(9).contains("5A A5 06 83"), "{}", txt(9));
        assert!(txt(10).contains("no warnings"), "{}", txt(10));
        assert!(dir.join("build/DWIN_SET/13TouchFile.bin").exists());
        assert_eq!(r[11]["result"]["content"][1]["mimeType"], "image/png");
        let saved: Project = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(matches!(&saved.pages[1].widgets[0].kind, WidgetKind::Button { text, .. } if text == "GO"));
        assert_eq!(saved.pages[1].widgets[0].x, 40);
        assert!(txt(13).contains("start (screen 1"), "{}", txt(13));
    }
}
