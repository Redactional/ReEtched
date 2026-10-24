//! End-to-end tests: real CPython + the reetched-python bridge, against a mock app.
//! Skipped (with a message) when no Python 3.8+ is installed.

mod mock;

use mock::MockHost;
use reetched_script::{Engine, LogLevel, find_python, library_name};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Builds the bridge cdylib with the same profile. It goes into its own target
/// dir because the outer `cargo test` keeps ours locked while tests run.
fn build_bridge() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    // <target>/<profile>/deps/engine-xxxx.exe
    let target = exe
        .ancestors()
        .nth(3)
        .unwrap()
        .join("reetched-python-tests");
    let release = !cfg!(debug_assertions);
    let mut cmd = Command::new(option_env!("CARGO").unwrap_or("cargo"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["build", "-p", "reetched-python", "--target-dir"])
        .arg(&target);
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .expect("running cargo to build reetched-python");
    assert!(status.success(), "building reetched-python failed");
    target
        .join(if release { "release" } else { "debug" })
        .join(library_name())
}

fn plugin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/plugins")
}

/// One interpreter per process, so the tests share an engine (serialised).
fn engine() -> Option<MutexGuard<'static, Engine>> {
    static ENGINE: OnceLock<Option<Mutex<Engine>>> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| {
        let install = match find_python() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skipping Python tests: {e}");
                return None;
            }
        };
        let library = build_bridge();
        let engine = Engine::start_with_library(&library, &install, &[plugin_dir()])
            .expect("starting Python");
        Some(Mutex::new(engine))
    });
    engine
        .as_ref()
        .map(|m| m.lock().unwrap_or_else(|e| e.into_inner()))
}

macro_rules! engine_or_skip {
    () => {
        match engine() {
            Some(e) => e,
            None => return,
        }
    };
}

/// Runs code that must succeed; returns its output.
fn run(engine: &mut Engine, host: &mut MockHost, code: &str) -> String {
    let r = engine.run(host, code, "<test>");
    assert!(
        r.ok(),
        "script failed:\n{}\noutput:\n{}",
        r.error.unwrap(),
        r.output
    );
    r.output
}

#[test]
fn python_is_found() {
    match find_python() {
        Ok(install) => {
            println!(
                "found Python {} at {}",
                install.version,
                install.home.display()
            );
            assert!(install.library.is_file());
        }
        Err(e) => eprintln!("skipping: {e}"),
    }
}

#[test]
fn console_keeps_state_between_runs() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    assert_eq!(run(&mut e, &mut host, "x = 41"), "");
    assert_eq!(run(&mut e, &mut host, "x + 1"), "42\n");
    assert_eq!(run(&mut e, &mut host, "print('a'); x * 2"), "a\n82\n");
    // Console output goes to the RunResult, not the log.
    assert!(host.logs.is_empty(), "{:?}", host.logs);
}

#[test]
fn tracebacks_have_line_numbers() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    let r = e.run(
        &mut host,
        "a = 1\n\ndef f():\n    raise ValueError('boom')\nf()\n",
        "<tb-test>",
    );
    let err = r.error.expect("should fail");
    assert!(err.contains("File \"<tb-test>\", line 4"), "{err}");
    assert!(
        err.contains("raise ValueError('boom')"),
        "source line missing:\n{err}"
    );
    assert!(err.contains("ValueError: boom"), "{err}");
    assert!(!err.contains("_engine"), "engine frames leaked:\n{err}");

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/sample_script.py");
    let r = e.run_file(&mut host, &script);
    assert_eq!(r.output, "__main__ sample_script.py\n");
    let err = r.error.expect("sample script raises");
    assert!(
        err.contains("sample_script.py\", line 5") && err.contains("ZeroDivisionError"),
        "{err}"
    );

    let err = e
        .run(&mut host, "y = (\n", "<syntax>")
        .error
        .expect("syntax error");
    assert!(
        err.contains("SyntaxError") && err.contains("<syntax>"),
        "{err}"
    );

    // Host errors surface as ReEtchedError.
    let out = run(
        &mut e,
        &mut host,
        "import reetched\ntry:\n    reetched.textureset.set_active('Nope')\nexcept reetched.ReEtchedError as err:\n    print('refused:', err)",
    );
    assert_eq!(out, "refused: no texture set \"Nope\"\n");
}

