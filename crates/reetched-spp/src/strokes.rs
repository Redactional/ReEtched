//! Painter's paint actions, read as brush strokes to replay with ReEtched's
//! brush engine (Painter stores strokes, not the painted pixels).

use reetched_doc::{Channel, NodeId};
use serde_json::Value;

/// One paint, eraser or smudge action on a layer or a mask.
#[derive(Clone, Debug)]
pub struct PaintJob {
    /// Texture set index and node.
    pub set: usize,
    pub node: NodeId,
    /// Paints the node's mask instead of its channels.
    pub mask: bool,
    pub erase: bool,
    /// Layer paint: value per channel (linear color, or [v; 3]).
    pub values: Vec<(Channel, [f32; 3])>,
    /// Mask paint: the grey value.
    pub mask_value: f32,
    /// Most a stroke covers, and coverage per dab.
    pub opacity: f32,
    pub flow: f32,
    /// Brush radius, in Painter's size space (`size_space` 1: fraction of the
    /// normalized scene, which is about 2 units across).
    pub size: f32,
    pub size_space: i64,
    /// Dab distance as a fraction of the diameter.
    pub spacing: f32,
    pub hardness: f32,
    pub pressure_size: bool,
    pub pressure_flow: bool,
    pub strokes: Vec<Stroke>,
}

#[derive(Clone, Debug)]
pub struct Stroke {
    /// Normalized view coordinates (0..1).
    pub points: Vec<[f32; 2]>,
    /// Pressure keys (position along the stroke 0..1, value); empty = full.
    pub pressure: Vec<[f32; 2]>,
    /// Column-major view and projection matrices of the view it was drawn in.
    pub view: [f32; 16],
    pub proj: [f32; 16],
    /// Drawn in the 2D (UV) view.
    pub uv_view: bool,
    /// Symmetry copies: object-space transforms applied to every dab.
    pub mirrors: Vec<[f32; 16]>,
}

fn f(v: &Value, k: &str, default: f32) -> f32 {
    v[k].as_f64().map_or(default, |x| x as f32)
}

fn on(v: &Value, k: &str) -> bool {
    v[k].as_i64().is_some_and(|x| x != 0)
}

fn mat(v: &Value) -> Option<[f32; 16]> {
    let a = v.as_array()?;
    let mut m = [0.0; 16];
    for (o, x) in m.iter_mut().zip(a) {
        *o = x.as_f64()? as f32;
    }
    (a.len() == 16).then_some(m)
}

/// The brush's hardness (the round "shape" brush's tweak), default 0.5.
fn hardness(action: &Value) -> f32 {
    for src in action["sourceTransparent"].as_array().into_iter().flatten() {
        for t in src["procedural"]["tweaks"].as_array().into_iter().flatten() {
            if t["identifier"] == "hardness" {
                return f(t, "value", 0.5);
            }
        }
    }
    0.5
}

/// Reads one paint/eraser action. `channels(mask)` maps a channel bit mask.
pub fn read(
    action: &Value,
    set: usize,
    node: NodeId,
    mask: bool,
    channels: &dyn Fn(u64) -> Vec<Channel>,
) -> PaintJob {
    let brush = &action["brush"];
    let emitter = &brush["stampsEmitter"];
    let erase = action["@type"] == "DataActionEraser";
    let mut values = Vec::new();
    let mut mask_value = 1.0;
    for src in action["sourceColor"].as_array().into_iter().flatten() {
        if src["@type"] != "DataSourceUniform" {
            continue;
        }
        let color = src["color"]
            .as_array()
            .map(|a| {
                let g = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                [g(0), g(1), g(2)]
            })
            .unwrap_or([1.0; 3]);
        mask_value = color[0];
        for c in channels(src["channelTypes"].as_u64().unwrap_or(0)) {
            let v = match c {
                c if c.is_color() || c.is_vector() => color,
                _ => [color[0]; 3],
            };
            values.push((c, v));
        }
    }
    let strokes = action["strokes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let points: Vec<[f32; 2]> = s["points"]
                .as_array()?
                .iter()
                .filter_map(|k| {
                    let p = k["position"].as_array()?;
                    Some([p.first()?.as_f64()? as f32, p.get(1)?.as_f64()? as f32])
                })
                .collect();
            let pressure = s["pressure"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|k| [f(k, "x", 0.0), f(k, "value", 1.0)])
                .collect();
            let count = s["duplicateCount"].as_i64().unwrap_or(0);
            let mirrors = (count > 0)
                .then(|| mat(&s["duplicateMatrix"]))
                .flatten()
                .into_iter()
                .collect();
            let (view, proj) = (mat(&s["viewMatrix"])?, mat(&s["projectionMatrix"])?);
            // 2D-view strokes store no camera; their points are UVs.
            let uv_view = s["viewType"].as_i64() == Some(0);
            Some(Stroke {
                points,
                pressure,
                view,
                proj,
                uv_view,
                mirrors,
            })
        })
        .collect();
    PaintJob {
        set,
        node,
        mask,
        erase,
        values,
        mask_value,
        opacity: f(action, "opacity", 1.0),
        flow: f(brush, "flow", 1.0),
        size: f(brush, "brushSize", 0.05),
        size_space: brush["brushSizeSpace"].as_i64().unwrap_or(1),
        spacing: f(emitter, "spacing", 0.1),
        hardness: hardness(action),
        pressure_size: on(emitter, "pressureSizeEnabled"),
        pressure_flow: on(emitter, "pressureFlowEnabled"),
        strokes,
    }
}

