//! Built-in smart materials: layer stacks driven by mesh-map generators.

use crate::preview::Look;
use reetched_doc::{
    Document, Fill, Generator, GeneratorKind, Mask, MaskBackground, MaskBlend, MaskEffect,
    MaskEffectKind, Node, NodeKind,
};

pub struct SmartPreset {
    pub name: &'static str,
    /// Thumbnail look (the material you see most of).
    pub look: Look,
    pub build: fn(&mut Document) -> Node,
}

/// sRGB bytes → linear.
fn lin(c: [u8; 3]) -> [f32; 3] {
    let r = egui::Rgba::from(egui::Color32::from_rgb(c[0], c[1], c[2]));
    [r.r(), r.g(), r.b()]
}

fn generator(
    kind: GeneratorKind,
    blend: MaskBlend,
    tweak: impl FnOnce(&mut Generator),
) -> MaskEffect {
    let mut g = Generator::new(kind);
    tweak(&mut g);
    MaskEffect {
        blend,
        ..MaskEffect::new(MaskEffectKind::Generator(g))
    }
}

/// A fill layer with a mask made of `effects` over `background`.
fn masked(
    doc: &mut Document,
    name: &str,
    fill: Fill,
    background: MaskBackground,
    effects: Vec<MaskEffect>,
) -> Node {
    let mut node = doc.new_fill(name.into(), fill);
    let mut mask = Mask::new(background);
    mask.effects = effects;
    node.mask = Some(mask);
    node
}

fn folder(doc: &mut Document, name: &str, children: Vec<Node>) -> Node {
    let mut f = doc.new_folder(name.into());
    if let NodeKind::Folder { children: c } = &mut f.kind {
        *c = children;
    }
    f
}

pub const BUILTIN: &[SmartPreset] = &[
    SmartPreset {
        name: "Painted Steel Worn",
        look: Look {
            base: [0.31, 0.012, 0.009],
            metallic: 0.0,
            roughness: 0.45,
        },
        build: |doc| {
            let paint = masked(
                doc,
                "Paint",
                Fill::new(lin([150, 28, 22]), 0.0, 0.45),
                MaskBackground::White,
                vec![generator(
                    GeneratorKind::EdgeWear,
                    MaskBlend::Subtract,
                    |_| {},
                )],
            );
            let steel = doc.new_fill("Steel".into(), Fill::new(lin([205, 205, 208]), 1.0, 0.3));
            folder(doc, "Painted Steel Worn", vec![paint, steel])
        },
    },
    SmartPreset {
        name: "Rusty Iron",
        // Thumbnail colors are linear (sRGB values in the fills below).
        look: Look {
            base: [0.187, 0.042, 0.014],
            metallic: 0.0,
            roughness: 0.85,
        },
        build: |doc| {
            let rust = masked(
                doc,
                "Rust",
                Fill::new(lin([120, 58, 32]), 0.0, 0.9),
                MaskBackground::Black,
                vec![
                    generator(GeneratorKind::AmbientOcclusion, MaskBlend::Normal, |g| {
                        g.level = 0.2
                    }),
                    generator(GeneratorKind::Noise, MaskBlend::Max, |g| {
                        g.level = 0.6;
                        g.noise_scale = 4.0;
                    }),
                ],
            );
            let iron = doc.new_fill("Iron".into(), Fill::new(lin([196, 198, 200]), 1.0, 0.45));
            folder(doc, "Rusty Iron", vec![rust, iron])
        },
    },
    SmartPreset {
        name: "Dusty Plastic",
        look: Look {
            base: [0.045, 0.051, 0.061],
            metallic: 0.0,
            roughness: 0.4,
        },
        build: |doc| {
            let dust = masked(
                doc,
                "Dust",
                Fill::new(lin([190, 184, 172]), 0.0, 0.95),
                MaskBackground::Black,
                vec![
                    generator(GeneratorKind::PositionGradient, MaskBlend::Normal, |g| {
                        g.level = 0.6;
                        g.noise = 0.3;
                    }),
                    generator(GeneratorKind::Cavity, MaskBlend::Max, |_| {}),
                ],
            );
            let plastic = doc.new_fill("Plastic".into(), Fill::new(lin([60, 64, 70]), 0.0, 0.4));
            folder(doc, "Dusty Plastic", vec![dust, plastic])
        },
    },
    SmartPreset {
        name: "Gold Grime",
        look: Look {
            base: [1.0, 0.761, 0.328],
            metallic: 1.0,
            roughness: 0.22,
        },
        build: |doc| {
            let grime = masked(
                doc,
                "Grime",
                Fill::new(lin([48, 40, 30]), 0.0, 0.85),
                MaskBackground::Black,
                vec![
                    generator(GeneratorKind::Cavity, MaskBlend::Normal, |g| g.level = 0.15),
                    generator(GeneratorKind::AmbientOcclusion, MaskBlend::Max, |g| {
                        g.level = 0.35
                    }),
                ],
            );
            let gold = doc.new_fill("Gold".into(), Fill::new(lin([255, 226, 155]), 1.0, 0.22));
            folder(doc, "Gold Grime", vec![grime, gold])
        },
    },
];
