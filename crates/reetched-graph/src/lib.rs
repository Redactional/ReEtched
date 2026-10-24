//! Procedural texture graphs, modelled on Substance Designer's: graphs of
//! image nodes (the "atomic" nodes plus our own generators), parameters that
//! can be driven by function graphs, and instances of other graphs.
//!
//! Evaluation runs on the CPU with rayon.
//! ponytail: CPU only; move hot nodes to WGSL compute when graphs get slow.

mod atomic;
mod eval;
pub mod func;
mod image;
pub mod library;
mod own;

pub use eval::{Args, Rendered, render};
pub use func::FnGraph;
pub use image::{Image, Px};

use std::{collections::HashMap, sync::Arc};

/// Value type: bool, int or float with 1–4 components.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Bool,
    Int(u8),
    Float(u8),
}

impl Ty {
    pub fn len(self) -> usize {
        match self {
            Ty::Bool => 1,
            Ty::Int(n) | Ty::Float(n) => n as usize,
        }
    }

    pub fn is_empty(self) -> bool {
        false
    }
}

/// A function-graph / parameter value. Ints and bools are stored as floats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct V {
    pub ty: Ty,
    pub x: [f32; 4],
}

impl Default for V {
    fn default() -> Self {
        V::f(0.0)
    }
}

impl V {
    pub fn f(x: f32) -> V {
        V {
            ty: Ty::Float(1),
            x: [x, 0.0, 0.0, 0.0],
        }
    }
    pub fn f2(x: f32, y: f32) -> V {
        V {
            ty: Ty::Float(2),
            x: [x, y, 0.0, 0.0],
        }
    }
    pub fn f3(x: [f32; 3]) -> V {
        V {
            ty: Ty::Float(3),
            x: [x[0], x[1], x[2], 0.0],
        }
    }
    pub fn f4(x: [f32; 4]) -> V {
        V {
            ty: Ty::Float(4),
            x,
        }
    }
    pub fn i(x: i32) -> V {
        V {
            ty: Ty::Int(1),
            x: [x as f32, 0.0, 0.0, 0.0],
        }
    }
    pub fn i2(x: i32, y: i32) -> V {
        V {
            ty: Ty::Int(2),
            x: [x as f32, y as f32, 0.0, 0.0],
        }
    }
    pub fn b(x: bool) -> V {
        V {
            ty: Ty::Bool,
            x: [if x { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        }
    }
    /// Same type as `self`, new components.
    pub fn with(self, x: [f32; 4]) -> V {
        V { ty: self.ty, x }
    }
    pub fn float(self) -> f32 {
        self.x[0]
    }
    pub fn int(self) -> i32 {
        self.x[0] as i32
    }
    pub fn bool(self) -> bool {
        self.x[0] != 0.0
    }
    /// Components as RGBA; a scalar becomes grey with alpha 1.
    pub fn rgba(self) -> [f32; 4] {
        match self.ty.len() {
            1 => [self.x[0], self.x[0], self.x[0], 1.0],
            3 => [self.x[0], self.x[1], self.x[2], 1.0],
            _ => self.x,
        }
    }
}

/// A node parameter.
#[derive(Clone, Debug)]
pub enum Param {
    Const(V),
    Str(String),
    /// Driven by a function graph, evaluated once per node with the graph's
    /// inputs as variables.
    Expr(Arc<FnGraph>),
    /// Bound to a graph input (our own graphs' shorthand for a `get` function).
    Input(String),
    /// Array parameters (gradient keys, curve points): one map per cell.
    Array(Vec<HashMap<String, V>>),
}

/// How a size or seed parameter combines with its context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Relative {
    Absolute,
    #[default]
    ToInput,
    ToParent,
}

#[derive(Clone, Debug)]
pub struct NodeParam {
    pub value: Param,
    pub relative: Relative,
}

/// A link into a node input: the source node and which of its outputs.
#[derive(Clone, Debug)]
pub struct Link {
    pub node: u64,
    /// Output uid; `None` means the first output.
    pub output: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum NodeKind {
    /// An atomic node or one of ours (`reetched_*`), by filter name.
    Atomic(String),
    /// An instance of another graph: `pkg:///id` or `pkg:///id?dependency=uid`.
    /// `outputs` maps this node's output uids to the instanced graph's outputs.
    Instance {
        path: String,
        outputs: Vec<(u64, String)>,
    },
    /// The graph input with this uid.
    Input(u64),
    /// Writes the graph output with this uid.
    Output(u64),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub uid: u64,
    pub kind: NodeKind,
    /// Input slot name → source.
    pub links: Vec<(String, Link)>,
    /// Output uids (most nodes have one).
    pub outputs: Vec<u64>,
    pub params: HashMap<String, NodeParam>,
}

impl Node {
    pub fn link(&self, slot: &str) -> Option<&Link> {
        self.links.iter().find(|(s, _)| s == slot).map(|(_, l)| l)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Value,
    ColorImage,
    GrayImage,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    Slider,
    Color,
    Toggle,
    /// (value, label)
    Combo(Vec<(i32, String)>),
    Angle,
}

#[derive(Clone, Debug)]
pub struct InputDef {
    pub id: String,
    pub uid: u64,
    pub label: String,
    pub group: String,
    pub kind: InputKind,
    pub default: V,
    pub min: f32,
    pub max: f32,
    pub clamp: bool,
    pub widget: Widget,
    /// Image inputs: the mesh map or usage they expect (`ambientocclusion`, `curvature`, …).
    pub usage: Option<String>,
}

impl InputDef {
    /// A value input with a slider, our graphs' common case.
    pub fn value(id: &str, label: &str, default: V, min: f32, max: f32) -> Self {
        let widget = match default.ty {
            Ty::Bool => Widget::Toggle,
            Ty::Float(3 | 4) => Widget::Color,
            _ => Widget::Slider,
        };
        InputDef {
            id: id.into(),
            uid: 0,
            label: label.into(),
            group: String::new(),
            kind: InputKind::Value,
            default,
            min,
            max,
            clamp: true,
            widget,
            usage: None,
        }
    }

