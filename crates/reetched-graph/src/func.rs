//! Function graphs (Designer's Pixel Processor, parameter expressions,
//! function packages): compiled to a flat list of ops and interpreted.
//! ponytail: interpreted on the CPU; compile to WGSL when per-pixel graphs get slow.

use crate::{Image, Ty, V};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug)]
pub enum FnData {
    V(V),
    Str(String),
}

#[derive(Clone, Debug)]
pub struct FnNode {
    /// Designer function name: `add`, `const_float2`, `get_float1`, `samplecol`, …
    pub func: String,
    pub data: Vec<(String, FnData)>,
    /// Input slot → source node uid.
    pub links: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Default)]
pub struct FnGraph {
    pub nodes: HashMap<u64, FnNode>,
    pub root: u64,
    /// Inputs of a function package (instanced function graphs): name and default.
    pub inputs: Vec<(String, V)>,
}

impl FnGraph {
    /// A graph that just reads variable `name`.
    pub fn get(name: &str, ty: Ty) -> FnGraph {
        let func = match ty {
            Ty::Bool => "get_bool".to_string(),
            Ty::Int(n) => format!("get_integer{n}"),
            Ty::Float(n) => format!("get_float{n}"),
        };
        FnGraph {
            nodes: HashMap::from([(
                1,
                FnNode {
                    func,
                    data: vec![("__constant__".into(), FnData::Str(name.into()))],
                    links: vec![],
                },
            )]),
            root: 1,
            inputs: vec![],
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Un {
    Neg,
    Abs,
    Floor,
    Ceil,
    Cos,
    Sin,
    Tan,
    Acos,
    Asin,
    Sqrt,
    Log,
    Log2,
    Exp,
    Pow2,
    Not,
}

#[derive(Clone, Copy, Debug)]
enum Bin {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Min,
    Max,
    Pow,
    Dot,
    Cross,
    Atan2,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    And,
    Or,
    Cartesian,
}

#[derive(Clone, Debug)]
enum Op {
    Const(V),
    Get(usize),
    Set(usize, usize),
    Copy(usize),
    Un(Un, usize),
    Bin(Bin, usize, usize),
    MulScalar(usize, usize),
    Lerp(usize, usize, usize),
    IfElse(usize, usize, usize),
    Vector(u8, bool, usize, usize),
    Swizzle(u8, bool, [u8; 4], usize),
    Cast(Ty, usize),
    Sample {
        color: bool,
        input: usize,
        nearest: bool,
        pos: usize,
    },
    Rand(usize, u32),
}

/// A compiled function graph.
#[derive(Clone, Debug)]
pub struct Program {
    ops: Vec<Op>,
    /// Variable names, indexed by the slots `Get`/`Set` use.
    pub vars: Vec<String>,
    result: usize,
}

/// Finds function packages referenced by `instance` nodes.
pub type Resolver<'a> = &'a dyn Fn(&str) -> Option<Arc<FnGraph>>;

struct Compiler<'a> {
    ops: Vec<Op>,
    vars: Vec<String>,
    resolve: Resolver<'a>,
    errors: Vec<String>,
    depth: usize,
}

struct Frame<'g> {
    graph: &'g FnGraph,
    memo: HashMap<u64, usize>,
    /// Inlined function-package arguments: input name → register.
    env: HashMap<String, usize>,
}

fn str_data(n: &FnNode) -> Option<&str> {
    n.data.iter().find_map(|(_, d)| match d {
        FnData::Str(s) => Some(s.as_str()),
        _ => None,
    })
}

/// First value datum (whose name contains `hint`, if given).
fn v_data(n: &FnNode, hint: Option<&str>) -> Option<V> {
    n.data.iter().find_map(|(k, d)| match d {
        FnData::V(v) if hint.is_none_or(|h| k.contains(h)) => Some(*v),
        _ => None,
    })
}

impl Compiler<'_> {
    fn push(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }

    fn var(&mut self, name: &str) -> usize {
        match self.vars.iter().position(|v| v == name) {
            Some(i) => i,
            None => {
                self.vars.push(name.into());
                self.vars.len() - 1
            }
        }
    }

