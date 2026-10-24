//! LUT file readers for FileTransform: .cube (Iridas and Resolve), .spi1d, .spi3d, .csp and
//! CLF/CTF. Each returns the forward op list; 3D LUTs come back with trilinear interpolation.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail, ensure};

use crate::ops::{CspCurve, GammaKind, LogParams, Lut1D, Lut3D, Op};

pub(crate) fn load(path: &str, bytes: &[u8]) -> Result<Vec<Op>> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let text = std::str::from_utf8(bytes).with_context(|| format!("{path}: not UTF-8 text"))?;
    match ext.as_str() {
        "cube" => parse_cube(text),
        "spi1d" => parse_spi1d(text),
        "spi3d" => parse_spi3d(text),
        "csp" => parse_csp(text),
        "clf" | "ctf" => parse_clf(text),
        _ => bail!("unsupported LUT file format '.{ext}'"),
    }
    .with_context(|| format!("reading LUT file '{path}'"))
}

/// Maps [min, max] to [0, 1] per channel without clamping (the LUT clamps).
fn domain_op(min: [f64; 3], max: [f64; 3]) -> Option<Op> {
    if min == [0.0; 3] && max == [1.0; 3] {
        return None;
    }
    let s = [0, 1, 2].map(|i| 1.0 / (max[i] - min[i]));
    Some(Op::scale_offset(s, [0, 1, 2].map(|i| -min[i] * s[i])))
}

fn lut1d_op(rows: &[[f32; 3]]) -> Op {
    let ch = [0, 1, 2].map(|c| rows.iter().map(|r| r[c]).collect());
    Op::Lut1D {
        lut: Arc::new(Lut1D::new(ch)),
        inverse: false,
    }
}

fn lut3d_op(n: usize, data: Vec<[f32; 3]>) -> Result<Op> {
    ensure!(
        n >= 2 && data.len() == n * n * n,
        "3D LUT expects {} entries, found {}",
        n * n * n,
        data.len()
    );
    Ok(Op::Lut3D {
        lut: Arc::new(Lut3D { n, data }),
        tetra: false,
        inverse: false,
    })
}

fn num<T: std::str::FromStr>(s: &str) -> Result<T> {
    s.parse().map_err(|_| anyhow!("invalid number '{s}'"))
}

fn parse_cube(text: &str) -> Result<Vec<Op>> {
    let (mut n1, mut n3) = (0usize, 0usize);
    let (mut dmin, mut dmax) = ([0.0f64; 3], [1.0f64; 3]);
    let (mut r1, mut r3) = (None, None);
    let mut rows = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some(&key) = t.first() else { continue };
        if key.starts_with('#') {
            continue;
        }
        let range = |t: &[&str]| -> Result<(f64, f64)> { Ok((num(t[1])?, num(t[2])?)) };
        let vec3 = |t: &[&str]| -> Result<[f64; 3]> { Ok([num(t[1])?, num(t[2])?, num(t[3])?]) };
        match key {
            "TITLE" | "LUT_IN_VIDEO_RANGE" | "LUT_OUT_VIDEO_RANGE" => {}
            "LUT_1D_SIZE" => n1 = num(t[1])?,
            "LUT_3D_SIZE" => n3 = num(t[1])?,
            "DOMAIN_MIN" => dmin = vec3(&t)?,
            "DOMAIN_MAX" => dmax = vec3(&t)?,
            "LUT_1D_INPUT_RANGE" => r1 = Some(range(&t)?),
            "LUT_3D_INPUT_RANGE" => r3 = Some(range(&t)?),
            _ => {
                ensure!(t.len() == 3, "expected 3 values per row, got '{line}'");
                rows.push([num(t[0])?, num(t[1])?, num(t[2])?]);
            }
        }
    }
    ensure!(n1 + n3 > 0, "missing LUT_1D_SIZE / LUT_3D_SIZE");
    ensure!(
        rows.len() == n1 + n3 * n3 * n3,
        "expected {} rows, found {}",
        n1 + n3 * n3 * n3,
        rows.len()
    );
    let mut ops = Vec::new();
    let domain = |r: Option<(f64, f64)>| r.map_or((dmin, dmax), |(a, b)| ([a; 3], [b; 3]));
    if n1 > 0 {
        let (a, b) = domain(r1);
        ops.extend(domain_op(a, b));
        ops.push(lut1d_op(&rows[..n1]));
    }
    if n3 > 0 {
        let (a, b) = if n1 > 0 {
            r3.map_or(([0.; 3], [1.; 3]), |(a, b)| ([a; 3], [b; 3]))
        } else {
            domain(r3)
        };
        ops.extend(domain_op(a, b));
        ops.push(lut3d_op(n3, rows[n1..].to_vec())?);
    }
    Ok(ops)
}

