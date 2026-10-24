//! Python scripting: the host side of the script API (see
//! crates/reetched-script/PROTOCOL.md), the console and plugin windows, and
//! the Python menu.

use crate::{app::Selected, app::State, theme};
use egui::{RichText, vec2};
use reetched_doc::{
    BlendMode, Channel, Fill, Mask, MaskBackground, Node, NodeId, NodeKind, RESOLUTIONS,
    ShaderModel, find_node, find_node_mut, take_node,
};
use reetched_script::{Engine, LogLevel, ScriptHost};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Most console text kept.
const CONSOLE_CHARS: usize = 200_000;

#[derive(Default)]
pub struct Scripting {
    engine: Option<Engine>,
    /// Python started (the engine may be lent out to a call right now).
    on: bool,
    /// "Python 3.12.1", or why scripting is off.
    pub status: String,
    /// Events raised while the engine was busy; sent once it's free.
    events: Vec<(String, Value)>,
    /// (active texture set, history step) last reported to scripts.
    seen: (usize, u64),
    pub console_open: bool,
    pub plugins_open: bool,
    input: String,
    pub output: String,
}

impl Scripting {
    /// Timers may be waiting: keep frames coming while Python runs.
    pub fn wants_ticks(&self) -> bool {
        self.on
    }
}

impl State {
    /// Finds Python and starts the engine with the plugin folder; scripting
    /// stays off (with the reason in `script.status`) when that fails.
    pub fn start_scripting(&mut self) {
        let install = match &self.settings.python {
            Some(p) => reetched_script::python_at(p),
            None => reetched_script::find_python(),
        };
        let dir = plugins_dir();
        let _ = std::fs::create_dir_all(&dir);
        let started = install.and_then(|i| Engine::start(&i, &[dir]).map(|e| (i, e)));
        match started {
            Ok((install, engine)) => {
                self.script.status = format!("Python {}", install.version);
                self.script.engine = Some(engine);
                self.script.on = true;
                self.script.seen = (self.selected_part, self.history.steps());
                for name in self.settings.plugins_enabled.clone() {
                    if let Some(Err(e)) =
                        self.with_engine(|e, s| e.set_plugin_enabled(s, &name, true))
                    {
                        self.error(format!("Plugin {name} failed to start: {e}"));
                    }
                }
            }
            Err(e) => {
                self.script.status = format!("Scripting is off: {e:#}");
                log::info!("{}", self.script.status);
            }
        }
    }

    /// Lends the engine to `f` (with the app as its host), then sends the
    /// events `f` raised. None when Python isn't running or is busy.
    fn with_engine<R>(&mut self, f: impl FnOnce(&mut Engine, &mut State) -> R) -> Option<R> {
        let mut engine = self.script.engine.take()?;
        let r = f(&mut engine, self);
        self.script.engine = Some(engine);
        self.flush_script_events();
        Some(r)
    }

    /// Tells Python scripts and plugins about an event.
    pub fn notify_script(&mut self, name: &str, data: Value) {
        if self.script.on {
            self.script.events.push((name.to_owned(), data));
        }
        self.flush_script_events();
    }

    /// Sends queued events, unless the engine is busy (then its caller sends them).
    pub fn flush_script_events(&mut self) {
        // Callbacks can raise more events; stop after a few rounds.
        for _ in 0..8 {
            if self.script.events.is_empty() {
                return;
            }
            let Some(mut engine) = self.script.engine.take() else {
                return;
            };
            for (name, data) in std::mem::take(&mut self.script.events) {
                engine.event(self, &name, data);
            }
            self.script.engine = Some(engine);
        }
    }

    /// Once a frame: layer and texture set events, then due Python timers.
    pub fn script_frame(&mut self) {
        if !self.script.on {
            return;
        }
        let now = (self.selected_part, self.history.steps());
        if now != self.script.seen {
            let name = self.doc.texture_sets.get(now.0).map(|s| s.name.clone());
            if now.0 != self.script.seen.0 {
                self.notify_script("texture_set_changed", json!({ "texture_set": name }));
            }
            if now.1 != self.script.seen.1 {
                self.notify_script("layers_changed", json!({ "texture_set": name }));
            }
            self.script.seen = now;
        }
        self.with_engine(|e, s| e.tick(s));
    }

    /// The asset library folders changed in Settings.
    pub fn libraries_changed(&mut self) {
        self.library.sync_roots(&self.settings.library_roots);
    }