    /// Register for input `slot` of `node` (by name, else by position).
    fn arg(&mut self, f: &mut Frame, node: &FnNode, slot: &str, index: usize) -> usize {
        let uid = node
            .links
            .iter()
            .find(|(s, _)| s == slot)
            .or_else(|| node.links.get(index))
            .map(|(_, u)| *u);
        match uid {
            Some(u) => self.node(f, u),
            None => {
                self.errors
                    .push(format!("`{}` is missing input `{slot}`", node.func));
                self.push(Op::Const(V::f(0.0)))
            }
        }
    }

    fn node(&mut self, f: &mut Frame, uid: u64) -> usize {
        if let Some(&r) = f.memo.get(&uid) {
            return r;
        }
        let Some(n) = f.graph.nodes.get(&uid) else {
            self.errors.push(format!("function node {uid} is missing"));
            return self.push(Op::Const(V::f(0.0)));
        };
        let func = n.func.as_str();
        let r = self.compile(f, n, func);
        f.memo.insert(uid, r);
        r
    }

    fn compile(&mut self, f: &mut Frame, n: &FnNode, func: &str) -> usize {
        let un = |op| match op {
            "neg" => Some(Un::Neg),
            "abs" => Some(Un::Abs),
            "floor" => Some(Un::Floor),
            "ceil" => Some(Un::Ceil),
            "cos" => Some(Un::Cos),
            "sin" => Some(Un::Sin),
            "tan" => Some(Un::Tan),
            "acos" => Some(Un::Acos),
            "asin" => Some(Un::Asin),
            "sqrt" => Some(Un::Sqrt),
            "log" => Some(Un::Log),
            "log2" => Some(Un::Log2),
            "exp" => Some(Un::Exp),
            "pow2" => Some(Un::Pow2),
            "not" => Some(Un::Not),
            _ => None,
        };
        let bin = |op| match op {
            "add" => Some(Bin::Add),
            "sub" => Some(Bin::Sub),
            "mul" => Some(Bin::Mul),
            "div" => Some(Bin::Div),
            "mod" => Some(Bin::Mod),
            "min" => Some(Bin::Min),
            "max" => Some(Bin::Max),
            "pow" => Some(Bin::Pow),
            "dot" => Some(Bin::Dot),
            "cross" => Some(Bin::Cross),
            "eq" => Some(Bin::Eq),
            "noteq" => Some(Bin::Ne),
            "gt" => Some(Bin::Gt),
            "gteq" => Some(Bin::Ge),
            "lr" | "lt" => Some(Bin::Lt),
            "lteq" => Some(Bin::Le),
            "and" => Some(Bin::And),
            "or" => Some(Bin::Or),
            _ => None,
        };
        if func.starts_with("const_") {
            let v = v_data(n, None).unwrap_or_default();
            return self.push(Op::Const(v));
        }
        if func.starts_with("get_") {
            let name = str_data(n).unwrap_or_default().to_string();
            if let Some(&r) = f.env.get(&name) {
                return r;
            }
            let slot = self.var(&name);
            return self.push(Op::Get(slot));
        }
        if let Some(op) = un(func) {
            let a = self.arg(f, n, "a", 0);
            return self.push(Op::Un(op, a));
        }
        if func == "atan2" {
            if n.links.len() >= 2 {
                let a = self.arg(f, n, "a", 0);
                let b = self.arg(f, n, "b", 1);
                return self.push(Op::Bin(Bin::Atan2, a, b));
            }
            let a = self.arg(f, n, "a", 0);
            return self.push(Op::Bin(Bin::Atan2, a, usize::MAX));
        }
        if func == "cartesian" {
            let a = self.arg(f, n, "rho", 0);
            let b = self.arg(f, n, "theta", 1);
            return self.push(Op::Bin(Bin::Cartesian, a, b));
        }
        if let Some(op) = bin(func) {
            let a = self.arg(f, n, "a", 0);
            let b = self.arg(f, n, "b", 1);
            return self.push(Op::Bin(op, a, b));
        }
        match func {
            "mulscalar" => {
                let a = self.arg(f, n, "a", 0);
                let s = self.arg(f, n, "scalar", 1);
                self.push(Op::MulScalar(a, s))
            }
            "lerp" => {
                let a = self.arg(f, n, "a", 0);
                let b = self.arg(f, n, "b", 1);
                let x = self.arg(f, n, "x", 2);
                self.push(Op::Lerp(a, b, x))
            }
            "ifelse" => {
                let c = self.arg(f, n, "condition", 0);
                let a = self.arg(f, n, "ifpath", 1);
                let b = self.arg(f, n, "elsepath", 2);
                self.push(Op::IfElse(c, a, b))
            }
            "sequence" => {
                self.arg(f, n, "seq1", 0);
                let b = self.arg(f, n, "seq2", 1);
                self.push(Op::Copy(b))
            }
            "set" => {
                let name = str_data(n).unwrap_or_default().to_string();
                let v = self.arg(f, n, "value", 0);
                let slot = self.var(&name);
                self.push(Op::Set(slot, v))
            }
            "passthrough" => {
                let a = self.arg(f, n, "input", 0);
                self.push(Op::Copy(a))
            }
            "rand" => {
                let a = self.arg(f, n, "a", 0);
                let salt = self.ops.len() as u32;
                self.push(Op::Rand(a, salt))
            }
            "samplelum" | "samplecol" => {
                let pos = self.arg(f, n, "pos", 0);
                let input = n
                    .data
                    .iter()
                    .find_map(|(k, d)| match d {
                        FnData::V(v) if !k.contains("filter") => Some(v.int().max(0) as usize),
                        _ => None,
                    })
                    .unwrap_or(0);
                let nearest = v_data(n, Some("filter")).is_some_and(|v| v.int() == 1);
                self.push(Op::Sample {
                    color: func == "samplecol",
                    input,
                    nearest,
                    pos,
                })
            }
            "instance" => self.instance(f, n),
            _ => {
                if let Some(rest) = func
                    .strip_prefix("ivector")
                    .map(|r| (r, true))
                    .or_else(|| func.strip_prefix("vector").map(|r| (r, false)))
                    && let Ok(len) = rest.0.parse::<u8>()
                {
                    let a = self.arg(f, n, "componentsin", 0);
                    let b = self.arg(f, n, "componentslast", 1);
                    return self.push(Op::Vector(len, rest.1, a, b));
                }
                if let Some(rest) = func
                    .strip_prefix("iswizzle")
                    .map(|r| (r, true))
                    .or_else(|| func.strip_prefix("swizzle").map(|r| (r, false)))
                    && let Ok(len) = rest.0.parse::<u8>()
                {
                    let a = self.arg(f, n, "vector", 0);
                    let idx = v_data(n, None).map_or([0, 1, 2, 3], |v| {
                        std::array::from_fn(|i| v.x[i].clamp(0.0, 3.0) as u8)
                    });
                    return self.push(Op::Swizzle(len, rest.1, idx, a));
                }
                let cast = match func {
                    "tofloat" | "tofloat1" => Some(Ty::Float(1)),
                    "tofloat2" => Some(Ty::Float(2)),
                    "tofloat3" => Some(Ty::Float(3)),
                    "tofloat4" => Some(Ty::Float(4)),
                    "toint" | "toint1" => Some(Ty::Int(1)),
                    "toint2" => Some(Ty::Int(2)),
                    "toint3" => Some(Ty::Int(3)),
                    "toint4" => Some(Ty::Int(4)),
                    _ => None,
                };
                if let Some(ty) = cast {
                    let a = self.arg(f, n, "value", 0);
                    return self.push(Op::Cast(ty, a));
                }
                self.errors
                    .push(format!("unsupported function node `{func}`"));
                self.push(Op::Const(V::f(0.0)))
            }
        }
    }