fn parse_spi1d(text: &str) -> Result<Vec<Op>> {
    let (mut from, mut len, mut comps) = ((0.0, 1.0), 0usize, 1usize);
    let mut rows = Vec::new();
    let mut in_data = false;
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some(&key) = t.first() else { continue };
        if in_data {
            if key == "}" {
                break;
            }
            let v: Vec<f32> = t.iter().map(|s| num(s)).collect::<Result<_>>()?;
            rows.push(match comps {
                1 => [v[0]; 3],
                2 => [v[0], v[1], 0.0],
                _ => [v[0], v[1], v[2]],
            });
            continue;
        }
        match key {
            "Version" => {}
            "From" => from = (num(t[1])?, num(t[2])?),
            "Length" => len = num(t[1])?,
            "Components" => comps = num(t[1])?,
            "{" => in_data = true,
            _ => bail!("unexpected spi1d line '{line}'"),
        }
    }
    ensure!(
        len >= 2 && rows.len() == len,
        "spi1d expects {len} values, found {}",
        rows.len()
    );
    let mut ops = Vec::new();
    ops.extend(domain_op([from.0; 3], [from.1; 3]));
    ops.push(lut1d_op(&rows));
    Ok(ops)
}

fn parse_spi3d(text: &str) -> Result<Vec<Op>> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().unwrap_or("");
    ensure!(header.starts_with("SPILUT"), "missing SPILUT header");
    lines.next(); // "3 3"
    let size: Vec<usize> = lines
        .next()
        .unwrap_or("")
        .split_whitespace()
        .map(num)
        .collect::<Result<_>>()?;
    ensure!(
        size.len() == 3 && size[0] == size[1] && size[1] == size[2],
        "spi3d needs a cubic size"
    );
    let n = size[0];
    let mut data = vec![[0.0f32; 3]; n * n * n];
    for line in lines {
        let t: Vec<&str> = line.split_whitespace().collect();
        ensure!(t.len() == 6, "bad spi3d entry '{line}'");
        let (r, g, b): (usize, usize, usize) = (num(t[0])?, num(t[1])?, num(t[2])?);
        ensure!(r < n && g < n && b < n, "spi3d index out of range '{line}'");
        data[r + n * (g + n * b)] = [num(t[3])?, num(t[4])?, num(t[5])?];
    }
    Ok(vec![lut3d_op(n, data)?])
}

