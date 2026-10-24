use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use reetched_library::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("reetched-lib-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn touch(root: &Path, rel: &str, content: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn root(name: &str, path: &Path, writable: bool) -> LibraryRoot {
    LibraryRoot {
        name: name.into(),
        path: path.into(),
        writable,
    }
}

fn id(s: &str) -> AssetId {
    AssetId(s.into())
}

#[test]
fn scan_and_classify() {
    let t = TempDir::new("scan");
    let lib_dir = t.0.join("lib");
    for rel in [
        "textures/rusty_metal-01.png",
        "Alphas/scratch.png",
        "brushes/Brushes Alphas/dot.tga",
        "environments/studio.exr",
        "textures/height.exr",
        "misc/sky.hdr",
        "materials/gold.rsm",
        "masks/edge.rsmk",
        "brushes/soft.rbrush",
        "graphs/noise.sbs",
        "shaders/pbr.glsl",
        "color/aces.ocio",
        "templates/game.rtpl",
        "meshes/head.FBX",
        "icons/logo.svg",
        "notes.txt",
        ".hidden/skip.png",
    ] {
        touch(&lib_dir, rel, b"x");
    }
    let mut lib = Library::new(t.0.join("meta"));
    lib.add_root(root("Mine", &lib_dir, true)).unwrap();

    let kind = |rel: &str| lib.get(&id(&format!("Mine/{rel}"))).map(|a| a.kind);
    assert_eq!(
        kind("textures/rusty_metal-01.png"),
        Some(AssetKind::Texture)
    );
    assert_eq!(kind("Alphas/scratch.png"), Some(AssetKind::Alpha));
    assert_eq!(
        kind("brushes/Brushes Alphas/dot.tga"),
        Some(AssetKind::Alpha)
    );
    assert_eq!(
        kind("environments/studio.exr"),
        Some(AssetKind::Environment)
    );
    assert_eq!(kind("textures/height.exr"), Some(AssetKind::Texture));
    assert_eq!(kind("misc/sky.hdr"), Some(AssetKind::Environment));
    assert_eq!(kind("materials/gold.rsm"), Some(AssetKind::SmartMaterial));
    assert_eq!(kind("masks/edge.rsmk"), Some(AssetKind::SmartMask));
    assert_eq!(kind("brushes/soft.rbrush"), Some(AssetKind::Brush));
    assert_eq!(kind("graphs/noise.sbs"), Some(AssetKind::Procedural));
    assert_eq!(kind("shaders/pbr.glsl"), Some(AssetKind::Shader));
    assert_eq!(kind("color/aces.ocio"), Some(AssetKind::OcioConfig));
    assert_eq!(
        kind("templates/game.rtpl"),
        Some(AssetKind::ProjectTemplate)
    );
    assert_eq!(kind("meshes/head.FBX"), Some(AssetKind::Mesh));
    assert_eq!(kind("icons/logo.svg"), Some(AssetKind::Texture));
    assert_eq!(kind("notes.txt"), None);
    assert_eq!(kind(".hidden/skip.png"), None);
    assert_eq!(lib.assets().count(), 15);
    assert_eq!(
        lib.get(&id("Mine/textures/rusty_metal-01.png"))
            .unwrap()
            .name,
        "Rusty Metal 01"
    );
}

#[test]
fn metadata_survives_reload_and_move() {
    let t = TempDir::new("meta");
    let mine = t.0.join("mine");
    let shared = t.0.join("shared");
    let big: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    touch(&mine, "textures/plate.png", &big);
    touch(&mine, "textures/other.png", &big[..1000]);
    touch(&shared, "brushes/soft.rbrush", b"{}");
    let meta_dir = t.0.join("meta");

    {
        let mut lib = Library::new(&meta_dir);
        lib.add_root(root("Mine", &mine, true)).unwrap();
        lib.add_root(root("Shared", &shared, false)).unwrap();
        let plate = id("Mine/textures/plate.png");
        lib.set_tags(&plate, ["metal", " Rusty ", "metal", ""])
            .unwrap();
        lib.set_favorite(&plate, true).unwrap();
        lib.set_display_name(&plate, Some("Steel plate".into()))
            .unwrap();
        lib.set_favorite(&id("Shared/brushes/soft.rbrush"), true)
            .unwrap();
        assert_eq!(lib.get(&plate).unwrap().tags, ["metal", "Rusty"]);
        assert_eq!(lib.all_tags(), ["Rusty", "metal"]);
    }
    assert!(mine.join(METADATA_FILE).exists());
    assert!(meta_dir.join("Shared.json").exists());
    assert!(!shared.join(METADATA_FILE).exists());

    {
        let mut lib = Library::new(&meta_dir);
        lib.add_root(root("Mine", &mine, true)).unwrap();
        lib.add_root(root("Shared", &shared, false)).unwrap();
        let a = lib.get(&id("Mine/textures/plate.png")).unwrap();
        assert_eq!(
            (a.name.as_str(), a.favorite, a.tags.len()),
            ("Steel plate", true, 2)
        );
        assert!(lib.get(&id("Shared/brushes/soft.rbrush")).unwrap().favorite);
    }

    // Move while the app is closed: identical content keeps its tags at the new path.
    fs::create_dir_all(mine.join("metals")).unwrap();
    fs::rename(
        mine.join("textures/plate.png"),
        mine.join("metals/plate2.png"),
    )
    .unwrap();
    let mut lib = Library::new(&meta_dir);
    lib.add_root(root("Mine", &mine, true)).unwrap();
    let a = lib.get(&id("Mine/metals/plate2.png")).unwrap();
    assert_eq!((a.name.as_str(), a.favorite), ("Steel plate", true));
    assert_eq!(a.tags, ["metal", "Rusty"]);
    assert!(!lib.get(&id("Mine/textures/other.png")).unwrap().favorite);
}