    fn instance(&mut self, f: &mut Frame, n: &FnNode) -> usize {
        let path = str_data(n).unwrap_or_default().to_string();
        let Some(sub) = (self.resolve)(&path) else {
            self.errors.push(format!("missing function package {path}"));
            return self.push(Op::Const(V::f(0.0)));
        };
        if self.depth > 32 {
            self.errors
                .push("function instances nest too deeply".into());
            return self.push(Op::Const(V::f(0.0)));
        }
        let mut env = HashMap::new();
        for (name, default) in &sub.inputs {
            let r = match n.links.iter().find(|(s, _)| s == name) {
                Some((_, u)) => self.node(f, *u),
                None => self.push(Op::Const(*default)),
            };
            env.insert(name.clone(), r);
        }
        let mut inner = Frame {
            graph: &sub,
            memo: HashMap::new(),
            env,
        };
        self.depth += 1;
        let r = self.node(&mut inner, sub.root);
        self.depth -= 1;
        r
    }
}

impl Program {
    /// Compiles `g`. Errors (unsupported nodes, missing links) are returned
    /// alongside a program that substitutes zeros.
    pub fn compile(g: &FnGraph, resolve: Resolver) -> (Program, Vec<String>) {
        let mut c = Compiler {
            ops: Vec::new(),
            vars: Vec::new(),
            resolve,
            errors: Vec::new(),
            depth: 0,
        };
        let mut frame = Frame {
            graph: g,
            memo: HashMap::new(),
            env: HashMap::new(),
        };
        let result = c.node(&mut frame, g.root);
        (
            Program {
                ops: c.ops,
                vars: c.vars,
                result,
            },
            c.errors,
        )
    }

