//! Layered PSD writer (8-bit RGB, PackBits-compressed), for Painter-style
//! "PSD export": one file per channel, layers and folders kept.

use crate::BlendMode;
use anyhow::{Context, Result};
use std::path::Path;

/// One PSD layer or group, children top first.
pub enum PsdNode {
    Layer {
        name: String,
        visible: bool,
        blend: BlendMode,
        /// 0..=1
        opacity: f32,
        /// width × height RGBA8, row 0 at the top.
        pixels: Vec<[u8; 4]>,
        mask: Option<Vec<u8>>,
    },
    Group {
        name: String,
        visible: bool,
        blend: BlendMode,
        opacity: f32,
        mask: Option<Vec<u8>>,
        children: Vec<PsdNode>,
    },
}

/// Photoshop's key for a blend mode; None where Photoshop has no equivalent.
pub fn psd_blend(mode: BlendMode) -> Option<&'static [u8; 4]> {
    use BlendMode::*;
    Some(match mode {
        Normal | Replace => b"norm",
        Passthrough => b"pass",
        Multiply => b"mul ",
        Divide => b"fdiv",
        Darken => b"dark",
        Lighten => b"lite",
        LinearDodge => b"lddg",
        Subtract => b"fsub",
        Difference => b"diff",
        Exclusion => b"smud",
        Overlay => b"over",
        Screen => b"scrn",
        LinearBurn => b"lbrn",
        ColorBurn => b"idiv",
        ColorDodge => b"div ",
        SoftLight => b"sLit",
        HardLight => b"hLit",
        VividLight => b"vLit",
        LinearLight => b"lLit",
        PinLight => b"pLit",
        Saturation => b"sat ",
        Color => b"colr",
        Value => b"lum ",
        _ => return None,
    })
}

/// PackBits (Apple RLE) for one scanline.
fn packbits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        // A run of the same byte?
        let mut run = 1;
        while i + run < row.len() && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((1i16 - run as i16) as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        // Literal bytes until the next run of 3+ (or 128 bytes).
        let start = i;
        while i < row.len() && i - start < 128 {
            if i + 2 < row.len() && row[i] == row[i + 1] && row[i] == row[i + 2] {
                break;
            }
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&row[start..i]);
    }
    out
}

/// RLE-compressed planes: per-row byte counts for every plane, then the data.
fn rle_planes(planes: &[Vec<u8>], width: usize) -> (Vec<u8>, Vec<u8>) {
    let mut counts = Vec::new();
    let mut data = Vec::new();
    for plane in planes {
        for row in plane.chunks(width) {
            let packed = packbits(row);
            counts.extend_from_slice(&(packed.len() as u16).to_be_bytes());
            data.extend_from_slice(&packed);
        }
    }
    (counts, data)
}

struct Record {
    name: String,
    visible: bool,
    blend: [u8; 4],
    opacity: u8,
    /// (channel id, plane)
    channels: Vec<(i16, Vec<u8>)>,
    has_mask: bool,
    /// lsct divider type: 0 none, 1 open group, 3 group end.
    section: u32,
}

