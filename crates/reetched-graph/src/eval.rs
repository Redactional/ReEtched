//! Graph evaluation: sizes, seeds, instances, parameter expressions.

use crate::{
    Graph, Image, InputDef, InputKind, Link, Node, NodeKind, OutputDef, Package, Param, Relative,
    V, atomic,
    func::{FnGraph, Program},
    split_path,
};
use std::{collections::HashMap, sync::Arc};

/// What to render a graph with.
#[derive(Clone, Debug, Default)]
pub struct Args {
    /// Output size (log2), the graph's "parent" size.
    pub size_log2: [i32; 2],
    pub seed: i32,
    /// Input value overrides by input id.
    pub values: HashMap<String, V>,
    /// Image inputs by input id.
    pub images: HashMap<String, Arc<Image>>,
    /// Image inputs by usage (`ambientocclusion`, `curvature`, …), used when
    /// `images` has nothing for an input with that usage.
    pub usages: HashMap<String, Arc<Image>>,
}

pub struct Rendered {
    pub outputs: Vec<(OutputDef, Arc<Image>)>,
    /// Unsupported nodes, missing dependencies, …
    pub warnings: Vec<String>,
}

/// Renders graph `graph_id` of `pkg`.
pub fn render(pkg: &Package, graph_id: &str, args: &Args) -> Rendered {
    let mut ev = Ev {
        warnings: Vec::new(),
        depth: 0,
    };
    let Some(graph) = pkg.graph(graph_id) else {
        return Rendered {
            outputs: Vec::new(),
            warnings: vec![format!("{} has no graph {graph_id}", pkg.name)],
        };
    };
    let size = graph_size(graph, args.size_log2);
    let sc = Scope {
        pkg,
        graph,
        size,
        seed: args.seed,
        values: args.values.clone(),
        images: args.images.clone(),
        usages: &args.usages,
    };
    let outputs = ev.graph(&sc);
    let mut seen = std::collections::HashSet::new();
    ev.warnings.retain(|w| seen.insert(w.clone()));
    Rendered {
        outputs: graph
            .outputs
            .iter()
            .filter_map(|o| outputs.get(&o.uid).map(|i| (o.clone(), i.clone())))
            .collect(),
        warnings: ev.warnings,
    }
}

fn graph_size(graph: &Graph, parent: [i32; 2]) -> [i32; 2] {
    match graph.size {
        Some((v, Relative::Absolute)) => v,
        Some((v, _)) => [parent[0] + v[0], parent[1] + v[1]],
        None => parent,
    }
}

fn clamp_size(s: [i32; 2]) -> [i32; 2] {
    s.map(|v| v.clamp(0, 13))
}

struct Scope<'a> {
    pkg: &'a Package,
    graph: &'a Graph,
    size: [i32; 2],
    seed: i32,
    values: HashMap<String, V>,
    images: HashMap<String, Arc<Image>>,
    usages: &'a HashMap<String, Arc<Image>>,
}

impl Scope<'_> {
    /// Value of graph input or built-in variable `name`.
    fn var(&self, name: &str) -> V {
        if let Some(v) = self.values.get(name) {
            return *v;
        }
        match name {
            "$size" => V::f2((1 << self.size[0]) as f32, (1 << self.size[1]) as f32),
            "$sizelog2" => V::f2(self.size[0] as f32, self.size[1] as f32),
            "$randomseed" => V::i(self.seed),
            // 1 = OpenGL (Painter's default and ours).
            "$normalformat" => V::i(1),
            "$time" => V::f(0.0),
            "$tiling" => V::i(3),
            _ => self
                .graph
                .input(name)
                .map_or_else(V::default, |i| i.default),
        }
    }

    fn resolve_fn(&self, path: &str) -> Option<Arc<FnGraph>> {
        let (name, dep) = split_path(path);
        let pkg = match dep {
            Some(d) => self.pkg.deps.get(d)?.as_ref().ok()?.as_ref(),
            None => self.pkg,
        };
        pkg.functions.get(name).cloned()
    }
}

struct Ev {
    warnings: Vec<String>,
    depth: usize,
}

/// Images waiting for their consumers: (node, output) → (image, uses left).
type Memo = HashMap<(u64, u64), (Arc<Image>, usize)>;