    pub fn var(&self, name: &str) -> Option<usize> {
        self.vars.iter().position(|v| v == name)
    }

    /// Runs the program. `regs` is scratch space (reused between calls),
    /// `vars` holds variable values in `self.vars` order.
    pub fn run(&self, regs: &mut Vec<V>, vars: &mut [V], images: &[&Image], seed: i32) -> V {
        regs.clear();
        for op in &self.ops {
            let v = match *op {
                Op::Const(v) => v,
                Op::Get(s) => vars[s],
                Op::Set(s, a) => {
                    vars[s] = regs[a];
                    regs[a]
                }
                Op::Copy(a) => regs[a],
                Op::Un(u, a) => unary(u, regs[a]),
                Op::Bin(b, a, c) => {
                    let rb = if c == usize::MAX {
                        V::default()
                    } else {
                        regs[c]
                    };
                    binary(b, regs[a], rb)
                }
                Op::MulScalar(a, s) => {
                    let (a, s) = (regs[a], regs[s].x[0]);
                    a.with(a.x.map(|x| x * s))
                }
                Op::Lerp(a, b, t) => {
                    let (a, b, t) = (regs[a], regs[b], regs[t]);
                    let n = a.ty.len().max(b.ty.len());
                    let ty = float_ty(n);
                    V {
                        ty,
                        x: std::array::from_fn(|i| {
                            let ti = comp(t, i);
                            comp(a, i) + (comp(b, i) - comp(a, i)) * ti
                        }),
                    }
                }
                Op::IfElse(c, a, b) => {
                    if regs[c].bool() {
                        regs[a]
                    } else {
                        regs[b]
                    }
                }
                Op::Vector(len, int, a, b) => {
                    let (a, b) = (regs[a], regs[b]);
                    let mut x = [0.0; 4];
                    let na = a.ty.len().min(len as usize);
                    x[..na].copy_from_slice(&a.x[..na]);
                    for (i, xi) in x.iter_mut().enumerate().take(len as usize).skip(na) {
                        *xi = b.x[(i - na).min(3)];
                    }
                    V {
                        ty: if int { Ty::Int(len) } else { Ty::Float(len) },
                        x,
                    }
                }
                Op::Swizzle(len, int, idx, a) => {
                    let a = regs[a];
                    V {
                        ty: if int { Ty::Int(len) } else { Ty::Float(len) },
                        x: std::array::from_fn(|i| {
                            if i < len as usize {
                                a.x[idx[i] as usize]
                            } else {
                                0.0
                            }
                        }),
                    }
                }
                Op::Cast(ty, a) => {
                    let a = regs[a];
                    let x = if matches!(ty, Ty::Int(_)) {
                        a.x.map(f32::trunc)
                    } else {
                        a.x
                    };
                    V { ty, x }
                }
                Op::Sample {
                    color,
                    input,
                    nearest,
                    pos,
                } => {
                    let p = regs[pos];
                    match images.get(input) {
                        Some(img) => {
                            let c = if nearest {
                                img.sample_nearest(p.x[0], p.x[1])
                            } else {
                                img.sample(p.x[0], p.x[1])
                            };
                            if color {
                                V::f4(c)
                            } else if img.is_color() {
                                V::f(crate::image::luma(c))
                            } else {
                                V::f(c[0])
                            }
                        }
                        None if color => V::f4([0.0, 0.0, 0.0, 1.0]),
                        None => V::f(0.0),
                    }
                }
                Op::Rand(a, salt) => {
                    let a = regs[a];
                    let r = hash01(seed as u32, salt);
                    a.with(a.x.map(|m| m * r))
                }
            };
            regs.push(v);
        }
        regs[self.result]
    }