fn opacity8(o: f32) -> u8 {
    (o.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Flattens the tree into PSD order (bottom first), with group markers.
fn flatten(nodes: &[PsdNode], out: &mut Vec<Record>, log: &mut Vec<String>) {
    let key = |b: BlendMode, name: &str, log: &mut Vec<String>| {
        *psd_blend(b).unwrap_or_else(|| {
            log.push(format!(
                "{name}: {b:?} has no Photoshop equivalent; written as Normal"
            ));
            b"norm"
        })
    };
    for node in nodes.iter().rev() {
        match node {
            PsdNode::Layer {
                name,
                visible,
                blend,
                opacity,
                pixels,
                mask,
            } => {
                let plane = |k: usize| pixels.iter().map(|p| p[k]).collect::<Vec<u8>>();
                let mut channels =
                    vec![(-1, plane(3)), (0, plane(0)), (1, plane(1)), (2, plane(2))];
                if let Some(m) = mask {
                    channels.push((-2, m.clone()));
                }
                out.push(Record {
                    name: name.clone(),
                    visible: *visible,
                    blend: key(*blend, name, log),
                    opacity: opacity8(*opacity),
                    channels,
                    has_mask: mask.is_some(),
                    section: 0,
                });
            }
            PsdNode::Group {
                name,
                visible,
                blend,
                opacity,
                mask,
                children,
            } => {
                out.push(Record {
                    name: "</Layer group>".into(),
                    visible: true,
                    blend: *b"norm",
                    opacity: 255,
                    channels: vec![(-1, vec![]), (0, vec![]), (1, vec![]), (2, vec![])],
                    has_mask: false,
                    section: 3,
                });
                flatten(children, out, log);
                let mut channels = vec![(-1, vec![]), (0, vec![]), (1, vec![]), (2, vec![])];
                if let Some(m) = mask {
                    channels.push((-2, m.clone()));
                }
                out.push(Record {
                    name: name.clone(),
                    visible: *visible,
                    blend: key(*blend, name, log),
                    opacity: opacity8(*opacity),
                    channels,
                    has_mask: mask.is_some(),
                    section: 1,
                });
            }
        }
    }
}

fn pascal_padded(name: &str, multiple: usize) -> Vec<u8> {
    let bytes: Vec<u8> = name
        .chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .take(255)
        .collect();
    let mut out = vec![bytes.len() as u8];
    out.extend_from_slice(&bytes);
    while out.len() % multiple != 0 {
        out.push(0);
    }
    out
}

/// Writes `nodes` (top first) over the `merged` composite. Returns notes on
/// anything that couldn't be represented (e.g. blend modes).
pub fn write_psd(
    path: &Path,
    width: u32,
    height: u32,
    merged: &[[u8; 4]],
    nodes: &[PsdNode],
) -> Result<Vec<String>> {
    let (w, h) = (width as usize, height as usize);
    let mut log = Vec::new();
    let mut records = Vec::new();
    flatten(nodes, &mut records, &mut log);

    let mut layer_info = Vec::new();
    layer_info.extend_from_slice(&(records.len() as i16).to_be_bytes());
    let mut image_data = Vec::new();
    for r in &records {
        // Group markers have an empty rect; layers cover the canvas.
        let empty = r.section != 0;
        let (bottom, right) = if empty { (0, 0) } else { (height, width) };
        for v in [0u32, 0, bottom, right] {
            layer_info.extend_from_slice(&v.to_be_bytes());
        }
        layer_info.extend_from_slice(&(r.channels.len() as u16).to_be_bytes());
        let mut channel_data = Vec::new();
        for (id, plane) in &r.channels {
            let mut d = Vec::new();
            if plane.is_empty() {
                d.extend_from_slice(&0u16.to_be_bytes()); // raw, no pixels
            } else {
                d.extend_from_slice(&1u16.to_be_bytes());
                let (counts, data) = rle_planes(std::slice::from_ref(plane), w);
                d.extend_from_slice(&counts);
                d.extend_from_slice(&data);
            }
            layer_info.extend_from_slice(&id.to_be_bytes());
            layer_info.extend_from_slice(&(d.len() as u32).to_be_bytes());
            channel_data.push(d);
        }
        layer_info.extend_from_slice(b"8BIM");
        layer_info.extend_from_slice(&r.blend);
        layer_info.push(r.opacity);
        layer_info.push(0); // clipping
        let mut flags = if r.visible { 0 } else { 2 };
        if r.section != 0 {
            flags |= 0x18; // bit 4 meaningful, pixel data irrelevant to appearance
        }
        layer_info.push(flags);
        layer_info.push(0);

        let mut extra = Vec::new();
        if r.has_mask {
            extra.extend_from_slice(&20u32.to_be_bytes());
            for v in [0u32, 0, height, width] {
                extra.extend_from_slice(&v.to_be_bytes());
            }
            extra.extend_from_slice(&[0, 0, 0, 0]); // default color black, flags, padding
        } else {
            extra.extend_from_slice(&0u32.to_be_bytes());
        }
        extra.extend_from_slice(&0u32.to_be_bytes()); // blending ranges
        extra.extend_from_slice(&pascal_padded(&r.name, 4));
        if r.section != 0 {
            extra.extend_from_slice(b"8BIMlsct");
            extra.extend_from_slice(&12u32.to_be_bytes());
            extra.extend_from_slice(&r.section.to_be_bytes());
            extra.extend_from_slice(b"8BIM");
            extra.extend_from_slice(if r.section == 1 { &r.blend } else { b"norm" });
        }
        layer_info.extend_from_slice(&(extra.len() as u32).to_be_bytes());
        layer_info.extend_from_slice(&extra);
        for d in channel_data {
            image_data.extend_from_slice(&d);
        }
    }
    layer_info.extend_from_slice(&image_data);
    if layer_info.len() % 2 == 1 {
        layer_info.push(0);
    }

    let mut f = Vec::new();
    f.extend_from_slice(b"8BPS");
    f.extend_from_slice(&1u16.to_be_bytes());
    f.extend_from_slice(&[0; 6]);
    f.extend_from_slice(&4u16.to_be_bytes()); // RGBA
    f.extend_from_slice(&height.to_be_bytes());
    f.extend_from_slice(&width.to_be_bytes());
    f.extend_from_slice(&8u16.to_be_bytes());
    f.extend_from_slice(&3u16.to_be_bytes()); // RGB
    f.extend_from_slice(&0u32.to_be_bytes()); // color mode data
    f.extend_from_slice(&0u32.to_be_bytes()); // image resources
    // Layer and mask information.
    let section_len = 4 + layer_info.len() + 4;
    f.extend_from_slice(&(section_len as u32).to_be_bytes());
    f.extend_from_slice(&(layer_info.len() as u32).to_be_bytes());
    f.extend_from_slice(&layer_info);
    f.extend_from_slice(&0u32.to_be_bytes()); // global layer mask
    // Merged image.
    let plane = |k: usize| merged.iter().map(|p| p[k]).collect::<Vec<u8>>();
    let (counts, data) = rle_planes(&[plane(0), plane(1), plane(2), plane(3)], w);
    debug_assert_eq!(counts.len(), h * 4 * 2);
    f.extend_from_slice(&1u16.to_be_bytes());
    f.extend_from_slice(&counts);
    f.extend_from_slice(&data);
    std::fs::write(path, f).with_context(|| format!("could not write {}", path.display()))?;
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits_round_trips() {
        fn unpack(d: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            let mut i = 0;
            while i < d.len() {
                let n = d[i] as i8;
                i += 1;
                if n >= 0 {
                    out.extend_from_slice(&d[i..i + n as usize + 1]);
                    i += n as usize + 1;
                } else if n != -128 {
                    out.extend(std::iter::repeat_n(d[i], (1 - n as isize) as usize));
                    i += 1;
                }
            }
            out
        }
        let row: Vec<u8> = (0..300)
            .map(|i| {
                if (100..250).contains(&i) {
                    7
                } else {
                    (i % 13) as u8
                }
            })
            .collect();
        assert_eq!(unpack(&packbits(&row)), row);
        assert!(packbits(&[5; 200]).len() < 10);
    }

    #[test]
    fn photoshop_readers_see_our_layers() {
        let (w, h) = (4u32, 2u32);
        let n = (w * h) as usize;
        let red = vec![[255, 0, 0, 255]; n];
        let half_blue: Vec<[u8; 4]> = (0..n)
            .map(|i| {
                if i % 4 < 2 {
                    [0, 0, 255, 255]
                } else {
                    [0, 0, 0, 0]
                }
            })
            .collect();
        let nodes = vec![
            PsdNode::Group {
                name: "Folder".into(),
                visible: true,
                blend: BlendMode::Passthrough,
                opacity: 1.0,
                mask: None,
                children: vec![PsdNode::Layer {
                    name: "Blue".into(),
                    visible: true,
                    blend: BlendMode::Multiply,
                    opacity: 0.5,
                    pixels: half_blue,
                    mask: Some(vec![255; n]),
                }],
            },
            PsdNode::Layer {
                name: "Red".into(),
                visible: false,
                blend: BlendMode::NormalMapCombine,
                opacity: 1.0,
                pixels: red.clone(),
                mask: None,
            },
        ];
        let dir = std::env::temp_dir().join("reetched-psd-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.psd");
        let log = write_psd(&path, w, h, &red, &nodes).unwrap();
        assert_eq!(log.len(), 1, "normal-map blend has no PSD equivalent");
        let bytes = std::fs::read(&path).unwrap();
        let psd = psd::Psd::from_bytes(&bytes).unwrap();
        assert_eq!((psd.width(), psd.height()), (w, h));
        let names: Vec<&str> = psd.layers().iter().map(|l| l.name()).collect();
        assert!(
            names.contains(&"Blue") && names.contains(&"Red"),
            "{names:?}"
        );
        let blue = psd.layer_by_name("Blue").unwrap();
        assert_eq!(blue.opacity(), 128);
        assert_eq!(format!("{:?}", blue.blend_mode()), "Multiply");
        assert_eq!(&blue.rgba()[..4], &[0, 0, 255, 255]);
        assert_eq!(blue.rgba()[11], 0, "third pixel transparent");
        // Flag bit 1 marks hidden layers in Photoshop (psd-tools agrees); the `psd`
        // crate reads it as "visible", so only check that the two layers differ.
        assert_ne!(psd.layer_by_name("Red").unwrap().visible(), blue.visible());
        assert_eq!(psd.groups().len(), 1);
        assert_eq!(&psd.rgba()[..4], &[255, 0, 0, 255]);
    }
}
