use super::*;
use reetched_graph::{Args, render};

fn data(path: &str) -> Option<Vec<u8>> {
    std::fs::read(format!("{}/tests/data/{path}", env!("CARGO_MANIFEST_DIR"))).ok()
}

fn load() -> Package {
    read(&data("main.sbs").unwrap(), "main.sbs", &data).unwrap()
}

#[test]
fn reads_graphs_inputs_and_dependencies() {
    let pkg = load();
    let main = pkg.graph("main").unwrap();
    assert_eq!(main.label, "Main material");
    let tint = main.input("tint").unwrap();
    assert_eq!(tint.widget, Widget::Color);
    assert_eq!(tint.group, "Color");
    assert_eq!(tint.default, V::f4([1.0, 0.5, 0.25, 1.0]));
    let amount = main.input("amount").unwrap();
    assert_eq!((amount.min, amount.max, amount.clamp), (0.0, 0.5, true));
    assert_eq!(main.outputs[0].usage.as_deref(), Some("baseColor"));
    assert!(pkg.functions.contains_key("double"));
    assert!(pkg.resources.contains_key("Resources/checker"));
    assert!(pkg.deps["200"].is_ok());
    assert!(
        pkg.deps["300"]
            .as_ref()
            .unwrap_err()
            .contains("library package")
    );
}

#[test]
fn renders_the_test_package() {
    let pkg = load();
    let r = render(
        &pkg,
        "main",
        &Args {
            size_log2: [3, 3],
            ..Default::default()
        },
    );
    let out = |id: &str| {
        r.outputs
            .iter()
            .find(|(o, _)| o.id == id)
            .unwrap()
            .1
            .clone()
    };

    // Height: invert(double(u)) = 1 - min(2u, 1).
    let height = out("height");
    assert_eq!((height.w, height.h), (8, 8));
    for x in 0..8 {
        let u = (x as f32 + 0.5) / 8.0;
        let want = 1.0 - (2.0 * u).min(1.0);
        assert!((height.get_gray(x, 0) - want).abs() < 1e-5, "x={x}");
    }

    // Base color: white multiplied by tint at opacity 0.25.
    let c = out("basecolor").get(2, 5);
    for (got, tint) in c[..3].iter().zip([1.0, 0.5, 0.25]) {
        assert!((got - (1.0 + (tint - 1.0) * 0.25)).abs() < 1e-5, "{c:?}");
    }

    // The bitmap resource, upsampled from 2×2.
    let b = out("bitmap");
    assert!(
        b.get(2, 2)[0] > 0.7 && b.get(6, 2)[0] < 0.3,
        "{:?} {:?}",
        b.get(2, 2),
        b.get(6, 2)
    );

    // The missing library node renders grey and is reported.
    assert_eq!(out("missing").get(0, 0)[0], 0.5);
    assert!(
        r.warnings.iter().any(|w| w.contains("pattern_missing")),
        "{:?}",
        r.warnings
    );
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
}

#[test]
fn input_overrides_reach_functions() {
    let pkg = load();
    let r = render(
        &pkg,
        "main",
        &Args {
            size_log2: [2, 2],
            values: HashMap::from([
                ("tint".to_string(), V::f4([0.0, 0.0, 0.0, 1.0])),
                ("amount".to_string(), V::f(1.0)),
            ]),
            ..Default::default()
        },
    );
    let base = &r
        .outputs
        .iter()
        .find(|(o, _)| o.id == "basecolor")
        .unwrap()
        .1;
    assert_eq!(base.get(1, 1)[..3], [0.0, 0.0, 0.0]);
}

#[test]
fn joins_dependency_paths() {
    assert_eq!(join("main.sbs", "lib/a.sbs"), "lib/a.sbs");
    assert_eq!(join("lib/a.sbs", "../b.sbs"), "b.sbs");
    assert_eq!(join("lib/a.sbs", "sbs://noise.sbs"), "sbs://noise.sbs");
    assert_eq!(join("a.sbs", "..\\up\\c.sbs"), "../up/c.sbs");
}

#[test]
fn rejects_non_packages() {
    assert!(read(b"<graph/>", "x.sbs", &|_| None).is_err());
    assert!(read(b"not xml", "x.sbs", &|_| None).is_err());
}