#[test]
fn query_syntax() {
    let t = TempDir::new("query");
    for rel in [
        "textures/Rusty Metal.png",
        "textures/metal.png",
        "textures/wood_planks.png",
        "alphas/metal scratch.png",
        "materials/gold.rsm",
    ] {
        touch(&t.0, rel, b"x");
    }
    let mut lib = Library::new(t.0.join("meta"));
    lib.add_root(root("L", &t.0, true)).unwrap();
    lib.set_tags(&id("L/textures/wood_planks.png"), ["floor", "Natural"])
        .unwrap();
    lib.set_tags(&id("L/materials/gold.rsm"), ["metal", "shiny"])
        .unwrap();
    lib.set_favorite(&id("L/materials/gold.rsm"), true).unwrap();

    let names = |text: &str, kind: Option<AssetKind>, fav: bool| -> Vec<String> {
        lib.query(text, kind, fav)
            .into_iter()
            .map(|a| a.name.clone())
            .collect()
    };
    // Exact name first, then prefix, then word start, then tag-only match.
    assert_eq!(
        names("METAL", None, false),
        ["Metal", "Metal Scratch", "Rusty Metal", "Gold"]
    );
    assert_eq!(names("metal kind:alpha", None, false), ["Metal Scratch"]);
    assert_eq!(names("tag:metal", None, false), ["Gold"]);
    assert_eq!(names("tag:nat", None, false), ["Wood Planks"]);
    assert_eq!(names("\"rusty metal\"", None, false), ["Rusty Metal"]);
    assert_eq!(names("\"metal rusty\"", None, false), Vec::<String>::new());
    assert_eq!(names("metal rusty", None, false), ["Rusty Metal"]);
    assert_eq!(names("", None, true), ["Gold"]);
    assert_eq!(names("", Some(AssetKind::SmartMaterial), false), ["Gold"]);
    assert_eq!(names("kind:bogus", None, false), Vec::<String>::new());
    assert_eq!(names("kind:\"smart material\"", None, false), ["Gold"]);
    assert_eq!(names("", None, false).len(), 5);
}

#[test]
fn import_and_remove() {
    let t = TempDir::new("import");
    let lib_dir = t.0.join("lib");
    fs::create_dir_all(&lib_dir).unwrap();
    touch(&t.0, "in/rock.png", b"png");
    touch(&t.0, "in/soft.rbrush", b"{}");
    touch(&t.0, "in/readme.txt", b"?");
    let mut lib = Library::new(t.0.join("meta"));
    lib.add_root(root("Mine", &lib_dir, true)).unwrap();
    lib.add_root(root("Ro", &t.0.join("in"), false)).unwrap();
    lib.poll();

    assert!(
        lib.import_into(
            "Mine",
            &[t.0.join("in/rock.png"), t.0.join("in/readme.txt")]
        )
        .is_err()
    );
    assert!(lib.import_into("Ro", &[t.0.join("in/rock.png")]).is_err());
    let ids = lib
        .import_into(
            "Mine",
            &[
                t.0.join("in/rock.png"),
                t.0.join("in/rock.png"),
                t.0.join("in/soft.rbrush"),
            ],
        )
        .unwrap();
    assert_eq!(
        ids,
        [
            id("Mine/textures/rock.png"),
            id("Mine/textures/rock (2).png"),
            id("Mine/brushes/soft.rbrush")
        ]
    );
    assert!(lib_dir.join("textures/rock (2).png").exists());

    lib.remove(&ids[0]).unwrap();
    assert!(!lib_dir.join("textures/rock.png").exists());
    assert!(lib.get(&ids[0]).is_none());
    let events = lib.poll();
    assert!(events.contains(&LibraryEvent::Added(ids[2].clone())));
    assert!(events.contains(&LibraryEvent::Removed(ids[0].clone())));
}

