"""ReEtched Bridge: exports the open Substance 3D Painter project for ReEtched.

Install: copy this file into Painter's Python plugin folder
(Documents/Adobe/Adobe Substance 3D Painter/python/plugins), then enable
"reetched_bridge" in Painter's Python menu. Use File > Export to ReEtched...

Needs Substance 3D Painter 10.0 or later (Python API 0.3, `layerstack`).

What it writes (a folder, read by ReEtched's File > Import Painter bridge):
  bridge.json        stack structure, blend modes, opacities, fill values
  mesh.<ext>         a copy of the project's mesh
  captures/...       pixel captures of layers and masks (16-bit PNG)
  maps/...           baked mesh maps
  reference/...      Painter's own export of each texture set, for checking

Painter's API does not expose brush strokes or per-layer pixels, so paint
layers, bitmap/Substance fills, effects and masks are captured by isolating a
layer and exporting it twice, over a black and a white background (masks:
with the layer's content forced to white, then black). ReEtched solves each
pair for value and coverage. The project is restored afterwards.
"""

import json
import os
import shutil
import traceback

import substance_painter as sp
import substance_painter.application
import substance_painter.colormanagement
import substance_painter.export
import substance_painter.layerstack
import substance_painter.project
import substance_painter.textureset
import substance_painter.ui

FORMAT = "reetched-bridge"
VERSION = 1

# Painter ChannelType names -> bundle names. Other channels are reported, not exported.
CHANNELS = {
    "BaseColor": "basecolor",
    "Metallic": "metallic",
    "Roughness": "roughness",
    "Normal": "normal",
    "Height": "height",
    "Emissive": "emissive",
    "Opacity": "opacity",
}

# Background values (black pass, white pass) under captured content, per channel.
BACKGROUNDS = {
    "basecolor": ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    "metallic": ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    "roughness": ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    "height": ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    "emissive": ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    "opacity": ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
    # Flat normal, then one tilted in red and green; blue carries no information.
    "normal": ([0.5, 0.5, 1.0], [1.0, 1.0, 1.0]),
}

# Export "meshMap" names -> bundle names.
MESH_MAPS = {
    "ambient_occlusion": "ambient_occlusion",
    "curvature": "curvature",
    "normal_base": "normal",
    "world_space_normals": "world_normal",
    "id": "id",
    "position": "position",
    "thickness": "thickness",
    "height": "height",
    "opacity": "opacity",
}


def _ls():
    return sp.layerstack


def _channel_types(stack):
    """(ChannelType, bundle name) for the channels the stack has that ReEtched supports."""
    out = []
    for ct in stack.all_channels().keys():
        name = CHANNELS.get(ct.name)
        if name:
            out.append((ct, name))
    return out


def _node_kind(node):
    return node.get_type().name  # PaintLayer, FillLayer, GroupLayer, InstanceLayer, ...