fn parse_csp(text: &str) -> Result<Vec<Op>> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    ensure!(
        lines.next() == Some("CSPLUTV100"),
        "missing CSPLUTV100 header"
    );
    let kind = lines.next().unwrap_or("");
    let mut tokens: Vec<&str> = Vec::new();
    let mut in_meta = false;
    for l in lines {
        match l {
            "BEGIN METADATA" => in_meta = true,
            "END METADATA" => in_meta = false,
            _ if in_meta => {}
            _ => tokens.extend(l.split_whitespace()),
        }
    }
    let mut it = tokens.into_iter();
    let mut next = || -> Result<&str> { it.next().ok_or_else(|| anyhow!("truncated csp file")) };
    let mut ops = Vec::new();
    let mut prelut: Vec<(Vec<f32>, Vec<f32>)> = Vec::new();
    for _ in 0..3 {
        let n: usize = num(next()?)?;
        let x = (0..n).map(|_| num(next()?)).collect::<Result<Vec<f32>>>()?;
        let y = (0..n).map(|_| num(next()?)).collect::<Result<Vec<f32>>>()?;
        ensure!(n >= 2, "csp prelut needs at least 2 points");
        prelut.push((x, y));
    }
    if prelut.iter().any(|(x, y)| x != y) {
        let ch = [0, 1, 2].map(|c| CspCurve::new(&prelut[c].0, &prelut[c].1));
        ops.push(Op::Prelut {
            ch: Arc::new(ch),
            inverse: false,
        });
    }
    match kind {
        "1D" => {
            let n: usize = num(next()?)?;
            let rows = (0..n)
                .map(|_| Ok([num(next()?)?, num(next()?)?, num(next()?)?]))
                .collect::<Result<Vec<_>>>()?;
            ops.push(lut1d_op(&rows));
        }
        "3D" => {
            let n: usize = num(next()?)?;
            let (_, _): (usize, usize) = (num(next()?)?, num(next()?)?);
            let data = (0..n * n * n)
                .map(|_| Ok([num(next()?)?, num(next()?)?, num(next()?)?]))
                .collect::<Result<_>>()?;
            ops.push(lut3d_op(n, data)?);
        }
        _ => bail!("csp type must be 1D or 3D, got '{kind}'"),
    }
    Ok(ops)
}

fn bit_depth_scale(bd: Option<&str>) -> Result<f64> {
    Ok(match bd.unwrap_or("32f") {
        "8i" => 255.0,
        "10i" => 1023.0,
        "12i" => 4095.0,
        "16i" => 65535.0,
        "16f" | "32f" => 1.0,
        other => bail!("unknown bit depth '{other}'"),
    })
}

fn floats(node: roxmltree::Node) -> Result<Vec<f64>> {
    node.text()
        .unwrap_or("")
        .split_whitespace()
        .map(num)
        .collect()
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| c.tag_name().name() == name)
}

fn array(node: roxmltree::Node) -> Result<(Vec<usize>, Vec<f64>)> {
    let a = child(node, "Array")
        .ok_or_else(|| anyhow!("<{}> without <Array>", node.tag_name().name()))?;
    let dim = a
        .attribute("dim")
        .unwrap_or("")
        .split_whitespace()
        .map(num)
        .collect::<Result<Vec<usize>>>()?;
    Ok((dim, floats(a)?))
}