/// Polls until `pred` sees the event it wants, or gives up after a generous timeout
/// (watcher latency varies; the polling fallback needs up to ~5 s).
fn wait_for(lib: &mut Library, mut pred: impl FnMut(&LibraryEvent) -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(15);
    while Instant::now() < end {
        if lib.poll().iter().any(&mut pred) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn watcher_sees_create_rename_delete() {
    let t = TempDir::new("watch");
    let mut lib = Library::new(t.0.join("meta"));
    lib.add_root(root("W", &t.0, true)).unwrap();
    if let Some(LibraryEvent::WatchFailed { error, .. }) = lib.poll().first() {
        eprintln!("watcher unavailable ({error}); testing the polling fallback");
    }

    touch(&t.0, "textures/new.png", b"data");
    let new = id("W/textures/new.png");
    assert!(
        wait_for(&mut lib, |e| *e == LibraryEvent::Added(new.clone())),
        "create not seen"
    );

    fs::rename(
        t.0.join("textures/new.png"),
        t.0.join("textures/renamed.png"),
    )
    .unwrap();
    let renamed = id("W/textures/renamed.png");
    let (mut gone, mut added) = (false, false);
    assert!(
        wait_for(&mut lib, |e| {
            gone |= *e == LibraryEvent::Removed(new.clone());
            added |= *e == LibraryEvent::Added(renamed.clone());
            gone && added
        }),
        "rename not seen"
    );

    fs::remove_file(t.0.join("textures/renamed.png")).unwrap();
    assert!(
        wait_for(&mut lib, |e| *e == LibraryEvent::Removed(renamed.clone())),
        "delete not seen"
    );
    assert_eq!(lib.assets().count(), 0);
}

fn wait_thumb(lib: &mut Library, id: &AssetId) -> Arc<image::RgbaImage> {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(t) = lib.thumbnail(id) {
            return t;
        }
        assert!(Instant::now() < end, "thumbnail for {id} never arrived");
        lib.poll();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn thumbnails_cached_across_restart() {
    let t = TempDir::new("thumbs");
    let lib_dir = t.0.join("lib");
    fs::create_dir_all(lib_dir.join("env")).unwrap();
    image::RgbaImage::from_pixel(300, 150, image::Rgba([255, 0, 0, 255]))
        .save(lib_dir.join("wide.png"))
        .unwrap();
    let hdr = image::Rgb32FImage::from_pixel(64, 32, image::Rgb([1.0, 1.0, 1.0]));
    image::DynamicImage::ImageRgb32F(hdr)
        .save(lib_dir.join("env/sky.hdr"))
        .unwrap();
    touch(&lib_dir, "mat.rsm", b"x");
    let cache = t.0.join("cache");

    let calls = Arc::new(AtomicUsize::new(0));
    let open = |calls: &Arc<AtomicUsize>| {
        let calls = calls.clone();
        let decoder: Decoder = Arc::new(move |_: &Path, _: &[u8]| {
            calls.fetch_add(1, Ordering::SeqCst);
            None // defer to the image crate
        });
        let mut lib = Library::new(t.0.join("meta")).with_thumbnails(
            &cache,
            DEFAULT_THUMB_SIZE,
            Some(decoder),
        );
        lib.add_root(root("T", &lib_dir, true)).unwrap();
        lib
    };

    let wide = id("T/wide.png");
    let sky = id("T/env/sky.hdr");
    {
        let mut lib = open(&calls);
        assert!(lib.thumbnail(&id("T/mat.rsm")).is_none());
        let img = wait_thumb(&mut lib, &wide);
        assert_eq!(img.dimensions(), (128, 128));
        assert_eq!(img.get_pixel(64, 64).0, [255, 0, 0, 255]);
        assert_eq!(
            img.get_pixel(64, 2).0[3],
            0,
            "letterbox must be transparent"
        );
        let env = wait_thumb(&mut lib, &sky);
        // Reinhard maps 1.0 to 0.5, sRGB-encoded ~188; a 2:1 strip 128x64 centered.
        assert_eq!(env.get_pixel(64, 64).0, [188, 188, 188, 255]);
        assert_eq!(env.get_pixel(64, 20).0[3], 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "decoder tried once for the png"
        );
    }
    let start = Instant::now();
    let mut lib = open(&calls);
    wait_thumb(&mut lib, &wide);
    wait_thumb(&mut lib, &sky);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "second run must come from the disk cache"
    );
    assert!(start.elapsed() < Duration::from_secs(5));
}
