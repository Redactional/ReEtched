//! Our own procedural material library (original work, no Adobe content),
//! plus the small builder used to write graphs in code.

use crate::{
    Graph, InputDef, Link, Node, NodeKind, NodeParam, OutputDef, Package, Param, Relative, V,
};
use std::{collections::HashMap, sync::Arc};

pub struct Builder {
    g: Graph,
    next: u64,
}

impl From<V> for Param {
    fn from(v: V) -> Param {
        Param::Const(v)
    }
}

/// A parameter bound to graph input `id`.
pub fn input(id: &str) -> Param {
    Param::Input(id.into())
}

/// Gradient keys for the gradient map: (position, sRGB color).
pub fn gradient(keys: &[(f32, [f32; 3])]) -> Param {
    Param::Array(
        keys.iter()
            .map(|(p, c)| {
                HashMap::from([
                    ("position".to_string(), V::f(*p)),
                    ("value".to_string(), V::f3(*c)),
                ])
            })
            .collect(),
    )
}

impl Builder {
    pub fn new(id: &str, label: &str) -> Self {
        Builder {
            g: Graph {
                id: id.into(),
                label: label.into(),
                ..Default::default()
            },
            next: 1,
        }
    }

    pub fn input(&mut self, mut def: InputDef) {
        def.uid = self.next;
        self.next += 1;
        self.g.inputs.push(def);
    }

    /// Adds a node; returns its uid (also its output's uid).
    pub fn node(&mut self, filter: &str, params: Vec<(&str, Param)>, links: &[(&str, u64)]) -> u64 {
        let uid = self.next;
        self.next += 1;
        self.g.nodes.insert(
            uid,
            Node {
                uid,
                kind: NodeKind::Atomic(filter.into()),
                links: links
                    .iter()
                    .map(|(s, n)| {
                        (
                            s.to_string(),
                            Link {
                                node: *n,
                                output: None,
                            },
                        )
                    })
                    .collect(),
                outputs: vec![uid],
                params: params
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k.to_string(),
                            NodeParam {
                                value: v,
                                relative: Relative::ToInput,
                            },
                        )
                    })
                    .collect(),
            },
        );
        uid
    }

    pub fn output(&mut self, id: &str, usage: &str, src: u64) {
        let uid = self.next;
        self.next += 2;
        self.g.outputs.push(OutputDef {
            id: id.into(),
            uid,
            label: id.into(),
            usage: Some(usage.into()),
        });
        self.g.nodes.insert(
            uid + 1,
            Node {
                uid: uid + 1,
                kind: NodeKind::Output(uid),
                links: vec![(
                    "input".into(),
                    Link {
                        node: src,
                        output: None,
                    },
                )],
                outputs: vec![],
                params: HashMap::new(),
            },
        );
    }

    pub fn build(self) -> Graph {
        self.g
    }

    // Shorthands for common nodes.

    pub fn uniform(&mut self, color: Param, is_color: bool) -> u64 {
        self.node(
            "uniform",
            vec![
                ("outputcolor", color),
                ("colorswitch", V::b(is_color).into()),
            ],
            &[],
        )
    }

    pub fn levels(&mut self, src: u64, out_lo: Param, out_hi: Param) -> u64 {
        self.node(
            "levels",
            vec![("leveloutlow", out_lo), ("levelouthigh", out_hi)],
            &[("input1", src)],
        )
    }

    /// `fg` over `bg` with blend `mode` and opacity (optionally masked).
    pub fn blend(&mut self, fg: u64, bg: u64, mask: Option<u64>, mode: i32, opacity: Param) -> u64 {
        let mut links = vec![("source", fg), ("destination", bg)];
        if let Some(m) = mask {
            links.push(("opacity", m));
        }
        self.node(
            "blend",
            vec![
                ("blendingmode", V::i(mode).into()),
                ("opacitymult", opacity),
            ],
            &links,
        )
    }

    pub fn normal(&mut self, height: u64, intensity: Param) -> u64 {
        self.node(
            "normal",
            vec![("intensity", intensity), ("inversedy", V::b(true).into())],
            &[("input1", height)],
        )
    }

    /// Standard PBR outputs.
    pub fn pbr(&mut self, base: u64, metallic: u64, roughness: u64, height: u64, normal: u64) {
        self.output("basecolor", "baseColor", base);
        self.output("metallic", "metallic", metallic);
        self.output("roughness", "roughness", roughness);
        self.output("height", "height", height);
        self.output("normal", "normal", normal);
    }
}