/// A Polygon Fill action ("decal"): clicks and rectangles that fill faces.
#[derive(Clone, Debug)]
pub struct DecalJob {
    pub set: usize,
    pub node: NodeId,
    pub mask: bool,
    pub hits: Vec<DecalHit>,
}

#[derive(Clone, Debug)]
pub struct DecalHit {
    /// A click (both corners equal) or a rectangle, in normalized view
    /// coordinates (y down); UVs for 2D-view hits.
    pub corners: [[f32; 2]; 2],
    pub rect: bool,
    /// Column-major inverse view-projection of the 3D view (None: 2D view).
    pub inv_view_proj: Option<[f32; 16]>,
    /// 0 triangle, 1 polygon, 2 mesh, 3 UV island.
    pub granularity: i64,
    /// Fill value (mask grey in x).
    pub color: [f32; 3],
    pub mirror: Option<[f32; 16]>,
}

fn pair(v: &Value, k: &str) -> Option<[f32; 2]> {
    let a = v[k].as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
}

/// Reads one decal action.
pub fn read_decal(action: &Value, set: usize, node: NodeId, mask: bool) -> DecalJob {
    let hits = action["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let rect = h["@type"] == "DataDecalHitRect";
            let corners = if rect {
                [pair(h, "position0")?, pair(h, "position1")?]
            } else {
                let p = pair(h, "position")?;
                [p, p]
            };
            let color = h["color"]
                .as_array()
                .map(|a| {
                    let g = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(1.0) as f32;
                    [g(0), g(1), g(2)]
                })
                .unwrap_or([1.0; 3]);
            let inv = mat(&h["projViewInvMatrix"]).filter(|m| m.iter().any(|x| *x != 0.0));
            Some(DecalHit {
                corners,
                rect,
                inv_view_proj: if h["viewType"].as_i64() == Some(0) {
                    None
                } else {
                    inv
                },
                granularity: h["granularity"].as_i64().unwrap_or(0),
                color,
                mirror: (h["duplicateCount"].as_i64().unwrap_or(0) > 0)
                    .then(|| mat(&h["duplicateMatrix"]))
                    .flatten(),
            })
        })
        .collect();
    DecalJob {
        set,
        node,
        mask,
        hits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strokes_and_fills_read_from_painter_objects() {
        let zero = [0.0; 16];
        let action = json!({
            "@type": "DataActionPaint",
            "opacity": 0.5,
            "brush": {"brushSize": 0.2, "brushSizeSpace": 1, "flow": 0.8,
                      "stampsEmitter": {"spacing": 0.1, "pressureSizeEnabled": 255}},
            "sourceColor": [{"@type": "DataSourceUniform", "channelTypes": 1, "color": [1.0, 0.0, 0.0]}],
            "sourceTransparent": [{"procedural": {"tweaks": [{"identifier": "hardness", "value": 0.9}]}}],
            "strokes": [{
                "points": [{"position": [0.1, 0.2]}, {"position": [0.3, 0.4]}],
                "pressure": [], "viewType": 0, "viewMatrix": zero, "projectionMatrix": zero,
                "duplicateCount": 1,
                "duplicateMatrix": [-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
            }]
        });
        let job = read(&action, 0, 7, false, &|_| vec![Channel::BaseColor]);
        assert_eq!((job.node, job.opacity, job.hardness), (7, 0.5, 0.9));
        assert_eq!(job.values, vec![(Channel::BaseColor, [1.0, 0.0, 0.0])]);
        let s = &job.strokes[0];
        assert!(s.uv_view && s.points == vec![[0.1, 0.2], [0.3, 0.4]]);
        assert_eq!(s.mirrors.len(), 1);

        let decal = json!({"hits": [
            {"@type": "DataDecalHitRect", "position0": [0.1, 0.1], "position1": [0.2, 0.3],
             "viewType": 0, "projViewInvMatrix": zero, "granularity": 3, "color": [1.0, 1.0, 1.0]},
            {"@type": "DataDecalHitPunctual", "position": [0.5, 0.5], "viewType": 1,
             "projViewInvMatrix": [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
             "granularity": 0, "color": [0.0, 0.0, 0.0]}
        ]});
        let d = read_decal(&decal, 1, 9, true);
        assert_eq!(d.hits.len(), 2);
        assert!(d.hits[0].rect && d.hits[0].inv_view_proj.is_none() && d.hits[0].granularity == 3);
        assert!(
            !d.hits[1].rect && d.hits[1].inv_view_proj.is_some() && d.hits[1].color == [0.0; 3]
        );
    }
}