    /// Runs console input and appends the result to the console.
    pub fn run_console(&mut self, code: &str) {
        let echo: String = code.lines().map(|l| format!(">>> {l}\n")).collect();
        self.console_print(&echo);
        let Some(r) = self.with_engine(|e, s| e.run(s, code, "<console>")) else {
            let status = self.script.status.clone();
            self.console_print(&format!("{status}\n"));
            return;
        };
        self.console_print(&r.output);
        if let Some(e) = &r.error {
            self.console_print(e);
        }
    }

    fn console_print(&mut self, text: &str) {
        let out = &mut self.script.output;
        out.push_str(text);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if out.len() > CONSOLE_CHARS {
            let cut = out.len() - CONSOLE_CHARS;
            let cut = (cut..out.len())
                .find(|&i| out.is_char_boundary(i))
                .unwrap_or(cut);
            out.drain(..cut);
        }
    }

    pub fn run_script_file(&mut self, path: &Path) {
        self.console_print(&format!("# {}\n", path.display()));
        if let Some(r) = self.with_engine(|e, s| e.run_file(s, path)) {
            self.console_print(&r.output);
            if let Some(e) = &r.error {
                self.console_print(e);
                self.error(format!(
                    "{} failed; see the Python console",
                    crate::app::file_name(path)
                ));
            }
        }
    }

    /// The Python menu: console, plugins, script files and actions scripts registered.
    pub fn python_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Console").clicked() {
            self.script.console_open = true;
        }
        if ui.button("Plugins…").clicked() {
            self.script.plugins_open = true;
        }
        let on = self.script.on;
        if ui
            .add_enabled(on, egui::Button::new("Run script…"))
            .clicked()
            && let Some(p) = rfd::FileDialog::new()
                .add_filter("Python", &["py"])
                .pick_file()
        {
            self.run_script_file(&p);
            self.script.console_open = true;
        }
        if ui.button("Open the plugins folder").clicked() {
            let dir = plugins_dir();
            let _ = std::fs::create_dir_all(&dir);
            crate::settings_ui::open_folder(&dir);
        }
        let actions = self
            .script
            .engine
            .as_mut()
            .map(|e| e.actions())
            .unwrap_or_default();
        if !actions.is_empty() {
            ui.separator();
        }
        let mut clicked = None;
        for a in &actions {
            let parts: Vec<&str> = a.label.split('/').collect();
            action_item(ui, &parts, a.id, &mut clicked);
        }
        if !on {
            ui.separator();
            ui.label(
                RichText::new(&self.script.status)
                    .color(theme::WEAK)
                    .small(),
            );
        }
        if let Some(id) = clicked
            && let Some(r) = self.with_engine(|e, s| e.trigger_action(s, id))
        {
            self.console_print(&r.output);
            if let Some(e) = r.error {
                self.console_print(&e);
                self.error("A Python action failed; see the Python console".into());
            }
        }
    }

    pub fn console_window(&mut self, ctx: &egui::Context) {
        if !self.script.console_open {
            return;
        }
        let mut open = true;
        let mut run = None;
        egui::Window::new(theme::header("PYTHON CONSOLE"))
            .open(&mut open)
            .default_size(vec2(560.0, 360.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(&self.script.status)
                        .color(theme::WEAK)
                        .small(),
                );
                let input_h = 70.0;
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .max_height((ui.available_height() - input_h).max(60.0))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.script.output).monospace().small(),
                            )
                            .wrap(),
                        );
                    });
                ui.separator();
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut self.script.input)
                        .code_editor()
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("import reetched — Ctrl+Enter runs"),
                );
                let ctrl_enter = edit.has_focus()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Run").clicked() || ctrl_enter {
                        run = Some(std::mem::take(&mut self.script.input));
                    }
                    if ui.button("Clear").clicked() {
                        self.script.output.clear();
                    }
                });
            });
        if let Some(code) = run.filter(|c| !c.trim().is_empty()) {
            self.run_console(&code);
        }
        self.script.console_open = open;
    }

    pub fn plugins_window(&mut self, ctx: &egui::Context) {
        if !self.script.plugins_open {
            return;
        }
        let plugins = self
            .script
            .engine
            .as_mut()
            .map(|e| e.plugins())
            .unwrap_or_default();
        let mut open = true;
        let mut toggle = None;
        let mut reload = None;
        egui::Window::new(theme::header("PLUGINS"))
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                if !self.script.on {
                    ui.label(RichText::new(&self.script.status).color(theme::WEAK));
                    return;
                }
                if plugins.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "No plugins yet. Put .py files or packages in {}",
                            plugins_dir().display()
                        ))
                        .color(theme::WEAK),
                    );
                }
                for p in &plugins {
                    ui.horizontal(|ui| {
                        let mut on = p.enabled;
                        if ui.checkbox(&mut on, &p.name).changed() {
                            toggle = Some((p.name.clone(), on));
                        }
                        if ui.small_button("Reload").clicked() {
                            reload = Some(p.name.clone());
                        }
                        if p.enabled && !p.started {
                            ui.label(RichText::new("not running").color(theme::WEAK));
                        }
                    });
                    if let Some(e) = &p.error {
                        ui.label(RichText::new(e).monospace().small().color(theme::ERROR));
                    }
                }
            });
        if let Some((name, on)) = toggle {
            let r = self.with_engine(|e, s| e.set_plugin_enabled(s, &name, on));
            if let Some(Err(e)) = r {
                self.error(format!("Plugin {name}: {e}"));
            }
            self.settings.plugins_enabled.retain(|n| *n != name);
            if on {
                self.settings.plugins_enabled.push(name);
            }
        }
        if let Some(name) = reload
            && let Some(Err(e)) = self.with_engine(|e, s| e.reload_plugin(s, &name))
        {
            self.error(format!("Plugin {name}: {e}"));
        }
        self.script.plugins_open = open;
    }
}