pub const MULTIPLY: i32 = 3;
pub const ADD_SUB: i32 = 4;

fn color_in(id: &str, label: &str, srgb: [f32; 3]) -> InputDef {
    InputDef::value(id, label, V::f3(srgb), 0.0, 1.0)
}

fn float_in(id: &str, label: &str, v: f32, min: f32, max: f32) -> InputDef {
    InputDef::value(id, label, V::f(v), min, max)
}

fn int_in(id: &str, label: &str, v: i32, min: f32, max: f32) -> InputDef {
    InputDef::value(id, label, V::i(v), min, max)
}

fn noise(b: &mut Builder, scale: Param, stretch: f32, octaves: i32, kind: i32, seed: i32) -> u64 {
    b.node(
        "reetched_noise",
        vec![
            ("scale", scale),
            ("stretch", V::f(stretch).into()),
            ("octaves", V::i(octaves).into()),
            ("kind", V::i(kind).into()),
            ("randomseed", V::i(seed).into()),
        ],
        &[],
    )
}

/// Roughness around an input value: `base` ± `amount` driven by `pattern`.
fn varied(b: &mut Builder, base: &str, pattern: u64, amount: f32) -> u64 {
    let r = b.uniform(input(base), false);
    b.blend(pattern, r, None, ADD_SUB, V::f(amount).into())
}

fn brushed_steel() -> Graph {
    let mut b = Builder::new("brushed_steel", "Brushed Steel");
    b.input(color_in("color", "Color", [0.77, 0.77, 0.79]));
    b.input(float_in("roughness", "Roughness", 0.32, 0.0, 1.0));
    b.input(int_in("brush_scale", "Brush scale", 2, 1.0, 16.0));
    let streaks = noise(&mut b, input("brush_scale"), 96.0, 4, 0, 1);
    let tint = b.levels(streaks, V::f(0.88).into(), V::f(1.0).into());
    let color = b.uniform(input("color"), true);
    let base = b.blend(tint, color, None, MULTIPLY, V::f(1.0).into());
    let metal = b.uniform(V::f(1.0).into(), false);
    let rough = varied(&mut b, "roughness", streaks, 0.08);
    let height = b.levels(streaks, V::f(0.45).into(), V::f(0.55).into());
    let normal = b.normal(streaks, V::f(0.15).into());
    b.pbr(base, metal, rough, height, normal);
    b.build()
}

fn rough_plastic() -> Graph {
    let mut b = Builder::new("rough_plastic", "Rough Plastic");
    b.input(color_in("color", "Color", [0.18, 0.36, 0.62]));
    b.input(float_in("roughness", "Roughness", 0.55, 0.0, 1.0));
    b.input(float_in("bump", "Bump", 0.4, 0.0, 2.0));
    let grain = noise(&mut b, V::i(48).into(), 1.0, 3, 0, 2);
    let base = b.uniform(input("color"), true);
    let metal = b.uniform(V::f(0.0).into(), false);
    let rough = varied(&mut b, "roughness", grain, 0.05);
    let height = b.levels(grain, V::f(0.48).into(), V::f(0.52).into());
    let normal = b.normal(grain, input("bump"));
    b.pbr(base, metal, rough, height, normal);
    b.build()
}