#[test]
fn project_and_textureset_round_trip() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    let out = run(
        &mut e,
        &mut host,
        r#"
from reetched import project, textureset
print(project.is_open(), project.name(), project.file_path())
sets = textureset.all_texture_sets()
print([t.name for t in sets])
body = textureset.get_active()
body.resolution = 4096
body.add_channel("emissive")
body.remove_channel("height")
body.shader = "openpbr"
print(body.resolution, body.channels(), body.shader, body.udim_tiles())
textureset.set_active(sets[1])
print(textureset.get_active())
print(project.save("C:/projects/robot2.ret"), project.undo())
"#,
    );
    assert_eq!(
        out,
        "True robot C:/projects/robot.ret\n\
         ['Body', 'Visor']\n\
         (4096, 4096) ['basecolor', 'metallic', 'roughness', 'normal', 'emissive'] openpbr [1001]\n\
         TextureSet('Visor')\n\
         C:/projects/robot2.ret True\n"
    );
    assert_eq!(host.doc.sets[0]["resolution"], json!([4096, 4096]));
    // The undo reverted the shader change.
    assert_eq!(host.doc.sets[0]["shader"], "metal_rough");
}

#[test]
fn layerstack_round_trip() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    let out = run(
        &mut e,
        &mut host,
        r#"
from reetched import layerstack as ls, textureset, resource
ts = textureset.get_active()
fill = ls.insert_fill(ls.InsertPosition.from_texture_set_stack(ts))
fill.name = "Rust"
fill.active_channels = ["basecolor", "roughness"]
fill.set_channel_value("basecolor", (0.4, 0.1, 0.05))
fill.set_channel_value("roughness", 0.8)
fill.set_blending_mode(ls.BlendingMode.Multiply, "basecolor")
fill.set_opacity(0.5, "roughness")
fill.add_mask("black")
group = ls.insert_group(ls.InsertPosition.above_node(fill))
paint = ls.insert_paint(ls.InsertPosition.inside_node(group))
tex = resource.import_project_resource("C:/tex/grunge.png")
fill.set_channel_bitmap("roughness", tex)
print([n.name for n in ls.get_root_layer_nodes(ts)])
print(group.children(), paint.parent() == group)
print(fill.blending_mode("basecolor"), fill.opacity("roughness"), fill.has_mask(), fill.channel_value("basecolor"))
ls.move_node(fill, ls.InsertPosition.inside_node(group))
ls.set_selected_nodes([paint, fill])
print([n.name for n in ls.get_selected_nodes()])
paint.visible = False
ls.delete_node(paint)
print([type(n).__name__ for n in group.children()], fill.visible)
print(resource.list_project_resources())
"#,
    );
    assert_eq!(
        out,
        "['group 2', 'Rust']\n\
         [PaintLayerNode('Body', 3)] True\n\
         BlendingMode.Multiply 0.5 True (0.4, 0.1, 0.05)\n\
         ['paint 3', 'Rust']\n\
         ['FillLayerNode'] True\n\
         [Resource(100, 'grunge.png')]\n"
    );
    let fill = host.node(1).unwrap();
    assert_eq!(fill["fill"]["roughness"], json!({ "resource": 100 }));
    assert_eq!(fill["blending"]["basecolor"], "Multiply");
    assert_eq!(fill["active_channels"], json!(["basecolor", "roughness"]));
    assert_eq!(host.nodes(0).len(), 1);
}

#[test]
fn export_bake_and_display_round_trip() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    let out = run(
        &mut e,
        &mut host,
        r#"
from reetched import export, baking, display
print(export.list_presets())
result = export.export_project_textures({
    "exportPath": "C:/out",
    "defaultExportPreset": "Unreal Engine",
    "exportList": [{"rootPath": "Visor"}],
    "exportParameters": [{"parameters": {"fileFormat": "tga", "bitDepth": "16", "paddingAlgorithm": "infinite", "dilationDistance": 8}}],
})
print(result.all_files()[:2])
export.export_mesh("C:/out/robot.fbx")
print(baking.bake(["Body"], resolution=1024, maps=["normal", "ambient_occlusion"]))
display.set_exposure(1.5)
display.set_tone_mapping("agx")
print(display.get_exposure(), display.get_tone_mapping(), display.get_environment())
"#,
    );
    assert_eq!(
        out,
        "['PBR Metallic Roughness', 'Unreal Engine']\n\
         ['C:/out/Visor_basecolor.tga', 'C:/out/Visor_metallic.tga']\n\
         ['Body']\n\
         1.5 agx studio.hdr\n"
    );
    let (_, args) = host
        .calls
        .iter()
        .find(|(m, _)| m == "export.textures")
        .unwrap();
    assert_eq!(
        *args,
        json!({
            "path": "C:/out", "preset": "Unreal Engine", "texture_sets": ["Visor"], "format": "tga",
            "bit_depth": 16, "padding": "infinite", "dilation": 8,
        })
    );
}

