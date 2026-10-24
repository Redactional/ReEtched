//! In-memory stand-in for the app: implements the PROTOCOL.md host methods on
//! a fake document, enough to exercise the Python API without a GPU.

use reetched_script::{LogLevel, ScriptHost};
use serde_json::{Value, json};

const CHANNELS: &[&str] = &[
    "basecolor",
    "metallic",
    "roughness",
    "normal",
    "height",
    "emissive",
    "opacity",
    "specularlevel",
    "specularedgecolor",
    "anisotropylevel",
    "anisotropyangle",
    "transmissive",
    "scattering",
    "scatteringcolor",
    "coatopacity",
    "coatcolor",
    "coatroughness",
    "sheenopacity",
    "sheencolor",
    "sheenroughness",
    "ambientocclusion",
];
const SHADERS: &[&str] = &["metal_rough", "asm", "openpbr"];
const MUTATING: &[&str] = &[
    "textureset.update",
    "layers.insert",
    "layers.update",
    "layers.delete",
    "layers.move",
    "resource.import",
];

#[derive(Clone)]
pub struct Doc {
    pub open: bool,
    pub path: Option<String>,
    pub dirty: bool,
    /// Texture sets in protocol shape, plus their layer "nodes".
    pub sets: Vec<Value>,
    pub active: usize,
    pub selection: Vec<u64>,
    pub resources: Vec<Value>,
    pub display: Value,
    pub next_id: u64,
}

pub struct MockHost {
    pub doc: Doc,
    pub undo: Vec<Doc>,
    pub redo: Vec<Doc>,
    pub logs: Vec<(LogLevel, String)>,
    /// Every call as (method, args), for asserting on exact arguments.
    pub calls: Vec<(String, Value)>,
}

fn texture_set(name: &str) -> Value {
    json!({
        "name": name, "resolution": [2048, 2048], "shader": "metal_rough",
        "channels": ["basecolor", "metallic", "roughness", "normal", "height"],
        "udims": [1001], "nodes": [],
    })
}

impl MockHost {
    pub fn new() -> Self {
        MockHost {
            doc: Doc {
                open: true,
                path: Some("C:/projects/robot.ret".into()),
                dirty: false,
                sets: vec![texture_set("Body"), texture_set("Visor")],
                active: 0,
                selection: vec![],
                resources: vec![],
                display: json!({ "exposure": 0.0, "environment": "studio.hdr", "tone_mapping": "aces" }),
                next_id: 1,
            },
            undo: vec![],
            redo: vec![],
            logs: vec![],
            calls: vec![],
        }
    }

    pub fn log_text(&self) -> String {
        self.logs.iter().map(|(_, t)| format!("{t}\n")).collect()
    }

    pub fn nodes(&self, set: usize) -> &Vec<Value> {
        self.doc.sets[set]["nodes"].as_array().unwrap()
    }

    pub fn node(&mut self, id: u64) -> Option<Value> {
        self.doc
            .sets
            .iter_mut()
            .find_map(|s| find_mut(nodes_mut(s), id).cloned())
    }

    fn set_index(&self, r: &Value) -> Result<usize, String> {
        match r {
            Value::Number(n) => n
                .as_u64()
                .map(|i| i as usize)
                .filter(|&i| i < self.doc.sets.len()),
            Value::String(s) => self.doc.sets.iter().position(|t| t["name"] == *s),
            _ => None,
        }
        .ok_or_else(|| format!("no texture set {r}"))
    }

    fn need_project(&self) -> Result<(), String> {
        if self.doc.open {
            Ok(())
        } else {
            Err("no project is open".into())
        }
    }

    /// Index of the texture set holding node `id`.
    fn set_of(&mut self, id: u64) -> Result<usize, String> {
        (0..self.doc.sets.len())
            .find(|&i| find_mut(nodes_mut(&mut self.doc.sets[i]), id).is_some())
            .ok_or_else(|| format!("no layer {id}"))
    }