    /// Evaluates once with the given variables (parameter expressions).
    pub fn eval(&self, lookup: &dyn Fn(&str) -> V, seed: i32) -> V {
        let mut vars: Vec<V> = self.vars.iter().map(|n| lookup(n)).collect();
        self.run(&mut Vec::new(), &mut vars, &[], seed)
    }
}

fn float_ty(n: usize) -> Ty {
    Ty::Float(n.clamp(1, 4) as u8)
}

/// Component `i`, broadcasting scalars.
fn comp(v: V, i: usize) -> f32 {
    if v.ty.len() == 1 { v.x[0] } else { v.x[i] }
}

fn unary(u: Un, a: V) -> V {
    let f = |g: fn(f32) -> f32| a.with(a.x.map(g));
    match u {
        Un::Neg => f(|x| -x),
        Un::Abs => f(f32::abs),
        Un::Floor => f(f32::floor),
        Un::Ceil => f(f32::ceil),
        Un::Cos => f(f32::cos),
        Un::Sin => f(f32::sin),
        Un::Tan => f(f32::tan),
        Un::Acos => f(f32::acos),
        Un::Asin => f(f32::asin),
        Un::Sqrt => f(f32::sqrt),
        Un::Log => f(f32::ln),
        Un::Log2 => f(f32::log2),
        Un::Exp => f(f32::exp),
        Un::Pow2 => f(f32::exp2),
        Un::Not => V::b(!a.bool()),
    }
}

fn binary(b: Bin, a: V, c: V) -> V {
    let n = a.ty.len().max(c.ty.len());
    let int = matches!(a.ty, Ty::Int(_)) && matches!(c.ty, Ty::Int(_));
    let ty = if int { Ty::Int(n as u8) } else { float_ty(n) };
    let zip = |f: &dyn Fn(f32, f32) -> f32| V {
        ty,
        x: std::array::from_fn(|i| {
            if i < n {
                f(comp(a, i), comp(c, i))
            } else {
                0.0
            }
        }),
    };
    match b {
        Bin::Add => zip(&|x, y| x + y),
        Bin::Sub => zip(&|x, y| x - y),
        Bin::Mul => zip(&|x, y| x * y),
        Bin::Div if int => zip(&|x, y| if y == 0.0 { 0.0 } else { (x / y).trunc() }),
        Bin::Div => zip(&|x, y| x / y),
        Bin::Mod => zip(&|x, y| {
            if y == 0.0 {
                0.0
            } else {
                x - y * (x / y).floor()
            }
        }),
        Bin::Min => zip(&f32::min),
        Bin::Max => zip(&f32::max),
        Bin::Pow => zip(&f32::powf),
        Bin::Dot => V::f((0..n).map(|i| comp(a, i) * comp(c, i)).sum()),
        Bin::Cross => V::f3([
            a.x[1] * c.x[2] - a.x[2] * c.x[1],
            a.x[2] * c.x[0] - a.x[0] * c.x[2],
            a.x[0] * c.x[1] - a.x[1] * c.x[0],
        ]),
        Bin::Atan2 if a.ty.len() >= 2 && c == V::default() => V::f(a.x[1].atan2(a.x[0])),
        Bin::Atan2 => V::f(a.x[0].atan2(c.x[0])),
        Bin::Cartesian => V::f2(a.x[0] * c.x[0].cos(), a.x[0] * c.x[0].sin()),
        Bin::Eq => V::b(a.x[0] == c.x[0]),
        Bin::Ne => V::b(a.x[0] != c.x[0]),
        Bin::Gt => V::b(a.x[0] > c.x[0]),
        Bin::Ge => V::b(a.x[0] >= c.x[0]),
        Bin::Lt => V::b(a.x[0] < c.x[0]),
        Bin::Le => V::b(a.x[0] <= c.x[0]),
        Bin::And => V::b(a.bool() && c.bool()),
        Bin::Or => V::b(a.bool() || c.bool()),
    }
}