#[test]
fn events_reach_connected_callbacks() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    run(
        &mut e,
        &mut host,
        r#"
from reetched import event
seen = []
def on_saved(ev): seen.append(("saved", ev.path))
def on_any(ev): seen.append(("any", ev.name))
def broken(ev): raise KeyError("callback bug")
event.DISPATCHER.connect(event.ProjectSaved, broken)
event.DISPATCHER.connect(event.ProjectSaved, on_saved)
event.DISPATCHER.connect(event.Event, on_any)
"#,
    );
    e.event(&mut host, "project_saved", json!({ "path": "C:/p.ret" }));
    e.event(&mut host, "some_future_event", json!({}));
    let out = run(
        &mut e,
        &mut host,
        "print(seen)\nfor cb in (on_saved, broken): event.DISPATCHER.disconnect(event.ProjectSaved, cb)\nevent.DISPATCHER.disconnect(event.Event, on_any)",
    );
    assert_eq!(
        out,
        "[('saved', 'C:/p.ret'), ('any', 'project_saved'), ('any', 'some_future_event')]\n"
    );
    // A failing callback is logged and does not stop the others.
    assert!(
        host.logs
            .iter()
            .any(|(l, t)| *l == LogLevel::Error && t.contains("KeyError: 'callback bug'")),
        "{:?}",
        host.logs
    );
}

#[test]
fn ui_actions_and_timers() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    run(
        &mut e,
        &mut host,
        r#"
from reetched import ui, logging
ticks = []
handle = ui.add_action("Console Tools/Greet", lambda: print("greetings"))
ui.call_later(0, lambda: logging.warning("timer fired"))
ui.call_later(3600, lambda: ticks.append("too early"))
every = ui.call_every(0, lambda: ticks.append("tick"))
"#,
    );
    let action = e
        .actions()
        .into_iter()
        .find(|a| a.label == "Console Tools/Greet")
        .expect("action registered");
    assert_eq!(action.owner, None);
    let r = e.trigger_action(&mut host, action.id);
    assert_eq!((r.output.as_str(), r.error), ("greetings\n", None));

    e.tick(&mut host);
    e.tick(&mut host);
    assert!(
        host.logs
            .contains(&(LogLevel::Warning, "timer fired".into())),
        "{:?}",
        host.logs
    );
    assert_eq!(
        host.logs.iter().filter(|(_, t)| t == "timer fired").count(),
        1
    );
    assert_eq!(
        run(
            &mut e,
            &mut host,
            "ui.cancel(every); ui.remove_action(handle); ticks"
        ),
        "['tick', 'tick']\n"
    );
    assert!(e.actions().iter().all(|a| a.label != "Console Tools/Greet"));
}

#[test]
fn print_goes_to_the_log_outside_the_console() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    run(
        &mut e,
        &mut host,
        "import sys\nfrom reetched import ui\nui.call_later(0, lambda: (print('out'), print('err', file=sys.stderr)))",
    );
    e.tick(&mut host);
    assert!(
        host.logs.contains(&(LogLevel::Info, "out".into())),
        "{:?}",
        host.logs
    );
    assert!(
        host.logs.contains(&(LogLevel::Error, "err".into())),
        "{:?}",
        host.logs
    );
}

#[test]
fn host_is_unreachable_from_other_threads() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    let out = run(
        &mut e,
        &mut host,
        r#"
import threading, reetched
result = []
def worker():
    try:
        reetched.project.name()
    except RuntimeError as err:
        result.append("refused")
t = threading.Thread(target=worker); t.start(); t.join()
print(result)
"#,
    );
    assert_eq!(out, "['refused']\n");
    assert!(host.calls.is_empty());
}

