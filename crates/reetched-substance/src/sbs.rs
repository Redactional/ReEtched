//! `.sbs` reader: package XML → graphs, function graphs, bitmap resources and
//! dependencies (other `.sbs` files, resolved through a caller-supplied loader).

use anyhow::{Context, Result, bail};
use reetched_graph::{
    Graph, Image, InputDef, InputKind, Link, Node, NodeKind, NodeParam, OutputDef, Package, Param,
    Relative, Ty, V, Widget,
    func::{FnData, FnGraph, FnNode},
};
use roxmltree::Node as X;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Loads a file the package refers to: a path relative to the root `.sbs`
/// (`textures/a.png`, `../lib/noise.sbs`) or a library package (`sbs://noise.sbs`).
pub type Files<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

const MAX_DEPTH: usize = 16;

/// Reads a `.sbs` package. `name` is its path relative to the loader's root
/// (normally just the file name).
pub fn read(bytes: &[u8], name: &str, files: Files) -> Result<Package> {
    let mut cache = HashMap::new();
    read_at(bytes, name, files, 0, &mut cache)
}

type Cache = HashMap<String, Result<Arc<Package>, String>>;

fn read_at(
    bytes: &[u8],
    name: &str,
    files: Files,
    depth: usize,
    cache: &mut Cache,
) -> Result<Package> {
    let text = std::str::from_utf8(bytes).context("not a text file")?;
    let doc = roxmltree::Document::parse(text).context("not valid XML")?;
    let root = doc.root_element();
    if root.tag_name().name() != "package" {
        bail!(
            "not a Substance package (root element is <{}>)",
            root.tag_name().name()
        );
    }
    let mut pkg = Package {
        name: name.rsplit('/').next().unwrap_or(name).to_string(),
        ..Default::default()
    };

    // Dependencies.
    let mut himself = HashSet::new();
    for dep in kids(child(root, "dependencies"), "dependency") {
        let (Some(file), Some(uid)) = (v(dep, "filename"), v(dep, "uid")) else {
            continue;
        };
        if file == "?himself" {
            himself.insert(uid.to_string());
            continue;
        }
        let path = join(name, file);
        let loaded = match cache.get(&path) {
            Some(r) => r.clone(),
            None => {
                let r = if depth >= MAX_DEPTH {
                    Err(format!("{file}: dependencies nest too deeply"))
                } else {
                    match files(&path) {
                        Some(b) => read_at(&b, &path, files, depth + 1, cache)
                            .map(Arc::new)
                            .map_err(|e| format!("{file}: {e:#}")),
                        None if file.starts_with("sbs://") => Err(format!(
                            "{file} is a Substance Designer library package; it is used when Substance Designer is installed"
                        )),
                        None => Err(format!("{file} was not found next to {name}")),
                    }
                };
                cache.insert(path, r.clone());
                r
            }
        };
        pkg.deps.insert(uid.to_string(), loaded);
    }

    if let Some(content) = child(root, "content") {
        content_items(content, "", name, files, &mut pkg);
    }

    // References to this same file ("?himself") become plain local paths.
    if !himself.is_empty() {
        let fix = |p: &mut String| {
            let (base, dep) = reetched_graph::split_path(p);
            if dep.is_some_and(|d| himself.contains(d)) {
                *p = format!("pkg:///{base}");
            }
        };
        for g in &mut pkg.graphs {
            for n in g.nodes.values_mut() {
                if let NodeKind::Instance { path, .. } = &mut n.kind {
                    fix(path);
                }
                for p in n.params.values_mut() {
                    if let Param::Expr(f) = &mut p.value {
                        fix_fn(Arc::make_mut(f), &fix);
                    }
                }
            }
        }
        for f in pkg.functions.values_mut() {
            fix_fn(Arc::make_mut(f), &fix);
        }
    }
    Ok(pkg)
}

fn fix_fn(g: &mut FnGraph, fix: &dyn Fn(&mut String)) {
    for n in g.nodes.values_mut() {
        if n.func == "instance" {
            for (_, d) in &mut n.data {
                if let FnData::Str(s) = d {
                    fix(s);
                }
            }
        }
    }
}

