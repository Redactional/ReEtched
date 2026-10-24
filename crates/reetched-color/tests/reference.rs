//! Numerical agreement with PyOpenColorIO (see gen_reference.py for how the data is made).
//!
//! Error metric: |ours - ocio| / max(1, |ocio|), i.e. absolute below 1 and relative above.

use std::collections::HashMap;

use reetched_color::{Config, Processor};
use serde_json::Value;

const REFERENCE: &str = include_str!("data/reference.json");

/// Analytic transforms (matrices, curves, log/exponent, CDL, ...).
const TOL: f64 = 1e-4;

fn inputs(r: &Value, name: &str) -> Vec<[f32; 3]> {
    r["inputs"][name]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| [0, 1, 2].map(|i| p[i].as_f64().unwrap() as f32))
        .collect()
}

/// Largest error of `proc` against the expected outputs of `case`.
fn case_error(r: &Value, proc: &Processor, case: &Value) -> (f64, String) {
    let ins = inputs(r, case["inputs"].as_str().unwrap());
    let mut worst = (0.0f64, String::new());
    for (rgb, exp) in ins.iter().zip(case["out"].as_array().unwrap()) {
        let mut p = *rgb;
        proc.apply_rgb(&mut p);
        for c in 0..3 {
            let Some(e) = exp[c].as_f64() else { continue };
            let err = (p[c] as f64 - e).abs() / e.abs().max(1.0);
            let err = if err.is_nan() { f64::INFINITY } else { err };
            if err > worst.0 {
                worst = (err, format!("in {rgb:?} got {p:?} want {exp}"));
            }
        }
    }
    worst
}

#[derive(Default)]
struct Report {
    max: HashMap<&'static str, (f64, String)>,
    failures: Vec<String>,
}

impl Report {
    fn add(
        &mut self,
        category: &'static str,
        tol: f64,
        label: String,
        (err, detail): (f64, String),
    ) {
        let e = self.max.entry(category).or_default();
        if err > e.0 {
            *e = (err, label.clone());
        }
        if err > tol {
            self.failures.push(format!(
                "[{category}] {label}: error {err:.3e} > {tol:.0e}: {detail}"
            ));
        }
    }

    fn finish(self, what: &str) {
        let mut cats: Vec<_> = self.max.iter().collect();
        cats.sort_by_key(|(k, _)| *k);
        for (cat, (err, label)) in cats {
            println!("{what} [{cat}] max error {err:.3e} ({label})");
        }
        assert!(
            self.failures.is_empty(),
            "{} mismatches:\n{}",
            self.failures.len(),
            self.failures.join("\n")
        );
    }
}

/// ACES output transforms: the 2.0 rendering is table-driven f32 math (hue/cusp tables built
/// by bisection), so libm differences in powf/atan2 move results slightly; measured worst case
/// is 6.7e-5 (HDR 1000 nit P3), the margin covers other platforms' libm. OCIO also bakes
/// PQ/HLG and the ACES 1.x roll-offs into half-float LUTs that this crate evaluates
/// analytically (measured below 2e-6).
const TOL_ACES_OUTPUT: f64 = 2e-4;

fn view_tolerance(view: &str) -> (&'static str, f64) {
    if view.starts_with("ACES") {
        ("aces output views", TOL_ACES_OUTPUT)
    } else {
        ("other views", TOL)
    }
}

#[test]
fn vendored_configs_match_ocio() {
    let r: Value = serde_json::from_str(REFERENCE).unwrap();
    for (name, cases) in r["configs"].as_object().unwrap() {
        let config = Config::builtin(name).unwrap();
        let mut report = Report::default();

        assert_eq!(
            config.default_display(),
            cases["default_display"].as_str(),
            "{name}"
        );
        for (display, views) in cases["views"].as_object().unwrap() {
            let want: Vec<&str> = views
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(config.views(display), want, "{name}: views of {display}");
        }
        let want_cs: Vec<&str> = cases["color_spaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let mut got_cs = config.color_space_names();
        got_cs.sort();
        let mut want_sorted = want_cs.clone();
        want_sorted.sort();
        assert_eq!(got_cs, want_sorted, "{name}: active color spaces");

        for case in cases["cs"].as_array().unwrap() {
            let (src, dst) = (case["src"].as_str().unwrap(), case["dst"].as_str().unwrap());
            let proc = config
                .processor(src, dst)
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            report.add(
                "color spaces",
                TOL,
                format!("{name}: {src} -> {dst}"),
                case_error(&r, &proc, case),
            );
        }
        for case in cases["display"].as_array().unwrap() {
            let s = |k: &str| case[k].as_str().unwrap();
            let looks = case["looks"].as_str();
            let proc = config
                .display_processor(s("src"), s("display"), s("view"), looks)
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let (cat, tol) = view_tolerance(s("view"));
            let label = format!(
                "{name}: {} -> {} / {} looks={looks:?}",
                s("src"),
                s("display"),
                s("view")
            );
            report.add(cat, tol, label, case_error(&r, &proc, case));
        }
        report.finish(name);
    }
}

#[test]
fn synthetic_config_matches_ocio() {
    let r: Value = serde_json::from_str(REFERENCE).unwrap();
    let syn = &r["synthetic"];
    let files: HashMap<String, String> = syn["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    let config = Config::from_str(syn["yaml"].as_str().unwrap(), move |p| {
        files
            .get(p)
            .map(|t| t.as_bytes().to_vec())
            .ok_or_else(|| anyhow::anyhow!("no file {p}"))
    })
    .unwrap();
    let mut report = Report::default();
    for case in syn["cs"].as_array().unwrap() {
        let (src, dst) = (case["src"].as_str().unwrap(), case["dst"].as_str().unwrap());
        let proc = config
            .processor(src, dst)
            .unwrap_or_else(|e| panic!("{src} -> {dst}: {e:#}"));
        report.add(
            "synthetic",
            TOL,
            format!("{src} -> {dst}"),
            case_error(&r, &proc, case),
        );
    }
    for case in syn["display"].as_array().unwrap() {
        let s = |k: &str| case[k].as_str().unwrap();
        let proc = config
            .display_processor(s("src"), s("display"), s("view"), None)
            .unwrap();
        report.add(
            "synthetic",
            TOL,
            format!("view {}", s("view")),
            case_error(&r, &proc, case),
        );
    }
    report.finish("synthetic");
}

#[test]
fn builtins_match_ocio() {
    let r: Value = serde_json::from_str(REFERENCE).unwrap();
    let mut report = Report::default();
    for case in r["builtins"].as_array().unwrap() {
        let style = case["style"].as_str().unwrap();
        let inverse = case["inverse"].as_bool().unwrap();
        let yaml = format!(
            "ocio_profile_version: 2.4\nroles: {{}}\ncolorspaces:\n  - !<ColorSpace> {{name: ref}}\n  - !<ColorSpace>\n    name: test\n    to_scene_reference: !<BuiltinTransform> {{style: \"{style}\"}}\n"
        );
        let config = Config::from_str(&yaml, |p| anyhow::bail!("no file {p}")).unwrap();
        let proc = if inverse {
            config.processor("ref", "test")
        } else {
            config.processor("test", "ref")
        };
        let proc = proc.unwrap_or_else(|e| panic!("{style}: {e:#}"));
        let (cat, tol) = if style.starts_with("ACES-OUTPUT") {
            ("aces output", TOL_ACES_OUTPUT)
        } else {
            ("other builtins", TOL)
        };
        report.add(
            cat,
            tol,
            format!("{style} inverse={inverse}"),
            case_error(&r, &proc, case),
        );
    }
    report.finish("builtins");
}