class Bridge:
    def __init__(self, out_dir):
        self.out = out_dir
        self.log = []
        self.exports = 0

    # -- files -------------------------------------------------------------

    def rel(self, path):
        return os.path.relpath(path, self.out).replace("\\", "/")

    def export(self, ts, stack, tag, maps):
        """Exports `maps` ([(bundle name, srcMapType, srcMapName)]) of one texture set
        into captures/<tag>/; returns {bundle name: relative path}."""
        self.exports += 1
        out_dir = os.path.join(self.out, "captures", tag)
        os.makedirs(out_dir, exist_ok=True)
        preset_maps = []
        for name, kind, src in maps:
            preset_maps.append(
                {
                    "fileName": "$textureSet_" + name,
                    "channels": [
                        {
                            "destChannel": c,
                            "srcChannel": c,
                            "srcMapType": kind,
                            "srcMapName": src,
                        }
                        for c in "RGB"
                    ],
                    "parameters": {
                        "fileFormat": "png",
                        "bitDepth": "16",
                        "dithering": False,
                        "paddingAlgorithm": "transparent",
                    },
                }
            )
        config = {
            "exportShaderParams": False,
            "exportPath": out_dir,
            "defaultExportPreset": "reetched_bridge",
            "exportPresets": [{"name": "reetched_bridge", "maps": preset_maps}],
            "exportList": [{"rootPath": ts.name()}],
        }
        result = sp.export.export_project_textures(config)
        files = [f for paths in result.textures.values() for f in paths]
        found = {}
        for name, _, _ in maps:
            suffix = "_" + name + ".png"
            match = [f for f in files if f.endswith(suffix)]
            if match:
                found[name] = self.rel(match[0])
            else:
                self.log.append("%s: export of %s produced no %s map" % (ts.name(), tag, name))
        return found

    # -- isolation ---------------------------------------------------------

    def isolate(self, stack, node, chain, channels, mask_on):
        """Shows only `node` (and its ancestors `chain`), with Normal blending, full
        opacity and ancestor masks off. Returns a function that undoes it."""
        undo = []
        keep = set(n.uid() for n in chain + [node])

        def hide_others(nodes):
            for n in nodes:
                if n.uid() in keep:
                    if n.uid() != node.uid() and _node_kind(n) == "GroupLayer":
                        hide_others(n.sub_layers())
                    undo.append((n.set_visible, n.is_visible()))
                    n.set_visible(True)
                else:
                    undo.append((n.set_visible, n.is_visible()))
                    n.set_visible(False)

        hide_others(_ls().get_root_layer_nodes(stack))
        for n in chain + [node]:
            for ct, _ in channels:
                try:
                    mode, opacity = n.get_blending_mode(ct), n.get_opacity(ct)
                except Exception:
                    continue
                undo.append((lambda v, n=n, ct=ct: n.set_blending_mode(v, ct), mode))
                undo.append((lambda v, n=n, ct=ct: n.set_opacity(v, ct), opacity))
                n.set_blending_mode(_ls().BlendingMode.Normal, ct)
                n.set_opacity(1.0, ct)
            if _has_mask(n):
                undo.append((n.enable_mask, n.is_mask_enabled()))
                n.enable_mask(mask_on and n.uid() == node.uid())

        def restore():
            for fn, value in reversed(undo):
                try:
                    fn(value)
                except Exception:
                    self.log.append("could not restore a layer setting: " + traceback.format_exc(limit=1))

        return restore

    def uniform_fill(self, position, values, channels):
        """Inserts a fill with uniform `values` ({bundle name: rgb}) at `position`."""
        fill = _ls().insert_fill(position)
        fill.active_channels = set(ct for ct, name in channels if name in values)
        for ct, name in channels:
            if name in values:
                r, g, b = values[name]
                fill.set_source(ct, sp.colormanagement.Color(r, g, b))
        return fill

    def capture_content(self, ts, stack, node, chain, channels, tag):
        restore = self.isolate(stack, node, chain, channels, mask_on=False)
        roots = _ls().get_root_layer_nodes(stack)
        maps = [(name, "documentMap", name) for _, name in channels]
        out = {}
        try:
            for which, i in (("black", 0), ("white", 1)):
                values = dict((name, BACKGROUNDS[name][i]) for _, name in channels)
                bg = self.uniform_fill(_ls().InsertPosition.below_node(roots[-1]), values, channels)
                try:
                    out[which] = self.export(ts, stack, "%s_%s" % (tag, which), maps)
                finally:
                    _ls().delete_node(bg)
        finally:
            restore()
        return out

    def capture_mask(self, ts, stack, node, chain, channels, tag):
        restore = self.isolate(stack, node, chain, channels, mask_on=True)
        out = {}
        try:
            if _node_kind(node) == "GroupLayer":
                where = _ls().InsertPosition.inside_node(node, _ls().NodeStack.Substack)
            else:
                where = _ls().InsertPosition.inside_node(node, _ls().NodeStack.Content)
            base = [(ct, name) for ct, name in channels if name == "basecolor"]
            for which, v in (("black", 0.0), ("white", 1.0)):
                over = self.uniform_fill(where, {"basecolor": [v, v, v]}, base)
                try:
                    out[which] = self.export(
                        ts, stack, "%s_mask_%s" % (tag, which), [("basecolor", "documentMap", "basecolor")]
                    )
                finally:
                    _ls().delete_node(over)
        finally:
            restore()
        return out

    # -- stack walk --------------------------------------------------------

    def node(self, ts, stack, node, chain, channels):
        kind = _node_kind(node)
        name = node.get_name()
        tag = "%s_%s" % (_safe(ts.name()), node.uid())
        d = {"name": name, "visible": node.is_visible(), "blend": {}, "notes": []}
        for ct, cname in channels:
            try:
                d["blend"][cname] = {
                    "mode": node.get_blending_mode(ct).name,
                    "opacity": float(node.get_opacity(ct)),
                }
            except Exception:
                pass

        if kind == "GroupLayer":
            d["type"] = "group"
            d["children"] = [
                self.node(ts, stack, c, chain + [node], channels) for c in node.sub_layers()
            ]
        else:
            d["type"] = "fill" if kind == "FillLayer" else "paint"
            exact = self.exact_fill(node, channels) if kind == "FillLayer" else None
            if exact is not None:
                d["fill"] = exact
            else:
                if kind not in ("PaintLayer", "FillLayer"):
                    d["notes"].append("%s imported as pixels" % kind)
                substance = self.substance_info(node, channels)
                if substance:
                    d["substance"] = substance
                d["content"] = self.capture_content(ts, stack, node, chain, channels, tag)

        if _has_mask(node):
            mask = {
                "background": node.get_mask_background().name.lower(),
                "enabled": node.is_mask_enabled(),
            }
            if node.mask_effects():
                mask["capture"] = self.capture_mask(ts, stack, node, chain, channels, tag)
            d["mask"] = mask
        if not d["notes"]:
            del d["notes"]
        return d

    def exact_fill(self, node, channels):
        """Channel values of a fill that is only uniform colors, else None."""
        if node.content_effects():
            return None
        values = {}
        active = set(ct.name for ct in node.active_channels)
        for ct, name in channels:
            if ct.name not in active:
                continue
            src = node.get_source(ct)
            if type(src).__name__ != "SourceUniformColor":
                return None
            c = src.get_color()
            values[name] = [float(x) for x in c.value[:3]]
        return values

    def substance_info(self, node, channels):
        """Resource and parameter values of Substance sources, kept for later re-rendering."""
        info = {}
        try:
            sources = [node.get_material_source()] if hasattr(node, "get_material_source") else []
        except Exception:
            sources = []
        for ct, name in channels:
            try:
                sources.append(node.get_source(ct))
            except Exception:
                pass
        for src in sources:
            if src is None or type(src).__name__ != "SourceSubstance":
                continue
            try:
                params = dict((k, _jsonable(v)) for k, v in src.get_parameters().items())
            except Exception:
                params = {}
            info[str(src.resource_id.url())] = params
        return info or None

    # -- run ---------------------------------------------------------------

    def run(self):
        if not sp.project.is_open():
            raise RuntimeError("No project is open")
        os.makedirs(self.out, exist_ok=True)
        bundle = {
            "format": FORMAT,
            "version": VERSION,
            "painter": _painter_version(),
            "mesh": self.copy_mesh(),
            "backgrounds": dict((k, [v[0], v[1]]) for k, v in BACKGROUNDS.items()),
            "texture_sets": [],
            "log": self.log,
        }
        for ts in sp.textureset.all_texture_sets():
            if ts.has_uv_tiles():
                self.log.append("%s: UDIM texture sets are not supported yet; skipped" % ts.name())
                continue
            stacks = ts.all_stacks()
            if len(stacks) > 1:
                self.log.append("%s: only the first material layer is exported" % ts.name())
            stack = stacks[0]
            channels = _channel_types(stack)
            dropped = [ct.name for ct in stack.all_channels().keys() if ct.name not in CHANNELS]
            if dropped:
                self.log.append("%s: channels not carried over: %s" % (ts.name(), ", ".join(sorted(dropped))))
            res = ts.get_resolution()
            entry = {
                "name": ts.name(),
                "resolution": [res.width, res.height],
                "mesh_maps": self.export(
                    ts, stack, _safe(ts.name()) + "_meshmaps",
                    [(b, "meshMap", m) for m, b in MESH_MAPS.items()],
                ),
                "reference": self.export(
                    ts, stack, _safe(ts.name()) + "_reference",
                    [(name, "documentMap", name) for _, name in channels],
                ),
                "layers": [
                    self.node(ts, stack, n, [], channels)
                    for n in _ls().get_root_layer_nodes(stack)
                ],
            }
            bundle["texture_sets"].append(entry)
        with open(os.path.join(self.out, "bridge.json"), "w", encoding="utf-8") as f:
            json.dump(bundle, f, indent=1)
        return bundle

    def copy_mesh(self):
        path = sp.project.last_imported_mesh_path()
        if not path or not os.path.exists(path):
            self.log.append("the project's mesh file was not found; copy it next to bridge.json as mesh.fbx")
            return None
        ext = os.path.splitext(path)[1].lower()
        dest = os.path.join(self.out, "mesh" + ext)
        shutil.copyfile(path, dest)
        return self.rel(dest)