/// Resolves `file` (as written in a package at `from`) to a loader path.
fn join(from: &str, file: &str) -> String {
    if file.starts_with("sbs://") {
        return file.to_string();
    }
    let file = file.replace('\\', "/");
    let mut parts: Vec<&str> = match from.rfind('/') {
        Some(i) => from[..i].split('/').collect(),
        None => Vec::new(),
    };
    for p in file.split('/') {
        match p {
            "" | "." => {}
            ".." if parts.last().is_some_and(|l| *l != "..") => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn content_items(content: X, prefix: &str, name: &str, files: Files, pkg: &mut Package) {
    for item in content.children().filter(X::is_element) {
        let Some(id) = v(item, "identifier") else {
            continue;
        };
        let path = format!("{prefix}{id}");
        match item.tag_name().name() {
            "group" => {
                if let Some(c) = child(item, "content") {
                    content_items(c, &format!("{path}/"), name, files, pkg);
                }
            }
            "graph" => pkg.graphs.push(graph(item, path)),
            "function" => {
                if let Some(d) = child(item, "paramValue").and_then(|p| child(p, "dynamicValue")) {
                    let mut f = fn_graph(d);
                    f.inputs = kids(child(item, "paraminputs"), "paraminput")
                        .filter_map(|p| {
                            let id = v(p, "identifier")?.to_string();
                            let ty =
                                type_code(v(p, "type").and_then(|t| t.parse().ok()).unwrap_or(256));
                            let default = child(p, "defaultValue")
                                .and_then(constant)
                                .and_then(Const::value)
                                .unwrap_or(zero(ty.unwrap_or(Ty::Float(1))));
                            Some((id, default))
                        })
                        .collect();
                    pkg.functions.insert(path, Arc::new(f));
                }
            }
            "resource" => {
                let Some(file) = v(item, "filepath") else {
                    continue;
                };
                if let Some(img) = files(&join(name, file)).and_then(|b| decode(&b)) {
                    pkg.resources.insert(path, Arc::new(img));
                }
            }
            _ => {}
        }
    }
}

/// Decodes an image file into a graph image (8/16-bit values kept as stored).
pub fn decode(bytes: &[u8]) -> Option<Image> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if img.color().has_color() {
        let px = img.to_rgba32f();
        Some(Image::color(w, h, |x, y| {
            px.get_pixel(x as u32, y as u32).0
        }))
    } else {
        let px = img.to_luma32f();
        Some(Image::gray(w, h, |x, y| {
            px.get_pixel(x as u32, y as u32).0[0]
        }))
    }
}

fn graph(g: X, id: String) -> Graph {
    let label = v(g, "label")
        .or_else(|| child(g, "attributes").and_then(|a| v(a, "label")))
        .unwrap_or(&id)
        .to_string();
    let inputs = kids(child(g, "paraminputs"), "paraminput")
        .filter_map(input_def)
        .collect();
    let outputs = kids(child(g, "graphOutputs"), "graphoutput")
        .filter_map(|o| {
            let id = v(o, "identifier")?.to_string();
            Some(OutputDef {
                uid: uid(o)?,
                label: v(o, "label")
                    .or_else(|| child(o, "attributes").and_then(|a| v(a, "label")))
                    .unwrap_or(&id)
                    .to_string(),
                usage: usage(o),
                id,
            })
        })
        .collect();
    let nodes = kids(child(g, "compNodes"), "compNode")
        .filter_map(comp_node)
        .map(|n| (n.uid, n))
        .collect();
    let size = kids(child(g, "baseParameters"), "parameter")
        .find(|p| v(*p, "name") == Some("outputsize"))
        .and_then(|p| {
            let c = child(p, "paramValue").and_then(constant)?.value()?;
            Some(([c.x[0] as i32, c.x[1] as i32], relative(p)))
        });
    Graph {
        id,
        label,
        inputs,
        outputs,
        nodes,
        size,
    }
}

fn usage(n: X) -> Option<String> {
    kids(child(n, "usages"), "usage")
        .find_map(|u| v(u, "name"))
        .map(str::to_string)
}

fn type_code(t: u32) -> Option<Ty> {
    Some(match t {
        4 => Ty::Bool,
        16 => Ty::Int(1),
        32 => Ty::Int(2),
        64 => Ty::Int(3),
        128 => Ty::Int(4),
        256 => Ty::Float(1),
        512 => Ty::Float(2),
        1024 => Ty::Float(3),
        2048 => Ty::Float(4),
        _ => return None,
    })
}

fn zero(ty: Ty) -> V {
    V { ty, x: [0.0; 4] }
}

fn input_def(p: X) -> Option<InputDef> {
    let id = v(p, "identifier")?.to_string();
    let t: u32 = v(p, "type")?.parse().ok()?;
    let kind = match t {
        1 | 3 => InputKind::ColorImage,
        2 => InputKind::GrayImage,
        _ => InputKind::Value,
    };
    let ty = type_code(t).unwrap_or(Ty::Float(1));
    let default = child(p, "defaultValue")
        .and_then(constant)
        .and_then(Const::value)
        .unwrap_or(zero(ty));
    let mut def = InputDef::value(&id, &id, default, 0.0, 1.0);
    def.uid = uid(p).unwrap_or(0);
    def.kind = kind;
    def.label = v(p, "label")
        .or_else(|| child(p, "attributes").and_then(|a| v(a, "label")))
        .unwrap_or(&id)
        .to_string();
    def.group = v(p, "group").unwrap_or_default().to_string();
    def.usage = usage(p);
    if let Some(w) = child(p, "defaultWidget") {
        let opts: HashMap<&str, &str> = kids(child(w, "options"), "option")
            .filter_map(|o| Some((v(o, "name")?, v(o, "value")?)))
            .collect();
        let num = |k: &str| {
            opts.get(k)
                .and_then(|s| s.split([' ', ';']).next())
                .and_then(|s| s.parse::<f32>().ok())
        };
        def.min = num("min").unwrap_or(0.0);
        def.max = num("max").unwrap_or(match ty {
            Ty::Int(_) => 16.0,
            _ => 1.0,
        });
        def.clamp = opts.get("clamp").is_some_and(|c| *c == "1" || *c == "true");
        def.widget = match v(w, "name").unwrap_or_default() {
            "color" => Widget::Color,
            "togglebutton" => Widget::Toggle,
            "angle" => Widget::Angle,
            "combobox" => match opts.get("parameters").map(|s| combo(s)) {
                Some(items) if !items.is_empty() => Widget::Combo(items),
                _ => Widget::Slider,
            },
            _ => def.widget.clone(),
        };
    }
    Some(def)
}

/// Combo box items: `count;value;label;value;label…`.
fn combo(s: &str) -> Vec<(i32, String)> {
    let parts: Vec<&str> = s.split(';').collect();
    parts
        .get(1..)
        .unwrap_or_default()
        .chunks(2)
        .filter_map(|c| Some((c.first()?.trim().parse().ok()?, c.get(1)?.to_string())))
        .collect()
}

fn relative(p: X) -> Relative {
    match v(p, "relativeTo").and_then(|r| r.parse::<u8>().ok()) {
        Some(0) => Relative::Absolute,
        Some(2) => Relative::ToParent,
        _ => Relative::ToInput,
    }
}

fn params(parent: Option<X>) -> HashMap<String, NodeParam> {
    kids(parent, "parameter")
        .filter_map(|p| {
            let name = v(p, "name")?.to_string();
            let pv = child(p, "paramValue")?;
            let value = if let Some(d) = child(pv, "dynamicValue") {
                Param::Expr(Arc::new(fn_graph(d)))
            } else {
                match constant(pv)? {
                    Const::V(v) => Param::Const(v),
                    Const::Str(s) => Param::Str(s),
                }
            };
            Some((
                name,
                NodeParam {
                    value,
                    relative: relative(p),
                },
            ))
        })
        .collect()
}

fn comp_node(n: X) -> Option<Node> {
    let node_uid = uid(n)?;
    let imp = child(n, "compImplementation")?
        .children()
        .find(X::is_element)?;
    let links = kids(child(n, "connections"), "connection")
        .filter_map(|c| {
            Some((
                v(c, "identifier")?.to_string(),
                Link {
                    node: v(c, "connRef")?.parse().ok()?,
                    output: v(c, "connRefOutput").and_then(|o| o.parse().ok()),
                },
            ))
        })
        .collect();
    let outputs = kids(child(n, "compOutputs"), "compOutput")
        .filter_map(uid)
        .collect();
    let mut node_params = HashMap::new();
    let kind = match imp.tag_name().name() {
        "compFilter" => {
            node_params = params(child(imp, "parameters"));
            for arr in kids(child(imp, "paramsArrays"), "paramsArray") {
                let Some(name) = v(arr, "name") else { continue };
                let cells = kids(child(arr, "paramsArrayCells"), "paramsArrayCell")
                    .map(|cell| {
                        params(child(cell, "parameters"))
                            .into_iter()
                            .filter_map(|(k, p)| match p.value {
                                Param::Const(v) => Some((k, v)),
                                _ => None,
                            })
                            .collect()
                    })
                    .collect();
                node_params.insert(
                    name.to_string(),
                    NodeParam {
                        value: Param::Array(cells),
                        relative: Relative::ToInput,
                    },
                );
            }
            NodeKind::Atomic(v(imp, "filter")?.to_string())
        }
        "compInstance" => {
            node_params = params(child(imp, "parameters"));
            NodeKind::Instance {
                path: v(imp, "path")?.to_string(),
                outputs: kids(child(imp, "outputBridgings"), "outputBridging")
                    .filter_map(|b| Some((uid(b)?, v(b, "identifier")?.to_string())))
                    .collect(),
            }
        }
        "compInputBridge" => NodeKind::Input(v(imp, "entry")?.parse().ok()?),
        "compOutputBridge" => NodeKind::Output(v(imp, "output")?.parse().ok()?),
        _ => return None,
    };
    Some(Node {
        uid: node_uid,
        kind,
        links,
        outputs,
        params: node_params,
    })
}

fn fn_graph(d: X) -> FnGraph {
    let nodes = kids(child(d, "paramNodes"), "paramNode")
        .filter_map(|n| {
            let uid = uid(n)?;
            let func = v(n, "function")?.to_string();
            let data = kids(child(n, "funcDatas"), "funcData")
                .filter_map(|fd| {
                    let name = v(fd, "name").unwrap_or_default().to_string();
                    let c = child(fd, "constantValue").and_then(constant)?;
                    Some((
                        name,
                        match c {
                            Const::V(v) => FnData::V(v),
                            Const::Str(s) => FnData::Str(s),
                        },
                    ))
                })
                .collect();
            let links = kids(child(n, "connections"), "connection")
                .filter_map(|c| {
                    Some((
                        v(c, "identifier")?.to_string(),
                        v(c, "connRef")?.parse().ok()?,
                    ))
                })
                .collect();
            Some((uid, FnNode { func, data, links }))
        })
        .collect();
    FnGraph {
        nodes,
        root: v(d, "rootnode").and_then(|r| r.parse().ok()).unwrap_or(0),
        inputs: Vec::new(),
    }
}

enum Const {
    V(V),
    Str(String),
}

impl Const {
    fn value(self) -> Option<V> {
        match self {
            Const::V(v) => Some(v),
            Const::Str(_) => None,
        }
    }
}

/// The `constantValue*` element inside `n`.
fn constant(n: X) -> Option<Const> {
    let c = n
        .children()
        .find(|c| c.is_element() && c.tag_name().name().starts_with("constantValue"))?;
    let tag = c.tag_name().name().trim_start_matches("constantValue");
    let raw = c.attribute("v").unwrap_or_default();
    let nums = || -> [f32; 4] {
        let mut x = [0.0; 4];
        for (o, s) in x.iter_mut().zip(raw.split_whitespace()) {
            *o = s.parse().unwrap_or(0.0);
        }
        x
    };
    let ty = match tag {
        "String" => return Some(Const::Str(raw.to_string())),
        "Bool" => Ty::Bool,
        "Int32" | "Int1" => Ty::Int(1),
        "Int2" => Ty::Int(2),
        "Int3" => Ty::Int(3),
        "Int4" => Ty::Int(4),
        "Float1" | "Float" => Ty::Float(1),
        "Float2" => Ty::Float(2),
        "Float3" => Ty::Float(3),
        "Float4" => Ty::Float(4),
        _ => return None,
    };
    let mut x = nums();
    if ty == Ty::Bool {
        x[0] = if raw == "1" || raw == "true" {
            1.0
        } else {
            0.0
        };
    }
    Some(Const::V(V { ty, x }))
}

fn child<'a, 'i>(n: X<'a, 'i>, tag: &str) -> Option<X<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
}

fn kids<'a, 'i: 'a>(n: Option<X<'a, 'i>>, tag: &'a str) -> impl Iterator<Item = X<'a, 'i>> + 'a {
    n.into_iter()
        .flat_map(|n| n.children())
        .filter(move |c| c.is_element() && c.tag_name().name() == tag)
}

/// The `v` attribute of child element `tag`.
fn v<'a>(n: X<'a, '_>, tag: &str) -> Option<&'a str> {
    child(n, tag).and_then(|c| c.attribute("v"))
}

fn uid(n: X) -> Option<u64> {
    v(n, "uid")?.parse().ok()
}

#[cfg(test)]
mod tests;