fn plugins_dir() -> PathBuf {
    crate::settings::config_dir().join("plugins")
}

/// A menu entry at `parts` (submenus first).
fn action_item(ui: &mut egui::Ui, parts: &[&str], id: u64, clicked: &mut Option<u64>) {
    match parts {
        [] => {}
        [label] => {
            if ui.button(*label).clicked() {
                *clicked = Some(id);
            }
        }
        [menu, rest @ ..] => {
            ui.menu_button(*menu, |ui| action_item(ui, rest, id, clicked));
        }
    }
}

// ---------------------------------------------------------------- host calls

type Reply = Result<Value, String>;

/// A present, non-null argument.
fn arg<'a>(a: &'a Value, k: &str) -> Option<&'a Value> {
    a.get(k).filter(|v| !v.is_null())
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    arg(a, k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("'{k}' must be a string"))
}

fn opt_str<'a>(a: &'a Value, k: &str) -> Result<Option<&'a str>, String> {
    arg(a, k).map(|_| str_arg(a, k)).transpose()
}

fn opt_id(a: &Value, k: &str) -> Result<Option<NodeId>, String> {
    arg(a, k)
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| format!("'{k}' must be a layer id"))
        })
        .transpose()
}

fn id_arg(a: &Value, k: &str) -> Result<NodeId, String> {
    opt_id(a, k)?.ok_or_else(|| format!("'{k}' is required"))
}

fn channel(id: &str) -> Result<Channel, String> {
    Channel::from_id(id).ok_or_else(|| format!("unknown channel '{id}'"))
}

fn channels_arg(a: &Value, k: &str) -> Result<Option<Vec<Channel>>, String> {
    let Some(v) = arg(a, k) else { return Ok(None) };
    let list = v
        .as_array()
        .ok_or_else(|| format!("'{k}' must be a list of channels"))?;
    list.iter()
        .map(|c| channel(c.as_str().unwrap_or_default()))
        .collect::<Result<_, _>>()
        .map(Some)
}

fn blend_mode(name: &str) -> Result<BlendMode, String> {
    BlendMode::ALL
        .into_iter()
        .find(|m| format!("{m:?}") == name)
        .ok_or_else(|| format!("unknown blend mode '{name}'"))
}

fn shader_name(s: &ShaderModel) -> &'static str {
    match s {
        ShaderModel::MetalRough => "metal_rough",
        ShaderModel::Asm => "asm",
        ShaderModel::OpenPbr => "openpbr",
        ShaderModel::Custom { .. } => "custom",
    }
}

/// Scripts see resources as integers: the first 64 bits of the content hash.
fn resource_number(id: &str) -> u64 {
    u64::from_str_radix(id.get(..16).unwrap_or(id), 16).unwrap_or(0)
}

/// The list holding node `id`, and its index there.
fn list_of(stack: &mut Vec<Node>, id: NodeId) -> Option<(&mut Vec<Node>, usize)> {
    if let Some(i) = stack.iter().position(|n| n.id == id) {
        return Some((stack, i));
    }
    for n in stack.iter_mut() {
        if let NodeKind::Folder { children } = &mut n.kind
            && let Some(found) = list_of(children, id)
        {
            return Some(found);
        }
    }
    None
}