fn wood_planks() -> Graph {
    let mut b = Builder::new("wood_planks", "Wood Planks");
    b.input(color_in("light", "Light wood", [0.72, 0.50, 0.30]));
    b.input(color_in("dark", "Dark wood", [0.42, 0.24, 0.12]));
    b.input(int_in("planks", "Planks", 6, 1.0, 32.0));
    b.input(float_in("rings", "Grain rings", 30.0, 1.0, 96.0));
    b.input(float_in("roughness", "Roughness", 0.6, 0.0, 1.0));
    let grain = b.node(
        "reetched_wood",
        vec![
            ("rings", input("rings")),
            ("warp", V::f(0.6).into()),
            ("scale", V::i(3).into()),
            ("randomseed", V::i(3).into()),
        ],
        &[],
    );
    let tile = |mode| {
        vec![
            ("columns", V::i(2).into()),
            ("rows", input("planks")),
            ("offset", V::f(0.37).into()),
            ("gap", V::f(0.004).into()),
            ("bevel", V::f(0.004).into()),
            ("variation", V::f(0.0).into()),
            ("mode", V::i(mode).into()),
        ]
    };
    let shape = b.node("reetched_tiles", tile(0), &[]);
    let plank_id = b.node("reetched_tiles", tile(1), &[]);
    let light = b.uniform(input("light"), true);
    let dark = b.uniform(input("dark"), true);
    let wood = b.blend(dark, light, Some(grain), 0, V::f(0.7).into());
    let fibers = noise(&mut b, V::i(3).into(), 40.0, 5, 0, 12);
    let fiber_shade = b.levels(fibers, V::f(0.8).into(), V::f(1.0).into());
    let fibered = b.blend(fiber_shade, wood, None, MULTIPLY, V::f(1.0).into());
    let shade = b.levels(plank_id, V::f(0.75).into(), V::f(1.0).into());
    let shaded = b.blend(shade, fibered, None, MULTIPLY, V::f(1.0).into());
    let seams = b.levels(shape, V::f(0.35).into(), V::f(1.0).into());
    let base = b.blend(seams, shaded, None, MULTIPLY, V::f(1.0).into());
    let metal = b.uniform(V::f(0.0).into(), false);
    let rough = varied(&mut b, "roughness", grain, 0.1);
    let grain_h = b.levels(grain, V::f(0.0).into(), V::f(0.1).into());
    let height = b.blend(grain_h, shape, None, ADD_SUB, V::f(0.5).into());
    let normal = b.normal(height, V::f(2.0).into());
    b.pbr(base, metal, rough, height, normal);
    b.build()
}

fn woven_fabric() -> Graph {
    let mut b = Builder::new("woven_fabric", "Woven Fabric");
    b.input(color_in("color", "Color", [0.55, 0.12, 0.14]));
    b.input(int_in("threads", "Threads", 48, 4.0, 256.0));
    b.input(float_in("roughness", "Roughness", 0.85, 0.0, 1.0));
    let weave = b.node("reetched_weave", vec![("scale", input("threads"))], &[]);
    let fuzz = noise(&mut b, V::i(64).into(), 1.0, 2, 0, 4);
    let color = b.uniform(input("color"), true);
    let shade = b.levels(weave, V::f(0.55).into(), V::f(1.0).into());
    let shaded = b.blend(shade, color, None, MULTIPLY, V::f(1.0).into());
    let fuzz_shade = b.levels(fuzz, V::f(0.9).into(), V::f(1.0).into());
    let base = b.blend(fuzz_shade, shaded, None, MULTIPLY, V::f(1.0).into());
    let metal = b.uniform(V::f(0.0).into(), false);
    let rough = varied(&mut b, "roughness", fuzz, 0.05);
    let normal = b.normal(weave, V::f(1.5).into());
    b.pbr(base, metal, rough, weave, normal);
    b.build()
}

