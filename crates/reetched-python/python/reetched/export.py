"""Texture and mesh export.

    from reetched import export
    result = export.export_project_textures({
        "exportPath": "C:/out",
        "defaultExportPreset": "Unreal Engine (Packed)",
        "exportList": [{"rootPath": "Body"}],
        "exportParameters": [{"parameters": {"fileFormat": "png", "bitDepth": "8",
                                             "paddingAlgorithm": "infinite"}}],
    })
    print(result.all_files())
"""
import os

from ._core import call


class ExportResult(object):
    def __init__(self, files):
        #: {texture set name: [written file paths]}
        self.files = files

    def all_files(self):
        return [f for paths in self.files.values() for f in paths]

    def __repr__(self):
        return "ExportResult(%r)" % self.files


def list_presets():
    """Names of the export presets the app knows."""
    return list(call("export.presets")["presets"])


def _parameters(config):
    # Accepts a plain dict or a list of {"filter": ..., "parameters": {...}} entries.
    # ponytail: per-filter parameters are merged into one set; filters are ignored.
    params = config.get("exportParameters") or {}
    if isinstance(params, dict):
        return dict(params.get("parameters", params))
    merged = {}
    for entry in params:
        merged.update(entry.get("parameters", {}))
    return merged


def export_project_textures(config):
    """Exports with a Painter-style config dict. Returns an ExportResult."""
    params = _parameters(config)
    preset = config.get("defaultExportPreset") or config.get("preset")
    sets = [entry["rootPath"] for entry in config.get("exportList") or []]
    bit_depth = params.get("bitDepth")
    dilation = params.get("dilationDistance")
    r = call(
        "export.textures",
        path=os.fspath(config["exportPath"]),
        preset=preset,
        texture_sets=sets or None,
        format=params.get("fileFormat"),
        bit_depth=None if bit_depth is None else int(bit_depth),
        padding=params.get("paddingAlgorithm"),
        dilation=None if dilation is None else int(dilation),
    )
    return ExportResult(r["files"])


def export_mesh(path, format=None):
    """Writes the project mesh. format: "obj", "fbx", "gltf", "glb", "usd"... (None: from the extension)."""
    call("export.mesh", path=os.fspath(path), format=format)