/// Puts `node` where `above` / `below` / `parent` say (PROTOCOL.md, layers.insert).
fn place(stack: &mut Vec<Node>, node: Node, a: &Value) -> Result<(), String> {
    if let Some(t) = opt_id(a, "above")? {
        let (list, i) = list_of(stack, t).ok_or("'above' is not a layer of this texture set")?;
        list.insert(i, node);
    } else if let Some(t) = opt_id(a, "below")? {
        let (list, i) = list_of(stack, t).ok_or("'below' is not a layer of this texture set")?;
        list.insert(i + 1, node);
    } else if let Some(p) = opt_id(a, "parent")? {
        match find_node_mut(stack, p).map(|n| &mut n.kind) {
            Some(NodeKind::Folder { children }) => children.insert(0, node),
            Some(_) => return Err("'parent' is not a group".into()),
            None => return Err("'parent' is not a layer of this texture set".into()),
        }
    } else {
        stack.insert(0, node);
    }
    Ok(())
}

fn node_json(n: &Node, channels: &[Channel]) -> Value {
    let kind = match &n.kind {
        NodeKind::Fill(_) => "fill",
        NodeKind::Paint(_) => "paint",
        NodeKind::Folder { .. } => "group",
    };
    let per_channel = |f: &dyn Fn(Channel) -> Value| -> Map<String, Value> {
        channels
            .iter()
            .map(|&c| (c.id().to_owned(), f(c)))
            .collect()
    };
    let mut v = json!({
        "id": n.id,
        "kind": kind,
        "name": n.name,
        "visible": n.visible,
        "blending": per_channel(&|c| json!(format!("{:?}", n.blend(c).mode))),
        "opacity": per_channel(&|c| json!(n.blend(c).opacity)),
        "has_mask": n.mask.is_some(),
    });
    match &n.kind {
        NodeKind::Fill(f) => {
            let active: Vec<&str> = f
                .channels
                .iter()
                .filter(|(c, s)| s.enabled && channels.contains(c))
                .map(|(c, _)| c.id())
                .collect();
            let fill = per_channel(&|c| match f.channels.get(&c) {
                Some(s) => match &s.bitmap {
                    Some(id) => json!({ "resource": resource_number(id) }),
                    None if c.is_vector() => json!(s.value),
                    None => json!(s.value[0]),
                },
                None => Value::Null,
            });
            v["active_channels"] = json!(active);
            v["fill"] = Value::Object(fill);
        }
        NodeKind::Folder { children } => {
            v["children"] = children.iter().map(|c| node_json(c, channels)).collect();
        }
        NodeKind::Paint(_) => {}
    }
    v
}

impl State {
    /// The text of the latest error in the log, for failed calls.
    fn last_error(&self, fallback: &str) -> String {
        self.log
            .iter()
            .rev()
            .find(|l| l.error)
            .map_or(fallback.to_owned(), |l| l.text.clone())
    }

    fn need_project(&self) -> Result<(), String> {
        if self.doc.texture_sets.is_empty() {
            Err("no project is open".into())
        } else {
            Ok(())
        }
    }

    fn set_index(&self, r: Option<&Value>) -> Result<usize, String> {
        self.need_project()?;
        let sets = &self.doc.texture_sets;
        match r {
            Some(Value::String(n)) => sets
                .iter()
                .position(|s| s.name == *n)
                .ok_or_else(|| format!("no texture set named '{n}'")),
            Some(Value::Number(n)) => n
                .as_u64()
                .map(|i| i as usize)
                .filter(|&i| i < sets.len())
                .ok_or_else(|| format!("no texture set {n}")),
            _ => Err("'texture_set' must be a texture set name or index".into()),
        }
    }

    /// Texture sets named in `k` (None = all), as an include list.
    fn sets_arg(&self, a: &Value, k: &str) -> Result<Vec<bool>, String> {
        let n = self.doc.texture_sets.len();
        let Some(v) = arg(a, k) else {
            return Ok(vec![true; n]);
        };
        let mut include = vec![false; n];
        for r in v
            .as_array()
            .ok_or_else(|| format!("'{k}' must be a list"))?
        {
            include[self.set_index(Some(r))?] = true;
        }
        Ok(include)
    }

    /// Which texture set holds layer `id`.
    fn locate(&self, id: NodeId) -> Result<usize, String> {
        self.doc
            .texture_sets
            .iter()
            .position(|s| find_node(&s.stack, id).is_some())
            .ok_or_else(|| format!("no layer with id {id}"))
    }