fn concrete() -> Graph {
    let mut b = Builder::new("concrete", "Concrete");
    b.input(color_in("color", "Color", [0.58, 0.57, 0.54]));
    b.input(float_in("pits", "Pits", 0.6, 0.0, 1.0));
    b.input(float_in("roughness", "Roughness", 0.9, 0.0, 1.0));
    let blotches = noise(&mut b, V::i(4).into(), 1.0, 7, 0, 5);
    let fine = noise(&mut b, V::i(64).into(), 1.0, 3, 0, 6);
    let cells = b.node(
        "reetched_cells",
        vec![("scale", V::i(48).into()), ("randomseed", V::i(7).into())],
        &[],
    );
    // Pits: small dark spots where the cell distance is near zero.
    let pits = b.node(
        "levels",
        vec![
            ("levelinlow", V::f(0.0).into()),
            ("levelinhigh", V::f(0.12).into()),
        ],
        &[("input1", cells)],
    );
    let color = b.uniform(input("color"), true);
    let shade = b.levels(blotches, V::f(0.8).into(), V::f(1.0).into());
    let shaded = b.blend(shade, color, None, MULTIPLY, V::f(1.0).into());
    let base = b.blend(pits, shaded, None, MULTIPLY, input("pits"));
    let metal = b.uniform(V::f(0.0).into(), false);
    let rough = varied(&mut b, "roughness", fine, 0.05);
    let surface = b.blend(fine, blotches, None, ADD_SUB, V::f(0.2).into());
    let height = b.blend(pits, surface, None, MULTIPLY, input("pits"));
    let normal = b.normal(height, V::f(1.0).into());
    b.pbr(base, metal, rough, height, normal);
    b.build()
}

fn rust() -> Graph {
    let mut b = Builder::new("rust", "Rust");
    b.input(int_in("scale", "Scale", 6, 1.0, 32.0));
    b.input(float_in("roughness", "Roughness", 0.85, 0.0, 1.0));
    let n = noise(&mut b, input("scale"), 1.0, 7, 0, 8);
    let flakes = noise(&mut b, V::i(32).into(), 1.0, 3, 2, 9);
    let mixed = b.blend(flakes, n, None, ADD_SUB, V::f(0.3).into());
    let base = b.node(
        "gradient",
        vec![(
            "gradientrgba",
            gradient(&[
                (0.0, [0.16, 0.06, 0.03]),
                (0.35, [0.40, 0.15, 0.06]),
                (0.6, [0.62, 0.28, 0.10]),
                (0.85, [0.78, 0.45, 0.20]),
                (1.0, [0.55, 0.36, 0.22]),
            ]),
        )],
        &[("input1", mixed)],
    );
    let metal = b.uniform(V::f(0.0).into(), false);
    let rough = varied(&mut b, "roughness", flakes, 0.1);
    let normal = b.normal(mixed, V::f(1.5).into());
    b.pbr(base, metal, rough, mixed, normal);
    b.build()
}

fn leather() -> Graph {
    let mut b = Builder::new("leather", "Leather");
    b.input(color_in("color", "Color", [0.30, 0.16, 0.08]));
    b.input(int_in("scale", "Grain scale", 40, 4.0, 128.0));
    b.input(float_in("roughness", "Roughness", 0.55, 0.0, 1.0));
    let cells = b.node(
        "reetched_cells",
        vec![
            ("scale", input("scale")),
            ("mode", V::i(1).into()),
            ("randomseed", V::i(10).into()),
        ],
        &[],
    );
    let creases = b.node(
        "levels",
        vec![
            ("levelinlow", V::f(0.0).into()),
            ("levelinhigh", V::f(0.35).into()),
        ],
        &[("input1", cells)],
    );
    let color = b.uniform(input("color"), true);
    let shade = b.levels(creases, V::f(0.6).into(), V::f(1.0).into());
    let base = b.blend(shade, color, None, MULTIPLY, V::f(1.0).into());
    let metal = b.uniform(V::f(0.0).into(), false);
    let rough = varied(&mut b, "roughness", creases, 0.1);
    let normal = b.normal(creases, V::f(1.2).into());
    b.pbr(base, metal, rough, creases, normal);
    b.build()
}