def _has_mask(node):
    try:
        return node.has_mask()
    except Exception:
        return False


def _safe(name):
    return "".join(c if c.isalnum() or c in "-_" else "_" for c in name)


def _jsonable(v):
    if isinstance(v, (bool, int, float, str)) or v is None:
        return v
    if isinstance(v, (list, tuple)):
        return [_jsonable(x) for x in v]
    if isinstance(v, dict):
        return dict((str(k), _jsonable(x)) for k, x in v.items())
    return str(v)


def _painter_version():
    try:
        return sp.application.version()
    except Exception:
        return None


# -- Painter UI ------------------------------------------------------------

_actions = []


def _qt():
    try:
        from PySide6 import QtGui, QtWidgets
        return QtGui.QAction, QtWidgets
    except ImportError:
        from PySide2 import QtWidgets
        return QtWidgets.QAction, QtWidgets


def _export_dialog():
    _, QtWidgets = _qt()
    parent = sp.ui.get_main_window()
    folder = QtWidgets.QFileDialog.getExistingDirectory(parent, "Export to ReEtched: choose a folder")
    if not folder:
        return
    name = os.path.splitext(os.path.basename(sp.project.file_path() or "project"))[0]
    out = os.path.join(folder, _safe(name) + "_reetched")
    with sp.layerstack.ScopedModification("Export to ReEtched"):
        try:
            bundle = Bridge(out).run()
        except Exception:
            QtWidgets.QMessageBox.critical(parent, "ReEtched Bridge", traceback.format_exc())
            return
    notes = "\n".join(bundle["log"]) or "Everything was carried over."
    QtWidgets.QMessageBox.information(
        parent, "ReEtched Bridge", "Exported to %s\n\n%s\n\nOpen it in ReEtched with File > Import Painter bridge." % (out, notes)
    )


def start_plugin():
    QAction, _ = _qt()
    action = QAction("Export to ReEtched...")
    action.triggered.connect(_export_dialog)
    sp.ui.add_action(sp.ui.ApplicationMenu.File, action)
    _actions.append(action)


def close_plugin():
    for a in _actions:
        sp.ui.delete_ui_element(a)
    _actions.clear()