    fn host_call(&mut self, method: &str, a: &Value) -> Reply {
        match method {
            "project.info" => Ok(json!({
                "open": !self.doc.texture_sets.is_empty(),
                "path": self.project_path,
                "name": self.project_name(),
                "dirty": self.session.is_dirty(&self.doc, self.history.can_undo()),
            })),
            "project.open" => {
                let path = PathBuf::from(str_arg(a, "path")?);
                self.open_project(&path);
                if self.project_path.as_deref() != Some(path.as_path()) {
                    return Err(self.last_error("could not open the project"));
                }
                Ok(Value::Null)
            }
            "project.save" => {
                let path = match opt_str(a, "path")? {
                    Some(p) => PathBuf::from(p),
                    None => self
                        .project_path
                        .clone()
                        .ok_or("the project was never saved; give a path")?,
                };
                if !self.save_to(&path) {
                    return Err(self.last_error("could not save"));
                }
                Ok(json!({ "path": path }))
            }
            "project.create" => {
                let mesh = PathBuf::from(str_arg(a, "mesh")?);
                let template = match opt_str(a, "template")? {
                    None => crate::templates::Template::builtin().remove(0),
                    Some(t) if Path::new(t).is_file() => {
                        crate::templates::Template::load(Path::new(t))
                            .map_err(|e| format!("{e:#}"))?
                    }
                    Some(t) => crate::templates::Template::builtin()
                        .into_iter()
                        .chain(
                            crate::templates::Template::user()
                                .into_iter()
                                .map(|(_, t)| t),
                        )
                        .find(|x| x.name == t)
                        .ok_or_else(|| format!("no template named '{t}'"))?,
                };
                self.create_project(&mesh, &template);
                if self.doc.mesh_source.as_deref() != Some(&mesh.display().to_string()) {
                    return Err(self.last_error("could not load the mesh"));
                }
                Ok(Value::Null)
            }
            "project.close" => {
                self.set_mesh(
                    reetched_io::Mesh::uv_sphere(1.0, 64, 32),
                    "Preview sphere".into(),
                    None,
                );
                self.project_path = None;
                self.notify_script("project_closed", json!({}));
                Ok(Value::Null)
            }
            "history.undo" => Ok(json!({ "done": self.history.undo(&mut self.doc) })),
            "history.redo" => Ok(json!({ "done": self.history.redo(&mut self.doc) })),
            "textureset.list" => {
                let sets: Vec<Value> = self
                    .doc
                    .texture_sets
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        json!({
                            "index": i,
                            "name": s.name,
                            "resolution": [s.resolution, s.resolution],
                            "shader": shader_name(&s.shader),
                            "channels": s.channels().iter().map(|c| c.id()).collect::<Vec<_>>(),
                            "udims": s.tiles(),
                        })
                    })
                    .collect();
                let active = (!sets.is_empty()).then_some(self.selected_part);
                Ok(json!({ "active": active, "sets": sets }))
            }
            "textureset.update" => {
                let i = self.set_index(arg(a, "texture_set"))?;
                let set = &mut self.doc.texture_sets[i];
                if let Some(r) = arg(a, "resolution") {
                    let wh: Vec<u64> = r
                        .as_array()
                        .map(|v| v.iter().filter_map(Value::as_u64).collect())
                        .unwrap_or_default();
                    match wh[..] {
                        [w, h] if w == h && RESOLUTIONS.contains(&(w as u32)) => {
                            set.resolution = w as u32;
                        }
                        _ => {
                            return Err(format!(
                                "resolution must be square and one of {RESOLUTIONS:?}"
                            ));
                        }
                    }
                }
                if let Some(s) = opt_str(a, "shader")? {
                    set.shader = match s {
                        "metal_rough" => ShaderModel::MetalRough,
                        "asm" => ShaderModel::Asm,
                        "openpbr" => ShaderModel::OpenPbr,
                        s => return Err(format!("unknown shader '{s}'")),
                    };
                }
                for c in channels_arg(a, "add_channels")?.unwrap_or_default() {
                    if !set.has_channel(c) {
                        set.extra_channels.push(c);
                    }
                }
                for c in channels_arg(a, "remove_channels")?.unwrap_or_default() {
                    if c.is_core() {
                        return Err(format!("'{}' is a core channel", c.id()));
                    }
                    set.extra_channels.retain(|x| *x != c);
                }
                Ok(Value::Null)
            }
            "textureset.set_active" => {
                let i = self.set_index(arg(a, "texture_set"))?;
                if i != self.selected_part {
                    self.selected_part = i;
                    self.select_top_layer();
                }
                Ok(Value::Null)
            }
            "layers.tree" => {
                let set = &self.doc.texture_sets[self.set_index(arg(a, "texture_set"))?];
                let channels = set.channels();
                let nodes: Vec<Value> = set.stack.iter().map(|n| node_json(n, &channels)).collect();
                Ok(json!({ "nodes": nodes }))
            }
            "layers.insert" => {
                let i = self.set_index(arg(a, "texture_set"))?;
                let channels = self.doc.texture_sets[i].channels();
                let node = match str_arg(a, "kind")? {
                    "fill" => {
                        let mut fill = Fill::new([0.8; 3], 0.0, 0.5);
                        for (c, s) in fill.channels.iter_mut() {
                            s.enabled = channels.contains(c);
                        }
                        self.doc.new_fill("Fill layer".into(), fill)
                    }
                    "paint" => {
                        let n = self.doc.texture_sets[i].stack.len() + 1;
                        self.doc.new_paint(format!("Layer {n}"))
                    }
                    "group" => self.doc.new_folder("Folder 1".into()),
                    k => return Err(format!("unknown layer kind '{k}'")),
                };
                let id = node.id;
                place(&mut self.doc.texture_sets[i].stack, node, a)?;
                Ok(json!({ "id": id }))
            }
            "layers.update" => {
                let id = id_arg(a, "id")?;
                let set = self.locate(id)?;
                let resources: BTreeMap<u64, String> = self
                    .doc
                    .resources
                    .keys()
                    .map(|k| (resource_number(k), k.clone()))
                    .collect();
                let stack = &mut self.doc.texture_sets[set].stack;
                let node = find_node_mut(stack, id).ok_or("layer vanished")?;
                if let Some(v) = arg(a, "name") {
                    node.name = v.as_str().ok_or("'name' must be a string")?.to_owned();
                }
                if let Some(v) = arg(a, "visible") {
                    node.visible = v.as_bool().ok_or("'visible' must be true or false")?;
                }
                if let Some(m) = arg(a, "blending").and_then(Value::as_object) {
                    for (k, v) in m {
                        let c = channel(k)?;
                        let mut b = node.blend(c);
                        b.mode = blend_mode(v.as_str().unwrap_or_default())?;
                        node.blend.insert(c, b);
                    }
                }
                if let Some(m) = arg(a, "opacity").and_then(Value::as_object) {
                    for (k, v) in m {
                        let c = channel(k)?;
                        let mut b = node.blend(c);
                        b.opacity = v
                            .as_f64()
                            .ok_or("opacity must be a number")?
                            .clamp(0.0, 1.0) as f32;
                        node.blend.insert(c, b);
                    }
                }
                let fill_args = arg(a, "fill").is_some() || arg(a, "active_channels").is_some();
                if fill_args {
                    let NodeKind::Fill(f) = &mut node.kind else {
                        return Err("'fill' and 'active_channels' need a fill layer".into());
                    };
                    if let Some(m) = arg(a, "fill").and_then(Value::as_object) {
                        for (k, v) in m {
                            let c = channel(k)?;
                            let Some(s) = f.channels.get_mut(&c) else {
                                return Err(format!("this layer has no '{k}' channel"));
                            };
                            match v {
                                Value::Number(x) => {
                                    s.value = [x.as_f64().unwrap_or(0.0) as f32; 3];
                                    s.bitmap = None;
                                }
                                Value::Array(rgb) if rgb.len() == 3 => {
                                    for (d, x) in s.value.iter_mut().zip(rgb) {
                                        *d =
                                            x.as_f64().ok_or("colors are [r, g, b] floats")? as f32;
                                    }
                                    s.bitmap = None;
                                }
                                Value::Object(o) => {
                                    let n = o
                                        .get("resource")
                                        .and_then(Value::as_u64)
                                        .ok_or("a bitmap is {\"resource\": id}")?;
                                    let rid = resources
                                        .get(&n)
                                        .ok_or_else(|| format!("no resource {n}"))?;
                                    s.bitmap = Some(rid.clone());
                                }
                                _ => return Err(format!("bad value for '{k}'")),
                            }
                        }
                    }
                    if let Some(list) = channels_arg(a, "active_channels")? {
                        for (c, s) in f.channels.iter_mut() {
                            s.enabled = list.contains(c);
                        }
                    }
                }
                match opt_str(a, "mask")? {
                    None => {}
                    Some("white") => node.mask = Some(Mask::new(MaskBackground::White)),
                    Some("black") => node.mask = Some(Mask::new(MaskBackground::Black)),
                    Some("none") => node.mask = None,
                    Some(m) => return Err(format!("mask must be white, black or none, not '{m}'")),
                }
                Ok(Value::Null)
            }
            "layers.delete" => {
                let id = id_arg(a, "id")?;
                let set = self.locate(id)?;
                take_node(&mut self.doc.texture_sets[set].stack, id);
                if self.selected.is_some_and(|s| s.node == id) {
                    self.selected = None;
                }
                Ok(Value::Null)
            }
            "layers.move" => {
                let id = id_arg(a, "id")?;
                let set = self.locate(id)?;
                let stack = &mut self.doc.texture_sets[set].stack;
                let node = find_node(stack, id).ok_or("layer vanished")?;
                for k in ["above", "below", "parent"] {
                    if let Some(t) = opt_id(a, k)? {
                        if find_node(std::slice::from_ref(node), t).is_some() {
                            return Err("a layer can't move into itself".into());
                        }
                        if find_node(stack, t).is_none() {
                            return Err(format!("'{k}' is not a layer of this texture set"));
                        }
                    }
                }
                let node = take_node(stack, id).ok_or("layer vanished")?;
                place(stack, node, a)?;
                Ok(Value::Null)
            }
            "layers.selection" => {
                let set = self.doc.texture_sets.get(self.selected_part);
                Ok(json!({
                    "texture_set": set.map(|s| &s.name),
                    "ids": self.selected.map(|s| s.node).into_iter().collect::<Vec<_>>(),
                }))
            }
            "layers.select" => {
                let ids: Vec<NodeId> = arg(a, "ids")
                    .and_then(Value::as_array)
                    .ok_or("'ids' must be a list")?
                    .iter()
                    .map(|v| v.as_u64().ok_or("'ids' must be layer ids"))
                    .collect::<Result<_, _>>()?;
                // ponytail: the app selects one layer; the first id wins.
                let Some(&first) = ids.first() else {
                    self.selected = None;
                    return Ok(Value::Null);
                };
                let set = self.locate(first)?;
                for &id in &ids {
                    if self.locate(id)? != set {
                        return Err("selected layers must be in one texture set".into());
                    }
                }
                self.selected_part = set;
                self.selected = Some(Selected {
                    node: first,
                    mask: false,
                });
                Ok(Value::Null)
            }
            "resource.import" => {
                let path = PathBuf::from(str_arg(a, "path")?);
                self.need_project()?;
                let id = self
                    .import_resource(&path)
                    .ok_or_else(|| self.last_error("could not import"))?;
                Ok(json!({
                    "id": resource_number(&id),
                    "name": crate::app::file_name(&path),
                    "kind": "texture",
                }))
            }
            "resource.list" => {
                let list: Vec<Value> = self
                    .doc
                    .resources
                    .iter()
                    .map(|(id, info)| {
                        json!({ "id": resource_number(id), "name": info.name, "kind": "texture" })
                    })
                    .collect();
                Ok(json!({ "resources": list }))
            }
            "export.presets" => {
                let names: Vec<&str> = self
                    .export
                    .presets
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                Ok(json!({ "presets": names }))
            }
            "export.textures" => self.script_export(a),
            "export.mesh" => {
                use crate::stack_ui::MeshFormat;
                let path = PathBuf::from(str_arg(a, "path")?);
                self.need_project()?;
                let ext = match opt_str(a, "format")? {
                    Some(f) => f.to_ascii_lowercase(),
                    None => path
                        .extension()
                        .map(|e| e.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default(),
                };
                let format = MeshFormat::ALL
                    .into_iter()
                    .find(|f| f.extension() == ext)
                    .ok_or_else(|| {
                        format!("can't export meshes as '{ext}' (usda, usdc, usdz, obj)")
                    })?;
                self.export_mesh_to(&path, format)
                    .map_err(|e| format!("{e:#}"))?;
                Ok(Value::Null)
            }
            "bake" => self.script_bake(a),
            "display.get" => Ok(json!({
                "exposure": self.exposure_ev,
                "environment": self.env_path,
                "tone_mapping": format!("{:?}", self.render.tone_map).to_ascii_lowercase(),
            })),
            "display.set" => {
                if let Some(v) = arg(a, "exposure") {
                    self.exposure_ev = v.as_f64().ok_or("'exposure' must be a number")? as f32;
                }
                if let Some(t) = opt_str(a, "tone_mapping")? {
                    self.render.tone_map = crate::viewport::ToneMap::ALL
                        .into_iter()
                        .find(|m| format!("{m:?}").eq_ignore_ascii_case(t))
                        .ok_or_else(|| format!("unknown tone mapping '{t}'"))?;
                }
                if let Some(p) = opt_str(a, "environment")? {
                    let path = PathBuf::from(p);
                    self.load_environment(&path);
                    if self.env_path.as_deref() != Some(path.as_path()) {
                        return Err(self.last_error("could not load the environment"));
                    }
                }
                Ok(Value::Null)
            }
            m => Err(format!("unknown method '{m}'")),
        }
    }

    fn script_export(&mut self, a: &Value) -> Reply {
        use reetched_doc::export::{BitDepth, FileFormat, Padding};
        let dir = PathBuf::from(str_arg(a, "path")?);
        self.need_project()?;
        let preset = match opt_str(a, "preset")? {
            Some(name) => self.export.presets.iter().find(|p| p.name == name),
            None => self.export.presets.get(self.export.preset),
        }
        .cloned()
        .ok_or("no such export preset")?;
        let include = self.sets_arg(a, "texture_sets")?;
        let mut settings = self.export.settings.clone();
        if let Some(f) = opt_str(a, "format")? {
            settings.format = match f.to_ascii_lowercase().as_str() {
                "png" => FileFormat::Png,
                "tga" => FileFormat::Tga,
                "tif" | "tiff" => FileFormat::Tiff,
                "exr" => FileFormat::Exr,
                f => return Err(format!("can't export '{f}' files (png, tga, tif, exr)")),
            };
        }
        if let Some(d) = arg(a, "bit_depth") {
            settings.depth = match d.as_u64() {
                Some(8) => BitDepth::Eight,
                Some(16) => BitDepth::Sixteen,
                Some(32) => BitDepth::Float32,
                _ => return Err("bit_depth must be 8, 16 or 32".into()),
            };
        }
        let dilation = arg(a, "dilation").and_then(Value::as_u64).map(|d| d as u32);
        if let Some(p) = opt_str(a, "padding")? {
            settings.padding = match p {
                "infinite" => Padding::Infinite,
                "dilation" => Padding::Dilation {
                    pixels: dilation.unwrap_or(16),
                    transparent: false,
                },
                "transparent" => Padding::Dilation {
                    pixels: dilation.unwrap_or(0),
                    transparent: true,
                },
                "color" => Padding::None,
                p => return Err(format!("unknown padding '{p}'")),
            };
        }
        self.notify_script("export_started", json!({ "path": dir }));
        let result = self.export_textures(&dir, &preset, &settings, &include);
        let (ok, message, files) = match &result {
            Ok(files) => (true, format!("Exported to {}", dir.display()), json!(files)),
            Err(e) => (false, format!("{e:#}"), json!({})),
        };
        self.notify_script(
            "export_finished",
            json!({ "ok": ok, "message": message, "files": files }),
        );
        if ok {
            Ok(json!({ "files": files }))
        } else {
            Err(message)
        }
    }

    fn script_bake(&mut self, a: &Value) -> Reply {
        use reetched_bake::MapKind;
        self.need_project()?;
        if self.bake.job.is_some() {
            return Err("a bake is already running".into());
        }
        let include = self.sets_arg(a, "texture_sets")?;
        let resolution = arg(a, "resolution")
            .and_then(Value::as_u64)
            .map(|r| r as u32)
            .filter(|r| RESOLUTIONS.contains(r))
            .ok_or_else(|| format!("resolution must be one of {RESOLUTIONS:?}"))?;
        let maps = arg(a, "maps")
            .and_then(Value::as_array)
            .ok_or("'maps' must be a list")?
            .iter()
            .map(|m| {
                let m = m.as_str().unwrap_or_default();
                Ok(match m {
                    "normal" => MapKind::Normal,
                    "world_normal" => MapKind::WorldNormal,
                    "id" => MapKind::Id,
                    "ambient_occlusion" => MapKind::AmbientOcclusion,
                    "curvature" => MapKind::Curvature,
                    "position" => MapKind::Position,
                    "thickness" => MapKind::Thickness,
                    "height" => MapKind::Height,
                    "opacity" => MapKind::Opacity,
                    m => return Err(format!("unknown map '{m}'")),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.bake.include = include.clone();
        self.bake.settings.resolution = resolution;
        self.bake.settings.maps = maps;
        self.bake.high_poly = opt_str(a, "high_poly")?.unwrap_or_default().to_owned();
        self.start_bake();
        let ok = self.poll_bake(true) == Some(true);
        let names: Vec<String> = self
            .doc
            .texture_sets
            .iter()
            .zip(&include)
            .filter(|(_, on)| **on)
            .map(|(s, _)| s.name.clone())
            .collect();
        self.notify_script(
            "baking_finished",
            json!({ "ok": ok, "texture_sets": names }),
        );
        if ok {
            Ok(json!({ "baked": names }))
        } else {
            Err(self.last_error("the bake failed"))
        }
    }
}

impl ScriptHost for State {
    fn call(&mut self, method: &str, args: Value) -> Result<Value, String> {
        // A failed call leaves the document as it was; a good one is its own undo step.
        let before = self.doc.clone();
        let r = self.host_call(method, &args);
        if r.is_err() {
            self.doc = before;
        }
        self.history.observe(&self.doc, false);
        r
    }

    fn log(&mut self, level: LogLevel, text: &str) {
        self.console_print(text);
        let text = text.trim_end();
        if text.is_empty() {
            return;
        }
        match level {
            LogLevel::Info => self.info(text.to_owned()),
            LogLevel::Warning => self.warn(text.to_owned()),
            LogLevel::Error => self.error(text.to_owned()),
        }
    }
}