fn parse_clf(text: &str) -> Result<Vec<Op>> {
    let doc = roxmltree::Document::parse(text)?;
    let root = doc.root_element();
    ensure!(
        root.tag_name().name() == "ProcessList",
        "missing <ProcessList>"
    );
    let mut ops = Vec::new();
    for node in root.children().filter(|n| n.is_element()) {
        let name = node.tag_name().name();
        let in_s = bit_depth_scale(node.attribute("inBitDepth"))?;
        let out_s = bit_depth_scale(node.attribute("outBitDepth"))?;
        match name {
            "Description" | "InputDescriptor" | "OutputDescriptor" | "Info" => {}
            "Matrix" => {
                let (dim, v) = array(node)?;
                let cols = *dim.get(1).ok_or_else(|| anyhow!("Matrix dim"))?;
                let rows = dim[0];
                ensure!(v.len() == rows * cols && rows >= 3, "Matrix size mismatch");
                let mut m = [[0.0; 3]; 3];
                let mut off = [0.0; 3];
                for r in 0..3 {
                    for c in 0..3 {
                        m[r][c] = v[r * cols + c] * in_s / out_s;
                    }
                    if cols == 4 && rows == 3 || cols == 5 {
                        off[r] = v[r * cols + cols - 1] / out_s;
                    }
                }
                ops.push(Op::Matrix { m, off });
            }
            "LUT1D" => {
                ensure!(
                    node.attribute("halfDomain") != Some("true"),
                    "half-domain LUT1D is not supported"
                );
                let (dim, v) = array(node)?;
                let c = *dim.get(1).unwrap_or(&1);
                let rows: Vec<[f32; 3]> = v
                    .chunks(c)
                    .map(|r| {
                        let g = |i: usize| (r[if c == 1 { 0 } else { i }] / out_s) as f32;
                        [g(0), g(1), g(2)]
                    })
                    .collect();
                ensure!(rows.len() >= 2, "LUT1D needs at least 2 entries");
                ops.push(lut1d_op(&rows));
            }
            "LUT3D" => {
                let (dim, v) = array(node)?;
                let n = dim[0];
                ensure!(v.len() == n * n * n * 3, "LUT3D size mismatch");
                // CLF orders blue fastest; store red fastest.
                let mut data = vec![[0.0f32; 3]; n * n * n];
                for r in 0..n {
                    for g in 0..n {
                        for b in 0..n {
                            let i = ((r * n + g) * n + b) * 3;
                            data[r + n * (g + n * b)] =
                                [0, 1, 2].map(|k| (v[i + k] / out_s) as f32);
                        }
                    }
                }
                let mut op = lut3d_op(n, data)?;
                if let Op::Lut3D { tetra, .. } = &mut op {
                    *tetra = node.attribute("interpolation") == Some("tetrahedral");
                }
                ops.push(op);
            }
            "Range" => {
                let get = |n: &str, s: f64| -> Result<Option<f64>> {
                    child(node, n)
                        .map(|c| Ok(num::<f64>(c.text().unwrap_or("").trim())? / s))
                        .transpose()
                };
                let (min_in, max_in) = (get("minInValue", in_s)?, get("maxInValue", in_s)?);
                let (min_out, max_out) = (get("minOutValue", out_s)?, get("maxOutValue", out_s)?);
                if node.attribute("style") == Some("noClamp") {
                    // Scale/offset only.
                    let (a, b, c, d) = (
                        min_in.unwrap_or(0.),
                        max_in.unwrap_or(1.),
                        min_out.unwrap_or(0.),
                        max_out.unwrap_or(1.),
                    );
                    let s = (d - c) / (b - a);
                    ops.push(Op::scale_offset([s; 3], [c - s * a; 3]));
                } else {
                    ops.push(Op::Range {
                        min_in,
                        max_in,
                        min_out,
                        max_out,
                    });
                }
            }
            "Log" => ops.push(clf_log(node)?),
            "Exponent" => ops.push(clf_exponent(node)?),
            "ASC_CDL" => {
                let sop = child(node, "SOPNode");
                let get3 = |n: &str, d: f64| -> Result<[f64; 3]> {
                    match sop.and_then(|s| child(s, n)) {
                        Some(c) => {
                            let v = floats(c)?;
                            ensure!(v.len() == 3, "{n} needs 3 values");
                            Ok([v[0], v[1], v[2]])
                        }
                        None => Ok([d; 3]),
                    }
                };
                let sat = match child(node, "SatNode").and_then(|s| child(s, "Saturation")) {
                    Some(s) => num(s.text().unwrap_or("").trim())?,
                    None => 1.0,
                };
                let style = node.attribute("style").unwrap_or("Fwd");
                ops.push(Op::Cdl {
                    slope: get3("Slope", 1.)?,
                    offset: get3("Offset", 0.)?,
                    power: get3("Power", 1.)?,
                    sat,
                    clamp: !style.contains("NoClamp") && !style.contains("noClamp"),
                    inverse: style.contains("Rev"),
                });
            }
            other => bail!("unsupported CLF/CTF process node <{other}>"),
        }
    }
    Ok(ops)
}

fn per_channel<T: Copy>(
    node: roxmltree::Node,
    elem: &str,
    f: impl Fn(roxmltree::Node) -> Result<T>,
    default: T,
) -> Result<[T; 3]> {
    let mut out = [default; 3];
    for p in node.children().filter(|c| c.tag_name().name() == elem) {
        let v = f(p)?;
        match p.attribute("channel") {
            Some("R") => out[0] = v,
            Some("G") => out[1] = v,
            Some("B") => out[2] = v,
            _ => out = [v; 3],
        }
    }
    Ok(out)
}

fn attr_f(n: roxmltree::Node, name: &str) -> Result<Option<f64>> {
    n.attribute(name).map(num).transpose()
}

