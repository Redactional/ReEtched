use std::time::Instant;

use reetched_color::{Config, Processor, Shaper, builtin_configs};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn processor_is_send_sync() {
    assert_send_sync::<Processor>();
    assert_send_sync::<Config>();
}

#[test]
fn builtin_configs_load() {
    for (name, _) in builtin_configs() {
        let config = Config::builtin(name).unwrap();
        let display = config.default_display().unwrap();
        for view in config.views(display) {
            config
                .display_processor("scene_linear", display, view, None)
                .unwrap();
        }
    }
}

/// The baked LUT, sampled the way the shader does, stays close to the exact transform for
/// scene-linear input across the shaper range (display-encoded output, 0..1).
#[test]
fn baked_lut_matches_processor() {
    let config = Config::builtin("cg-config-v4.0.0_aces-v2.0_ocio-v2.5").unwrap();
    let t = Instant::now();
    let proc = config
        .display_processor(
            "ACEScg",
            "sRGB - Display",
            "ACES 2.0 - SDR 100 nits (Rec.709)",
            None,
        )
        .unwrap();
    println!("build processor: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    for size in [33u32, 65] {
        let t = Instant::now();
        let lut = proc.bake_lut3d(size, Shaper::default());
        println!("bake {size}^3: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(lut.data.len(), (size * size * size) as usize);

        // Deterministic pseudo-random scene colors: a base level `lo..hi` stops around 0.18,
        // each channel within `spread` stops of it. Returns the worst |lut - exact|.
        let error = |lo: f32, hi: f32, spread: f32| {
            let mut worst = 0.0f32;
            for i in 0..4000u32 {
                let h = |k: u32| {
                    (i.wrapping_mul(2654435761) ^ k.wrapping_mul(40503)).wrapping_mul(2246822519) >> 8
                } as f32
                    / (1 << 24) as f32;
                let stops = lo + h(7) * (hi - lo);
                let rgb = [0, 1, 2].map(|c| 0.18 * (stops + (h(c) - 0.5) * spread).exp2());
                let mut exact = rgb;
                proc.apply_rgb(&mut exact);
                let got = lut.sample(rgb);
                for c in 0..3 {
                    worst = worst.max((got[c] - exact[c]).abs());
                }
            }
            worst
        };
        let mid = error(-9.0, 1.0, 2.0);
        let all = error(-9.0, 8.0, 2.0);
        let saturated = error(-9.0, 8.0, 12.0);
        println!(
            "bake {size}^3: max |lut - exact| (sRGB-encoded): {mid:.1e} up to 0.36 scene, {all:.1e} up to ~46, {saturated:.1e} for very saturated colors"
        );
        // Regression bounds, not accuracy goals: per-channel log addressing interpolates ACES 2.0
        // gamut compression of saturated colors coarsely, the sRGB encoding amplifies that near
        // zero, and ACES 2.0 drops chroma abruptly once J passes the display peak.
        assert!(
            mid < if size == 33 { 4e-2 } else { 1.5e-2 },
            "size {size}: {mid}"
        );
    }
}

#[test]
fn shaper_round_trips() {
    let s = Shaper::default();
    for x in [0.001f32, 0.18, 1.0, 100.0] {
        assert!((s.decode(s.encode(x)) - x).abs() / x < 1e-5);
    }
    assert_eq!(s.encode(-1.0), 0.0);
    assert_eq!(s.encode(1e9), 1.0);
}