#[test]
fn plugins_start_stop_and_reload() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    let names: Vec<String> = e.plugins().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["broken_plugin", "hello_plugin", "pkg_plugin"]);

    e.set_plugin_enabled(&mut host, "hello_plugin", true)
        .unwrap();
    e.set_plugin_enabled(&mut host, "pkg_plugin", true).unwrap();
    assert!(host.log_text().contains("hello_plugin started\n"));
    assert!(host.log_text().contains("pkg_plugin started\n"));
    let info = e
        .plugins()
        .into_iter()
        .find(|p| p.name == "hello_plugin")
        .unwrap();
    assert!(info.enabled && info.started && info.error.is_none());

    let hello: Vec<_> = e
        .actions()
        .into_iter()
        .filter(|a| a.label == "Test Plugin/Say Hello")
        .collect();
    assert_eq!(hello.len(), 1);
    assert_eq!(hello[0].owner.as_deref(), Some("hello_plugin"));
    assert_eq!(
        e.trigger_action(&mut host, hello[0].id).output,
        "hello from plugin\n"
    );

    e.event(&mut host, "project_saved", json!({ "path": "C:/x.ret" }));
    assert!(
        host.log_text()
            .contains("hello_plugin saw a save to C:/x.ret\n")
    );

    host.logs.clear();
    e.reload_plugin(&mut host, "hello_plugin").unwrap();
    assert_eq!(
        host.log_text(),
        "hello_plugin closed\nhello_plugin started\n"
    );
    assert_eq!(
        run(
            &mut e,
            &mut host,
            "import builtins; builtins._hello_plugin_loads"
        ),
        "2\n"
    );
    // Its old action and listener were dropped on close, so there is exactly one of each.
    assert_eq!(
        e.actions()
            .iter()
            .filter(|a| a.label == "Test Plugin/Say Hello")
            .count(),
        1
    );
    host.logs.clear();
    e.event(&mut host, "project_saved", json!({ "path": "C:/y.ret" }));
    assert_eq!(host.log_text(), "hello_plugin saw a save to C:/y.ret\n");

    e.set_plugin_enabled(&mut host, "hello_plugin", false)
        .unwrap();
    assert!(
        e.actions()
            .iter()
            .all(|a| a.owner.as_deref() != Some("hello_plugin"))
    );
    let info = e
        .plugins()
        .into_iter()
        .find(|p| p.name == "hello_plugin")
        .unwrap();
    assert!(!info.enabled && !info.started);

    let err = e
        .set_plugin_enabled(&mut host, "broken_plugin", true)
        .unwrap_err();
    assert!(
        err.contains("broken on purpose") && err.contains("line 2"),
        "{err}"
    );
    assert!(!err.contains("<embedded>"), "engine frames leaked:\n{err}");
    let info = e
        .plugins()
        .into_iter()
        .find(|p| p.name == "broken_plugin")
        .unwrap();
    assert!(info.enabled && !info.started && info.error.is_some());
    e.set_plugin_enabled(&mut host, "broken_plugin", false)
        .unwrap();
    e.set_plugin_enabled(&mut host, "pkg_plugin", false)
        .unwrap();
    assert!(e.set_plugin_enabled(&mut host, "missing", true).is_err());
}

#[test]
fn painter_style_script_runs_on_the_shim() {
    let mut e = engine_or_skip!();
    let mut host = MockHost::new();
    let out = run(
        &mut e,
        &mut host,
        r#"
import substance_painter.layerstack
import substance_painter.textureset
import substance_painter.project
import substance_painter.event
import substance_painter.logging
from substance_painter.colormanagement import Color

if substance_painter.project.is_open():
    stack = substance_painter.textureset.get_active_stack()
    res = stack.material().get_resolution()
    print(stack.material().name(), res.width, res.height)
    position = substance_painter.layerstack.InsertPosition.from_textureset_stack(stack)
    with substance_painter.layerstack.ScopedModification("Add fill"):
        fill = substance_painter.layerstack.insert_fill(position)
        fill.set_name("Painter Fill")
        fill.active_channels = {substance_painter.textureset.ChannelType.BaseColor}
        fill.set_source(substance_painter.textureset.ChannelType.BaseColor, Color(1.0, 0.0, 0.0))
        fill.set_blending_mode(substance_painter.layerstack.BlendingMode.Overlay,
                               substance_painter.textureset.ChannelType.BaseColor)
    group = substance_painter.layerstack.insert_group(
        substance_painter.layerstack.InsertPosition.above_node(fill))
    group.set_name("Folder")
    for node in substance_painter.layerstack.get_root_layer_nodes(stack):
        print(node.get_name(), node.get_type())
    substance_painter.logging.info("done")

got = []
substance_painter.event.DISPATCHER.connect(substance_painter.event.ExportTexturesEnded,
                                           lambda e: got.append(e.textures))
substance_painter.event.DISPATCHER.connect(substance_painter.event.ProjectEditionEntered,
                                           lambda e: got.append("entered"))
try:
    substance_painter.js.evaluate("alg.project.name()")
except NotImplementedError:
    print("no js")
"#,
    );
    assert_eq!(
        out,
        "Body 2048 2048\nFolder NodeType.GroupLayer\nPainter Fill NodeType.FillLayer\nno js\n"
    );
    assert_eq!(host.logs, [(LogLevel::Info, "done".to_string())]);
    let fill = host.node(1).unwrap();
    assert_eq!(fill["fill"]["basecolor"], json!([1.0, 0.0, 0.0]));
    assert_eq!(fill["blending"]["basecolor"], "Overlay");
    assert_eq!(fill["active_channels"], json!(["basecolor"]));

    e.event(
        &mut host,
        "export_finished",
        json!({ "files": { "Body": ["C:/out/a.png"] } }),
    );
    e.event(&mut host, "project_opened", json!({ "path": "C:/p.ret" }));
    assert_eq!(
        run(&mut e, &mut host, "got"),
        "[{('Body', ''): ['C:/out/a.png']}, 'entered']\n"
    );
}