fn clf_log(node: roxmltree::Node) -> Result<Op> {
    let style = node
        .attribute("style")
        .ok_or_else(|| anyhow!("Log without style"))?;
    let (base, inverse, camera) = match style {
        "log10" => (10.0, false, false),
        "log2" => (2.0, false, false),
        "antiLog10" => (10.0, true, false),
        "antiLog2" => (2.0, true, false),
        "linToLog" => (10.0, false, false),
        "logToLin" => (10.0, true, false),
        "cameraLinToLog" => (10.0, false, true),
        "cameraLogToLin" => (10.0, true, true),
        _ => bail!("unsupported Log style '{style}'"),
    };
    let base = std::cell::Cell::new(base);
    let p = per_channel(
        node,
        "LogParams",
        |p| {
            if let Some(b) = attr_f(p, "base")? {
                base.set(b);
            }
            ensure!(
                p.attribute("gamma").is_none(),
                "legacy CTF Log params are not supported"
            );
            Ok(LogParams {
                log_slope: attr_f(p, "logSideSlope")?.unwrap_or(1.),
                log_offset: attr_f(p, "logSideOffset")?.unwrap_or(0.),
                lin_slope: attr_f(p, "linSideSlope")?.unwrap_or(1.),
                lin_offset: attr_f(p, "linSideOffset")?.unwrap_or(0.),
                lin_break: attr_f(p, "linSideBreak")?,
                linear_slope: attr_f(p, "linearSlope")?,
            })
        },
        LogParams::PLAIN,
    )?;
    ensure!(
        !camera || p.iter().all(|c| c.lin_break.is_some()),
        "camera Log needs linSideBreak"
    );
    Ok(Op::Log {
        base: base.get(),
        p,
        inverse,
    })
}