fn bricks() -> Graph {
    let mut b = Builder::new("bricks", "Bricks");
    b.input(color_in("brick", "Brick", [0.56, 0.24, 0.17]));
    b.input(color_in("mortar", "Mortar", [0.70, 0.68, 0.62]));
    b.input(int_in("columns", "Columns", 4, 1.0, 32.0));
    b.input(int_in("rows", "Rows", 12, 1.0, 64.0));
    let tile = |mode| {
        vec![
            ("columns", input("columns")),
            ("rows", input("rows")),
            ("offset", V::f(0.5).into()),
            ("gap", V::f(0.012).into()),
            ("bevel", V::f(0.008).into()),
            ("variation", V::f(0.15).into()),
            ("mode", V::i(mode).into()),
        ]
    };
    let shape = b.node("reetched_tiles", tile(0), &[]);
    let id = b.node("reetched_tiles", tile(1), &[]);
    let grit = noise(&mut b, V::i(32).into(), 1.0, 5, 0, 11);
    let brick = b.uniform(input("brick"), true);
    let mortar = b.uniform(input("mortar"), true);
    let shade = b.levels(id, V::f(0.75).into(), V::f(1.0).into());
    let shaded = b.blend(shade, brick, None, MULTIPLY, V::f(1.0).into());
    let mask = b.node(
        "levels",
        vec![("levelinhigh", V::f(0.05).into())],
        &[("input1", shape)],
    );
    let base0 = b.blend(shaded, mortar, Some(mask), 0, V::f(1.0).into());
    let grit_shade = b.levels(grit, V::f(0.85).into(), V::f(1.0).into());
    let base = b.blend(grit_shade, base0, None, MULTIPLY, V::f(1.0).into());
    let metal = b.uniform(V::f(0.0).into(), false);
    let r = b.uniform(V::f(0.85).into(), false);
    let rough = b.blend(grit, r, None, ADD_SUB, V::f(0.05).into());
    let height = b.blend(grit, shape, None, ADD_SUB, V::f(0.1).into());
    let normal = b.normal(height, V::f(2.0).into());
    b.pbr(base, metal, rough, height, normal);
    b.build()
}

/// The built-in materials, in shelf order.
/// (id, graph builder)
pub type Material = (&'static str, fn() -> Graph);

pub const MATERIALS: &[Material] = &[
    ("brushed_steel", brushed_steel),
    ("rough_plastic", rough_plastic),
    ("wood_planks", wood_planks),
    ("woven_fabric", woven_fabric),
    ("concrete", concrete),
    ("rust", rust),
    ("leather", leather),
    ("bricks", bricks),
];

/// All built-in materials as one package (graph id = material id).
pub fn package() -> Arc<Package> {
    static PKG: std::sync::OnceLock<Arc<Package>> = std::sync::OnceLock::new();
    PKG.get_or_init(|| {
        Arc::new(Package {
            name: "ReEtched library".into(),
            graphs: MATERIALS.iter().map(|(_, f)| f()).collect(),
            ..Default::default()
        })
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Args, render};

    #[test]
    fn every_material_renders_all_outputs_without_warnings() {
        let pkg = package();
        for g in &pkg.graphs {
            let r = render(
                &pkg,
                &g.id,
                &Args {
                    size_log2: [6, 6],
                    seed: 0,
                    ..Default::default()
                },
            );
            assert!(r.warnings.is_empty(), "{}: {:?}", g.id, r.warnings);
            assert_eq!(r.outputs.len(), 5, "{}", g.id);
            for (o, img) in &r.outputs {
                assert_eq!((img.w, img.h), (64, 64), "{} {}", g.id, o.id);
                let first = img.get(0, 0);
                let varies = (0..64 * 64).any(|i| img.get(i % 64, i / 64) != first);
                // Metallic (and plain plastic's color) are uniform by design.
                let flat_ok =
                    o.id == "metallic" || (g.id == "rough_plastic" && o.id == "basecolor");
                if !flat_ok {
                    assert!(varies, "{} {} is flat", g.id, o.id);
                }
            }
        }
    }

    #[test]
    fn inputs_change_the_result() {
        let pkg = package();
        let base = |color: [f32; 3]| {
            let r = render(
                &pkg,
                "rough_plastic",
                &Args {
                    size_log2: [4, 4],
                    values: HashMap::from([("color".to_string(), V::f3(color))]),
                    ..Default::default()
                },
            );
            r.outputs
                .into_iter()
                .find(|(o, _)| o.id == "basecolor")
                .unwrap()
                .1
                .get(3, 3)
        };
        assert_eq!(base([1.0, 0.0, 0.0])[..3], [1.0, 0.0, 0.0]);
        assert_eq!(base([0.0, 0.0, 1.0])[..3], [0.0, 0.0, 1.0]);
    }
}