    /// The `$outputsize`, `$randomseed`, … inputs every graph has; not shown to users.
    pub fn is_builtin(&self) -> bool {
        self.id.starts_with('$')
    }
}

#[derive(Clone, Debug)]
pub struct OutputDef {
    pub id: String,
    pub uid: u64,
    pub label: String,
    /// `baseColor`, `normal`, `roughness`, …
    pub usage: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub id: String,
    pub label: String,
    pub inputs: Vec<InputDef>,
    pub outputs: Vec<OutputDef>,
    pub nodes: HashMap<u64, Node>,
    /// The graph's own output size (log2) and how it relates to the parent.
    pub size: Option<([i32; 2], Relative)>,
}

impl Graph {
    pub fn input(&self, id: &str) -> Option<&InputDef> {
        self.inputs.iter().find(|i| i.id == id)
    }
}

/// A loaded package (`.sbs` file or one of ours): graphs, function graphs,
/// bitmap resources and resolved dependencies.
#[derive(Clone, Debug, Default)]
pub struct Package {
    pub name: String,
    pub graphs: Vec<Graph>,
    pub functions: HashMap<String, Arc<FnGraph>>,
    /// Bitmap resources by their `pkg:///…` path.
    pub resources: HashMap<String, Arc<Image>>,
    /// Dependency uid → package, or why it could not be loaded.
    pub deps: HashMap<String, Result<Arc<Package>, String>>,
}

impl Package {
    pub fn graph(&self, id: &str) -> Option<&Graph> {
        self.graphs.iter().find(|g| g.id == id)
    }
}

/// Splits `pkg:///name?dependency=123` into (`name`, `Some("123")`).
pub fn split_path(path: &str) -> (&str, Option<&str>) {
    let path = path
        .trim_start_matches("pkg:///")
        .trim_start_matches("pkg://");
    match path.split_once('?') {
        Some((name, query)) => (
            name,
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix("dependency=")),
        ),
        None => (path, None),
    }
}