    fn handle(&mut self, method: &str, a: &Value) -> Result<Value, String> {
        let doc = &mut self.doc;
        match method {
            "project.info" => Ok(json!({
                "open": doc.open,
                "path": if doc.open { json!(doc.path) } else { Value::Null },
                "name": if doc.open { json!("robot") } else { Value::Null },
                "dirty": doc.dirty,
            })),
            "project.open" => {
                doc.open = true;
                doc.path = a["path"].as_str().map(String::from);
                Ok(Value::Null)
            }
            "project.save" => {
                self.need_project()?;
                let doc = &mut self.doc;
                if let Some(p) = a["path"].as_str() {
                    doc.path = Some(p.into());
                }
                doc.dirty = false;
                Ok(
                    json!({ "path": doc.path.clone().ok_or("the project has no file yet; pass a path")? }),
                )
            }
            "project.create" => {
                self.doc = MockHost::new().doc;
                self.doc.path = None;
                Ok(Value::Null)
            }
            "project.close" => {
                doc.open = false;
                Ok(Value::Null)
            }
            "history.undo" | "history.redo" => {
                let (from, to) = if method == "history.undo" {
                    (&mut self.undo, &mut self.redo)
                } else {
                    (&mut self.redo, &mut self.undo)
                };
                let Some(prev) = from.pop() else {
                    return Ok(json!({ "done": false }));
                };
                to.push(std::mem::replace(&mut self.doc, prev));
                Ok(json!({ "done": true }))
            }
            "textureset.list" => {
                self.need_project()?;
                let sets: Vec<Value> = self
                    .doc
                    .sets
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let mut s = s.clone();
                        s.as_object_mut().unwrap().remove("nodes");
                        s["index"] = json!(i);
                        s
                    })
                    .collect();
                Ok(json!({ "active": self.doc.active, "sets": sets }))
            }
            "textureset.update" => {
                let i = self.set_index(&a["texture_set"])?;
                let s = &mut self.doc.sets[i];
                if let Some(r) = a.get("resolution").filter(|v| !v.is_null()) {
                    s["resolution"] = r.clone();
                }
                if let Some(sh) = a.get("shader").and_then(Value::as_str) {
                    if !SHADERS.contains(&sh) {
                        return Err(format!("unknown shader {sh}"));
                    }
                    s["shader"] = json!(sh);
                }
                for c in a
                    .get("add_channels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if !CHANNELS.contains(&c.as_str().unwrap_or_default()) {
                        return Err(format!("unknown channel {c}"));
                    }
                    let list = s["channels"].as_array_mut().unwrap();
                    if !list.contains(c) {
                        list.push(c.clone());
                    }
                }
                for c in a
                    .get("remove_channels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    s["channels"].as_array_mut().unwrap().retain(|x| x != c);
                }
                Ok(Value::Null)
            }
            "textureset.set_active" => {
                self.doc.active = self.set_index(&a["texture_set"])?;
                Ok(Value::Null)
            }
            "layers.tree" => {
                let i = self.set_index(&a["texture_set"])?;
                Ok(json!({ "nodes": self.doc.sets[i]["nodes"] }))
            }
            "layers.insert" => {
                let i = self.set_index(&a["texture_set"])?;
                let kind = a["kind"].as_str().unwrap_or_default();
                let id = self.doc.next_id;
                self.doc.next_id += 1;
                let mut node = json!({
                    "id": id, "kind": kind, "name": format!("{kind} {id}"), "visible": true,
                    "blending": {}, "opacity": {}, "has_mask": false,
                });
                match kind {
                    "fill" => {
                        node["active_channels"] = self.doc.sets[i]["channels"].clone();
                        node["fill"] = json!({});
                    }
                    "group" => node["children"] = json!([]),
                    "paint" => {}
                    _ => return Err(format!("unknown layer kind {kind}")),
                }
                place(nodes_mut(&mut self.doc.sets[i]), node, a)?;
                Ok(json!({ "id": id }))
            }
            "layers.update" => {
                let id = a["id"].as_u64().ok_or("id required")?;
                let i = self.set_of(id)?;
                let n = find_mut(nodes_mut(&mut self.doc.sets[i]), id).unwrap();
                for key in ["name", "visible", "active_channels"] {
                    if let Some(v) = a.get(key) {
                        n[key] = v.clone();
                    }
                }
                for key in ["blending", "opacity", "fill"] {
                    if let Some(map) = a.get(key).and_then(Value::as_object) {
                        if key == "fill" && n["kind"] != "fill" {
                            return Err("only fill layers have channel values".into());
                        }
                        for (k, v) in map {
                            n[key][k] = v.clone();
                        }
                    }
                }
                match a.get("mask").and_then(Value::as_str) {
                    Some("white" | "black") => n["has_mask"] = json!(true),
                    Some("none") => n["has_mask"] = json!(false),
                    Some(m) => return Err(format!("bad mask {m}")),
                    None => {}
                }
                Ok(Value::Null)
            }
            "layers.delete" => {
                let id = a["id"].as_u64().ok_or("id required")?;
                let i = self.set_of(id)?;
                let (list, at) = list_containing(nodes_mut(&mut self.doc.sets[i]), id).unwrap();
                list.remove(at);
                self.doc.selection.retain(|&s| s != id);
                Ok(Value::Null)
            }
            "layers.move" => {
                let id = a["id"].as_u64().ok_or("id required")?;
                let i = self.set_of(id)?;
                let nodes = nodes_mut(&mut self.doc.sets[i]);
                let (list, at) = list_containing(nodes, id).unwrap();
                let node = list.remove(at);
                place(nodes, node, a)?;
                Ok(Value::Null)
            }
            "layers.selection" => {
                let name = self.doc.sets[self.doc.active]["name"].clone();
                Ok(json!({ "texture_set": name, "ids": self.doc.selection }))
            }
            "layers.select" => {
                let ids: Vec<u64> = a["ids"]
                    .as_array()
                    .ok_or("ids required")?
                    .iter()
                    .filter_map(Value::as_u64)
                    .collect();
                if let Some(&first) = ids.first() {
                    self.doc.active = self.set_of(first)?;
                }
                self.doc.selection = ids;
                Ok(Value::Null)
            }
            "resource.import" => {
                let path = a["path"].as_str().ok_or("path required")?;
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                let r = json!({ "id": 100 + self.doc.resources.len() as u64, "name": name, "kind": "texture" });
                self.doc.resources.push(r.clone());
                Ok(r)
            }
            "resource.list" => Ok(json!({ "resources": self.doc.resources })),
            "export.presets" => {
                Ok(json!({ "presets": ["PBR Metallic Roughness", "Unreal Engine"] }))
            }
            "export.textures" => {
                self.need_project()?;
                let dir = a["path"].as_str().ok_or("path required")?;
                let ext = a["format"].as_str().unwrap_or("png");
                let mut files = serde_json::Map::new();
                for s in &self.doc.sets {
                    let name = s["name"].as_str().unwrap();
                    let wanted = a["texture_sets"]
                        .as_array()
                        .is_none_or(|l| l.iter().any(|x| x == name));
                    if wanted {
                        let list: Vec<String> = s["channels"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|c| format!("{dir}/{name}_{}.{ext}", c.as_str().unwrap()))
                            .collect();
                        files.insert(name.into(), json!(list));
                    }
                }
                Ok(json!({ "files": files }))
            }
            "export.mesh" => Ok(Value::Null),
            "bake" => {
                let names: Vec<Value> = match a["texture_sets"].as_array() {
                    Some(l) => l.clone(),
                    None => self.doc.sets.iter().map(|s| s["name"].clone()).collect(),
                };
                Ok(json!({ "baked": names }))
            }
            "display.get" => Ok(self.doc.display.clone()),
            "display.set" => {
                for (k, v) in a.as_object().unwrap() {
                    if !v.is_null() {
                        self.doc.display[k] = v.clone();
                    }
                }
                Ok(Value::Null)
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

impl ScriptHost for MockHost {
    fn call(&mut self, method: &str, args: Value) -> Result<Value, String> {
        self.calls.push((method.to_string(), args.clone()));
        let before = self.doc.clone();
        let result = self.handle(method, &args);
        if result.is_ok() && MUTATING.contains(&method) {
            self.undo.push(before);
            self.redo.clear();
            self.doc.dirty = true;
        }
        result
    }

    fn log(&mut self, level: LogLevel, text: &str) {
        self.logs.push((level, text.to_string()));
    }
}

fn nodes_mut(set: &mut Value) -> &mut Vec<Value> {
    set["nodes"].as_array_mut().unwrap()
}

/// The list holding node `id`, and its index there.
fn list_containing(nodes: &mut Vec<Value>, id: u64) -> Option<(&mut Vec<Value>, usize)> {
    if let Some(i) = nodes.iter().position(|n| n["id"] == id) {
        return Some((nodes, i));
    }
    for n in nodes.iter_mut() {
        if let Some(children) = n.get_mut("children").and_then(Value::as_array_mut)
            && let Some(found) = list_containing(children, id)
        {
            return Some(found);
        }
    }
    None
}

fn find_mut(nodes: &mut Vec<Value>, id: u64) -> Option<&mut Value> {
    let (list, i) = list_containing(nodes, id)?;
    Some(&mut list[i])
}

/// Inserts per the protocol's position fields: above/below a sibling, else
/// at the top of `parent`'s children, else at the top of the root stack.
fn place(nodes: &mut Vec<Value>, node: Value, a: &Value) -> Result<(), String> {
    for (key, offset) in [("above", 0), ("below", 1)] {
        if let Some(sibling) = a.get(key).and_then(Value::as_u64) {
            let (list, i) = list_containing(nodes, sibling).ok_or(format!("no layer {sibling}"))?;
            list.insert(i + offset, node);
            return Ok(());
        }
    }
    match a.get("parent").and_then(Value::as_u64) {
        Some(parent) => {
            let group = find_mut(nodes, parent).ok_or(format!("no layer {parent}"))?;
            let children = group
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .ok_or("parent is not a group")?;
            children.insert(0, node);
        }
        None => nodes.insert(0, node),
    }
    Ok(())
}
