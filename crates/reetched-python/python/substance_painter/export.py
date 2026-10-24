from enum import Enum

from reetched import ReEtchedError
from reetched import export as _e


class ExportStatus(Enum):
    Success = 0
    Cancelled = 1
    Warning = 2
    Error = 3


class TextureExportResult(object):
    def __init__(self, status, message, textures):
        self.status = status
        self.message = message
        #: {(texture set, stack): [file paths]}; the stack is always "".
        self.textures = textures


class ResourceExportPreset(object):
    def __init__(self, name):
        self.name = name
        self.resource_id = type("ResourceID", (), {"name": name, "url": lambda self: "resource://export/" + name})()


def list_resource_export_presets():
    return [ResourceExportPreset(n) for n in _e.list_presets()]


def _preset_name(preset):
    # Painter configs often name presets by URL ("resource://starter_assets/Unreal Engine").
    preset = getattr(preset, "name", preset)
    return preset.rsplit("/", 1)[-1] if isinstance(preset, str) else preset


def export_project_textures(json_config):
    # ponytail: custom "exportPresets" map definitions are not supported; use a named preset.
    config = dict(json_config)
    config["defaultExportPreset"] = _preset_name(config.get("defaultExportPreset"))
    try:
        result = _e.export_project_textures(config)
    except ReEtchedError as e:
        return TextureExportResult(ExportStatus.Error, str(e), {})
    textures = {(ts, ""): files for ts, files in result.files.items()}
    return TextureExportResult(ExportStatus.Success, "", textures)


def export_mesh(file_path, option=None):
    _e.export_mesh(file_path)