impl Ev {
    fn graph(&mut self, sc: &Scope) -> HashMap<u64, Arc<Image>> {
        let g = sc.graph;
        // How many links read each node output, so images are dropped when done.
        let mut uses: HashMap<(u64, u64), usize> = HashMap::new();
        for n in g.nodes.values() {
            for (_, l) in &n.links {
                *uses.entry(link_key(g, l)).or_default() += 1;
            }
        }
        let mut memo = Memo::new();
        let mut out = HashMap::new();
        let mut outputs: Vec<&Node> = g
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::Output(_)))
            .collect();
        outputs.sort_by_key(|n| n.uid);
        for n in outputs {
            let NodeKind::Output(uid) = n.kind else {
                continue;
            };
            let img = match n.links.first() {
                Some((_, l)) => self.link(sc, l, &mut memo, &uses),
                None => None,
            };
            let img = img.unwrap_or_else(|| {
                let [w, h] = clamp_size(sc.size).map(|s| 1usize << s);
                Arc::new(Image::uniform(w, h, [0.0, 0.0, 0.0, 1.0], false))
            });
            out.insert(uid, img);
        }
        out
    }

    fn link(
        &mut self,
        sc: &Scope,
        l: &Link,
        memo: &mut Memo,
        uses: &HashMap<(u64, u64), usize>,
    ) -> Option<Arc<Image>> {
        let key = link_key(sc.graph, l);
        if !memo.contains_key(&key) {
            let node = sc.graph.nodes.get(&l.node)?;
            for (out, img) in self.node(sc, node, memo, uses) {
                let k = (node.uid, out);
                if let Some(&n) = uses.get(&k) {
                    memo.insert(k, (img, n));
                }
            }
        }
        let (img, left) = memo.get_mut(&key)?;
        let img = img.clone();
        *left -= 1;
        if *left == 0 {
            memo.remove(&key);
        }
        Some(img)
    }

    fn node(
        &mut self,
        sc: &Scope,
        n: &Node,
        memo: &mut Memo,
        uses: &HashMap<(u64, u64), usize>,
    ) -> Vec<(u64, Arc<Image>)> {
        let first_out = n.outputs.first().copied().unwrap_or(0);
        match &n.kind {
            NodeKind::Output(_) => Vec::new(),
            NodeKind::Input(uid) => {
                let Some(def) = sc.graph.inputs.iter().find(|i| i.uid == *uid) else {
                    self.warnings
                        .push(format!("input node {} has no input", n.uid));
                    return Vec::new();
                };
                let img = sc
                    .images
                    .get(&def.id)
                    .or_else(|| {
                        def.usage
                            .as_ref()
                            .and_then(|u| sc.usages.get(&u.to_lowercase()))
                    })
                    .cloned()
                    .unwrap_or_else(|| Arc::new(default_input(def, sc.size)));
                vec![(first_out, img)]
            }
            NodeKind::Atomic(name) => {
                let inputs: Vec<(String, Option<Arc<Image>>)> = n
                    .links
                    .iter()
                    .map(|(slot, l)| (slot.clone(), self.link(sc, l, memo, uses)))
                    .collect();
                let size = clamp_size(self.node_size(sc, n, &inputs, false));
                let seed = self.seed(sc, n);
                let img = if name == "pixelprocessor" {
                    self.pixel_processor(sc, n, &inputs, size, seed)
                } else {
                    let cx = atomic::Cx {
                        w: 1 << size[0],
                        h: 1 << size[1],
                        seed,
                        inputs: inputs
                            .into_iter()
                            .filter_map(|(s, i)| i.map(|i| (s, i)))
                            .collect(),
                        params: n
                            .params
                            .keys()
                            .filter_map(|k| self.param(sc, n, k).map(|v| (k.clone(), v)))
                            .collect(),
                        arrays: n
                            .params
                            .iter()
                            .filter_map(|(k, p)| match &p.value {
                                Param::Array(a) => Some((k.clone(), a.clone())),
                                _ => None,
                            })
                            .collect(),
                        strs: n
                            .params
                            .iter()
                            .filter_map(|(k, p)| match &p.value {
                                Param::Str(s) => Some((k.clone(), s.clone())),
                                _ => None,
                            })
                            .collect(),
                        resources: &sc.pkg.resources,
                    };
                    match atomic::run(name, &cx) {
                        Ok(img) => img,
                        Err(e) => {
                            self.warnings.push(e);
                            Image::uniform(cx.w, cx.h, [0.5, 0.5, 0.5, 1.0], false)
                        }
                    }
                };
                vec![(first_out, Arc::new(img))]
            }
            NodeKind::Instance { path, outputs } => self.instance(sc, n, path, outputs, memo, uses),
        }
    }

    fn instance(
        &mut self,
        sc: &Scope,
        n: &Node,
        path: &str,
        outputs: &[(u64, String)],
        memo: &mut Memo,
        uses: &HashMap<(u64, u64), usize>,
    ) -> Vec<(u64, Arc<Image>)> {
        let (name, dep) = split_path(path);
        let pkg = match dep {
            Some(d) => match sc.pkg.deps.get(d) {
                Some(Ok(p)) => p.as_ref(),
                Some(Err(e)) => {
                    self.warnings.push(e.clone());
                    return self.fallback(sc, n);
                }
                None => {
                    self.warnings
                        .push(format!("{path}: dependency {d} is not listed"));
                    return self.fallback(sc, n);
                }
            },
            None => sc.pkg,
        };
        let Some(graph) = pkg.graph(name) else {
            self.warnings
                .push(format!("{path}: no graph {name} in {}", pkg.name));
            return self.fallback(sc, n);
        };
        if self.depth > 64 {
            self.warnings.push("graph instances nest too deeply".into());
            return self.fallback(sc, n);
        }
        let inputs: Vec<(String, Option<Arc<Image>>)> = n
            .links
            .iter()
            .map(|(slot, l)| (slot.clone(), self.link(sc, l, memo, uses)))
            .collect();
        let size = clamp_size(self.node_size(sc, n, &inputs, true));
        let mut values = HashMap::new();
        for k in n.params.keys() {
            if let Some(v) = self.param(sc, n, k) {
                values.insert(k.clone(), v);
            }
        }
        let inner = Scope {
            pkg,
            graph,
            size: graph_size(graph, size),
            seed: self.seed(sc, n),
            values,
            images: inputs
                .into_iter()
                .filter_map(|(s, i)| i.map(|i| (s, i)))
                .collect(),
            usages: sc.usages,
        };
        self.depth += 1;
        let result = self.graph(&inner);
        self.depth -= 1;
        let by_id: HashMap<&str, &Arc<Image>> = graph
            .outputs
            .iter()
            .filter_map(|o| result.get(&o.uid).map(|i| (o.id.as_str(), i)))
            .collect();
        if outputs.is_empty() {
            // No bridging info: match outputs by position.
            return n
                .outputs
                .iter()
                .zip(&graph.outputs)
                .filter_map(|(uid, o)| by_id.get(o.id.as_str()).map(|i| (*uid, (*i).clone())))
                .collect();
        }
        outputs
            .iter()
            .filter_map(|(uid, id)| by_id.get(id.as_str()).map(|i| (*uid, (*i).clone())))
            .collect()
    }

    /// Grey images for every output of a node that could not be evaluated.
    fn fallback(&self, sc: &Scope, n: &Node) -> Vec<(u64, Arc<Image>)> {
        let [w, h] = clamp_size(sc.size).map(|s| 1usize << s);
        let img = Arc::new(Image::uniform(w, h, [0.5, 0.5, 0.5, 1.0], false));
        n.outputs.iter().map(|o| (*o, img.clone())).collect()
    }

    fn node_size(
        &mut self,
        sc: &Scope,
        n: &Node,
        inputs: &[(String, Option<Arc<Image>>)],
        instance: bool,
    ) -> [i32; 2] {
        let input_size = inputs
            .iter()
            .find_map(|(_, i)| i.as_ref())
            .map(|i| [i.w.trailing_zeros() as i32, i.h.trailing_zeros() as i32]);
        let param = n.params.get("outputsize");
        let v = param.and_then(|_| self.param(sc, n, "outputsize"));
        let rel = param.map(|p| p.relative);
        let offset = v.map_or([0, 0], |v| [v.x[0] as i32, v.x[1] as i32]);
        let add = |s: [i32; 2]| [s[0] + offset[0], s[1] + offset[1]];
        match rel {
            Some(Relative::Absolute) => offset,
            Some(Relative::ToParent) => add(sc.size),
            Some(Relative::ToInput) if input_size.is_some() && !instance => {
                add(input_size.unwrap())
            }
            Some(Relative::ToInput) => add(sc.size),
            None if instance => sc.size,
            None => input_size.unwrap_or(sc.size),
        }
    }

    fn seed(&mut self, sc: &Scope, n: &Node) -> i32 {
        let Some(p) = n.params.get("randomseed") else {
            return sc.seed;
        };
        let v = self.param(sc, n, "randomseed").map_or(0, |v| v.int());
        match p.relative {
            Relative::Absolute => v,
            _ => sc.seed.wrapping_add(v),
        }
    }

    /// Resolves a node parameter in its graph's context.
    fn param(&mut self, sc: &Scope, n: &Node, name: &str) -> Option<V> {
        match &n.params.get(name)?.value {
            Param::Const(v) => Some(*v),
            Param::Input(id) => Some(sc.var(id)),
            Param::Expr(g) => {
                let (prog, errors) = Program::compile(g, &|p| sc.resolve_fn(p));
                self.warnings.extend(
                    errors
                        .into_iter()
                        .map(|e| format!("{}: parameter {name}: {e}", sc.graph.id)),
                );
                Some(prog.eval(&|v| sc.var(v), sc.seed))
            }
            Param::Str(_) | Param::Array(_) => None,
        }
    }

    fn pixel_processor(
        &mut self,
        sc: &Scope,
        n: &Node,
        inputs: &[(String, Option<Arc<Image>>)],
        size: [i32; 2],
        seed: i32,
    ) -> Image {
        let [w, h] = size.map(|s| 1usize << s);
        let color = self.param(sc, n, "colorswitch").is_none_or(|v| v.bool());
        let Some(Param::Expr(g)) = n.params.get("perpixel").map(|p| &p.value) else {
            self.warnings.push("pixel processor has no function".into());
            return Image::uniform(w, h, [0.0, 0.0, 0.0, 1.0], color);
        };
        let (prog, errors) = Program::compile(g, &|p| sc.resolve_fn(p));
        self.warnings.extend(errors);
        // Inputs ordered input, input:1 / input1, input2, …
        let mut imgs: Vec<(usize, &Image)> = inputs
            .iter()
            .filter_map(|(s, i)| {
                let digits: String = s.chars().filter(char::is_ascii_digit).collect();
                Some((digits.parse().unwrap_or(0), i.as_deref()?))
            })
            .collect();
        imgs.sort_by_key(|(k, _)| *k);
        let imgs: Vec<&Image> = imgs.into_iter().map(|(_, i)| i).collect();
        let template: Vec<V> = prog
            .vars
            .iter()
            .map(|v| match v.as_str() {
                "$size" => V::f2(w as f32, h as f32),
                "$sizelog2" => V::f2(size[0] as f32, size[1] as f32),
                _ => sc.var(v),
            })
            .collect();
        let pos = prog.var("$pos");
        let run = |x: usize, y: usize, regs: &mut Vec<V>, vars: &mut Vec<V>| {
            vars.clone_from(&template);
            if let Some(p) = pos {
                vars[p] = V::f2((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            }
            prog.run(regs, vars, &imgs, seed).rgba()
        };
        use rayon::prelude::*;
        let rows: Vec<Vec<[f32; 4]>> = (0..h)
            .into_par_iter()
            .map_init(
                || (Vec::new(), Vec::new()),
                |(regs, vars), y| (0..w).map(|x| run(x, y, regs, vars)).collect(),
            )
            .collect();
        Image::new(w, h, color, |x, y| rows[y][x])
    }
}

fn link_key(g: &Graph, l: &Link) -> (u64, u64) {
    let out = l.output.unwrap_or_else(|| {
        g.nodes
            .get(&l.node)
            .and_then(|n| n.outputs.first().copied())
            .unwrap_or(0)
    });
    (l.node, out)
}

/// What an unconnected image input reads: neutral values for mesh maps.
fn default_input(def: &InputDef, size: [i32; 2]) -> Image {
    let [w, h] = clamp_size(size).map(|s| 1usize << s);
    let usage = def.usage.as_deref().unwrap_or_default().to_lowercase();
    let c = match usage.as_str() {
        "ambientocclusion" | "ao" => [1.0; 4],
        "curvature" | "position" | "thickness" => [0.5, 0.5, 0.5, 1.0],
        "normal" | "worldspacenormal" => [0.5, 0.5, 1.0, 1.0],
        _ => [0.0, 0.0, 0.0, 1.0],
    };
    Image::uniform(w, h, c, def.kind == InputKind::ColorImage)
}