fn clf_exponent(node: roxmltree::Node) -> Result<Op> {
    let style = node
        .attribute("style")
        .ok_or_else(|| anyhow!("Exponent without style"))?;
    let inverse = style.ends_with("Rev");
    let kind = match style.trim_end_matches("Fwd").trim_end_matches("Rev") {
        "basic" => GammaKind::Basic,
        "basicMirror" => GammaKind::Mirror,
        "basicPassThru" => GammaKind::PassThru,
        "monCurve" => GammaKind::Moncurve,
        "monCurveMirror" => GammaKind::MoncurveMirror,
        _ => bail!("unsupported Exponent style '{style}'"),
    };
    let p = per_channel(
        node,
        "ExponentParams",
        |p| {
            Ok((
                attr_f(p, "exponent")?.unwrap_or(1.),
                attr_f(p, "offset")?.unwrap_or(0.),
            ))
        },
        (1.0, 0.0),
    )?;
    Ok(Op::Gamma {
        kind,
        inverse,
        gamma: p.map(|v| v.0),
        offset: p.map(|v| v.1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ops: &[Op], rgb: [f32; 3]) -> [f32; 3] {
        let mut p = rgb;
        ops.iter().for_each(|o| o.apply(&mut p));
        p
    }

    fn close(a: [f32; 3], b: [f32; 3]) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} vs {b:?}");
        }
    }

    // 2x2x2 LUT that swaps red and blue: easy to check any interpolation.
    fn swap_rows() -> Vec<String> {
        let mut rows = Vec::new();
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    rows.push(format!("{b} {g} {r}"));
                }
            }
        }
        rows
    }

    #[test]
    fn cube_iridas_3d() {
        let text = format!(
            "TITLE \"swap\"\n# comment\nLUT_3D_SIZE 2\n{}\n",
            swap_rows().join("\n")
        );
        let ops = parse_cube(&text).unwrap();
        close(run(&ops, [0.2, 0.5, 0.9]), [0.9, 0.5, 0.2]);
    }

    #[test]
    fn cube_iridas_1d_domain() {
        let ops =
            parse_cube("LUT_1D_SIZE 3\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n0 0 0\n1 2 3\n2 4 6\n")
                .unwrap();
        close(run(&ops, [0.5, 1.0, 1.5]), [0.5, 2.0, 4.5]);
    }

    #[test]
    fn cube_resolve_shaper_and_3d() {
        let text = format!(
            "LUT_1D_SIZE 2\nLUT_1D_INPUT_RANGE 0 4\nLUT_3D_SIZE 2\n0 0 0\n1 1 1\n{}\n",
            swap_rows().join("\n")
        );
        let ops = parse_cube(&text).unwrap();
        close(run(&ops, [0.4, 2.0, 4.0]), [1.0, 0.5, 0.1]);
    }

    #[test]
    fn spi1d() {
        let ops =
            parse_spi1d("Version 1\nFrom -1 1\nLength 3\nComponents 1\n{\n 0\n 0.25\n 1\n}\n")
                .unwrap();
        close(run(&ops, [-1.0, 0.0, 0.5]), [0.0, 0.25, 0.625]);
        let inv: Vec<Op> = ops.iter().rev().map(Op::inverse).collect();
        close(run(&inv, [0.0, 0.25, 0.625]), [-1.0, 0.0, 0.5]);
    }

    #[test]
    fn spi3d() {
        let mut text = String::from("SPILUT 1.0\n3 3\n2 2 2\n");
        for r in 0..2 {
            for g in 0..2 {
                for b in 0..2 {
                    text += &format!("{r} {g} {b} {b} {g} {r}\n");
                }
            }
        }
        close(
            run(&parse_spi3d(&text).unwrap(), [0.1, 0.3, 0.7]),
            [0.7, 0.3, 0.1],
        );
    }

    #[test]
    fn csp_1d_with_prelut() {
        // Red prelut passes through its middle point (cubic between points, exact on them).
        let text = "CSPLUTV100\n1D\n\nBEGIN METADATA\nfoo\nEND METADATA\n\n\
                    3\n0 0.5 4\n0 0.75 1\n2\n0 1\n0 1\n2\n0 1\n0 1\n\n3\n0 0 0\n0.5 0.5 0.5\n1 1 1\n";
        let ops = parse_csp(text).unwrap();
        close(run(&ops, [0.5, 0.5, 0.25]), [0.75, 0.5, 0.25]);
        let inv: Vec<Op> = ops.iter().rev().map(Op::inverse).collect();
        close(run(&inv, [0.75, 0.5, 0.25]), [0.5, 0.5, 0.25]);
    }

    #[test]
    fn csp_3d() {
        let text = format!(
            "CSPLUTV100\n3D\n2\n0 1\n0 1\n2\n0 1\n0 1\n2\n0 1\n0 1\n2 2 2\n{}\n",
            swap_rows().join("\n")
        );
        close(
            run(&parse_csp(&text).unwrap(), [0.2, 0.4, 0.6]),
            [0.6, 0.4, 0.2],
        );
    }

    #[test]
    fn clf_process_nodes() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<ProcessList id="test" compCLFversion="3">
  <Description>test</Description>
  <Matrix inBitDepth="32f" outBitDepth="32f">
    <Array dim="3 4">2 0 0 0.1  0 1 0 0  0 0 1 0</Array>
  </Matrix>
  <Range inBitDepth="32f" outBitDepth="32f">
    <minInValue>0</minInValue><maxInValue>2</maxInValue>
    <minOutValue>0</minOutValue><maxOutValue>1</maxOutValue>
  </Range>
  <LUT1D inBitDepth="32f" outBitDepth="10i">
    <Array dim="2 1">0 1023</Array>
  </LUT1D>
  <Exponent style="basicFwd"><ExponentParams exponent="2"/></Exponent>
  <Log style="log2"/>
  <Log style="antiLog2"/>
  <ASC_CDL style="Fwd"><SOPNode><Slope>1 1 1</Slope><Offset>0 0 0</Offset><Power>1 1 1</Power></SOPNode>
    <SatNode><Saturation>1</Saturation></SatNode></ASC_CDL>
  <LUT3D interpolation="tetrahedral">
    <Array dim="2 2 2 3">0 0 0  0 0 1  0 1 0  0 1 1  1 0 0  1 0 1  1 1 0  1 1 1</Array>
  </LUT3D>
</ProcessList>"#;
        let ops = parse_clf(text).unwrap();
        // (2*0.2+0.1)=0.5 -> /2 -> 0.25 -> lut identity -> ^2 = 0.0625
        close(run(&ops, [0.2, 1.0, 0.5]), [0.0625, 0.25, 0.0625]);
    }
}