/// Deterministic hash → [0, 1).
pub fn hash01(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(func: &str, data: Vec<(&str, FnData)>, links: &[(&str, u64)]) -> FnNode {
        FnNode {
            func: func.into(),
            data: data.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            links: links.iter().map(|(s, u)| (s.to_string(), *u)).collect(),
        }
    }

    #[test]
    fn arithmetic_vars_and_swizzle() {
        // (pos.y * 2 + 1) with pos = (0.25, 0.5)  → 2
        let g = FnGraph {
            nodes: HashMap::from([
                (
                    1,
                    node(
                        "get_float2",
                        vec![("__constant__", FnData::Str("$pos".into()))],
                        &[],
                    ),
                ),
                (
                    2,
                    node(
                        "swizzle1",
                        vec![("__constant__", FnData::V(V::i(1)))],
                        &[("vector", 1)],
                    ),
                ),
                (
                    3,
                    node(
                        "const_float1",
                        vec![("__constant__", FnData::V(V::f(2.0)))],
                        &[],
                    ),
                ),
                (4, node("mul", vec![], &[("a", 2), ("b", 3)])),
                (
                    5,
                    node(
                        "const_float1",
                        vec![("__constant__", FnData::V(V::f(1.0)))],
                        &[],
                    ),
                ),
                (6, node("add", vec![], &[("a", 4), ("b", 5)])),
            ]),
            root: 6,
            inputs: vec![],
        };
        let (p, errors) = Program::compile(&g, &|_| None);
        assert!(errors.is_empty(), "{errors:?}");
        let v = p.eval(
            &|n| {
                if n == "$pos" {
                    V::f2(0.25, 0.5)
                } else {
                    V::default()
                }
            },
            0,
        );
        assert_eq!(v.float(), 2.0);
    }

    #[test]
    fn function_instances_are_inlined() {
        // double(x) = x + x, called with 3.
        let double = Arc::new(FnGraph {
            nodes: HashMap::from([
                (
                    1,
                    node("get_float1", vec![("n", FnData::Str("x".into()))], &[]),
                ),
                (2, node("add", vec![], &[("a", 1), ("b", 1)])),
            ]),
            root: 2,
            inputs: vec![("x".into(), V::f(0.0))],
        });
        let g = FnGraph {
            nodes: HashMap::from([
                (
                    1,
                    node("const_float1", vec![("v", FnData::V(V::f(3.0)))], &[]),
                ),
                (
                    2,
                    node(
                        "instance",
                        vec![("path", FnData::Str("pkg:///double".into()))],
                        &[("x", 1)],
                    ),
                ),
            ]),
            root: 2,
            inputs: vec![],
        };
        let (p, errors) = Program::compile(&g, &|path| {
            (path == "pkg:///double").then(|| double.clone())
        });
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(p.eval(&|_| V::default(), 0).float(), 6.0);
    }
}
