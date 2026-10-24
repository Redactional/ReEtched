"""A tiny stand-in for Painter's Python API, enough to run the bridge plugin.

It models only what the plugin touches. Its compositor is a toy (Normal and
Multiply blending, masks, groups as pass-through), so it checks the plugin's
logic and file format, not Painter's behaviour.
"""

import enum
import os
import struct
import zlib


class ChannelType(enum.Enum):
    BaseColor = 0
    Metallic = 1
    Roughness = 2
    Normal = 3
    Height = 4
    Emissive = 5


class BlendingMode(enum.Enum):
    Normal = 0
    Multiply = 1
    Passthrough = 2


class NodeType(enum.Enum):
    PaintLayer = 0
    FillLayer = 1
    GroupLayer = 2
    FillEffect = 3
    PaintEffect = 4


class MaskBackground(enum.Enum):
    White = 0
    Black = 1


class NodeStack(enum.Enum):
    Content = 0
    Mask = 1
    Substack = 2


class Color:
    def __init__(self, r, g, b):
        self.value = (r, g, b)


class SourceUniformColor:
    def __init__(self, color):
        self.color = color

    def get_color(self):
        return self.color


class ResourceID:
    def __init__(self, url):
        self._url = url

    def url(self):
        return self._url


class SourceSubstance:
    def __init__(self, url, params, image):
        self.resource_id = ResourceID(url)
        self.params = params
        self.image = image  # channel -> per-pixel rgb (what it renders)

    def get_parameters(self):
        return dict(self.params)


_uid = [100]


class Node:
    def __init__(self, kind, name, channels):
        _uid[0] += 1
        self._uid = _uid[0]
        self.kind = kind
        self.name = name
        self.visible = True
        self.blend = dict((ct, [BlendingMode.Normal, 1.0]) for ct in channels)
        self.mask = None  # {"background", "enabled", "paint": [m per pixel] or None}
        self.content = {}  # paint: ct -> [(rgb, a) per pixel]
        self.sources = {}  # fill: ct -> source
        self.active_channels = set(channels)
        self.effects = []  # content fill effects, top first
        self.children = []

    def uid(self):
        return self._uid

    def get_type(self):
        return self.kind

    def get_name(self):
        return self.name

    def is_visible(self):
        return self.visible

    def set_visible(self, v):
        self.visible = v

    def get_blending_mode(self, ct):
        return self.blend[ct][0]

    def set_blending_mode(self, m, ct):
        self.blend[ct][0] = m

    def get_opacity(self, ct):
        return self.blend[ct][1]

    def set_opacity(self, v, ct):
        self.blend[ct][1] = v

    def sub_layers(self):
        return list(self.children)

    def has_mask(self):
        return self.mask is not None

    def get_mask_background(self):
        return self.mask["background"]

    def is_mask_enabled(self):
        return self.mask["enabled"]

    def enable_mask(self, v):
        self.mask["enabled"] = v

    def mask_effects(self):
        return ["paint"] if self.mask and self.mask["paint"] else []

    def content_effects(self):
        return list(self.effects)

    def get_source(self, ct):
        return self.sources.get(ct)

    def set_source(self, ct, value):
        self.sources[ct] = SourceUniformColor(value) if isinstance(value, Color) else value


class Resolution:
    def __init__(self, w, h):
        self.width, self.height = w, h


class Stack:
    def __init__(self, channels):
        self.channels = channels
        self.roots = []

    def all_channels(self):
        return dict((ct, ct.name) for ct in self.channels)


class TextureSet:
    def __init__(self, name, size, channels):
        self._name = name
        self.size = size
        self.stack = Stack(channels)
        self.mesh_maps = {}  # export name -> [rgb per pixel]

    def name(self):
        return self._name

    def has_uv_tiles(self):
        return False

    def all_stacks(self):
        return [self.stack]

    def get_resolution(self):
        return Resolution(self.size, self.size)


class Scene:
    texture_sets = []
    mesh_path = None
    exports = 0


scene = Scene()

# -- textureset / project / application ------------------------------------


def all_texture_sets():
    return list(scene.texture_sets)


def is_open():
    return True


def last_imported_mesh_path():
    return scene.mesh_path


def file_path():
    return "C:/projects/demo.spp"


def version():
    return "fake"


# -- layerstack -------------------------------------------------------------


def _find(nodes, uid):
    for i, n in enumerate(nodes):
        if n.uid() == uid:
            return nodes, i
        if n.children:
            r = _find(n.children, uid)
            if r:
                return r
    return None


def _owner(node):
    for ts in scene.texture_sets:
        r = _find(ts.stack.roots, node.uid())
        if r:
            return r
    raise KeyError(node.name)


class InsertPosition:
    def __init__(self, place):
        self.place = place

    @staticmethod
    def below_node(node):
        return InsertPosition(("below", node))

    @staticmethod
    def inside_node(node, where):
        return InsertPosition(("inside", node, where))


def get_root_layer_nodes(stack):
    return list(stack.roots)


def insert_fill(position):
    place = position.place
    if place[0] == "below":
        parent, i = _owner(place[1])
        fill = Node(NodeType.FillLayer, "temp fill", list(ChannelType))
        fill.active_channels = set()
        parent.insert(i + 1, fill)
    elif place[2] == NodeStack.Substack:
        fill = Node(NodeType.FillLayer, "temp fill", list(ChannelType))
        fill.active_channels = set()
        place[1].children.insert(0, fill)
    else:
        fill = Node(NodeType.FillEffect, "temp effect", list(ChannelType))
        fill.active_channels = set()
        place[1].effects.insert(0, fill)
    return fill


def delete_node(node):
    for ts in scene.texture_sets:
        for holder in _all_nodes(ts.stack.roots):
            if node in holder.effects:
                holder.effects.remove(node)
                return
    parent, i = _owner(node)
    parent.pop(i)


def _all_nodes(nodes):
    for n in nodes:
        yield n
        yield from _all_nodes(n.children)


class ScopedModification:
    def __init__(self, name):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


# -- toy compositor -----------------------------------------------------------


def _encode(ct, rgb):
    """Uniform colors are linear (like Painter's Color); base color is stored sRGB-encoded."""
    if ct != ChannelType.BaseColor:
        return rgb

    def enc(v):
        return v * 12.92 if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055

    return tuple(enc(v) for v in rgb)


def _content(node, ct, n):
    """[(rgb, a)] per pixel of a node's own content for channel ct."""
    if node.effects:
        top = node.effects[0]
        if ct in top.active_channels:
            return [(_encode(ct, top.sources[ct].get_color().value), 1.0)] * n
    if node.kind == NodeType.PaintLayer:
        return node.content.get(ct, [((0, 0, 0), 0.0)] * n)
    if ct not in node.active_channels:
        return [((0, 0, 0), 0.0)] * n
    src = node.sources.get(ct)
    if isinstance(src, SourceUniformColor):
        return [(_encode(ct, src.get_color().value), 1.0)] * n
    if isinstance(src, SourceSubstance):
        return [(rgb, 1.0) for rgb in src.image[ct]]
    return [((0, 0, 0), 0.0)] * n


def _mask(node, n):
    if not node.mask or not node.mask["enabled"]:
        return [1.0] * n
    if node.mask["paint"]:
        return list(node.mask["paint"])
    return [1.0 if node.mask["background"] == MaskBackground.White else 0.0] * n


def _composite(nodes, ct, dst):
    n = len(dst)
    for node in reversed(nodes):  # bottom first
        if not node.visible:
            continue
        mode, opacity = node.blend.get(ct, [BlendingMode.Normal, 1.0])
        m = _mask(node, n)
        if node.kind == NodeType.GroupLayer:
            inner = _composite(node.children, ct, list(dst))
            dst = [tuple(d[k] + (i[k] - d[k]) * opacity * m[p] for k in range(3)) for p, (d, i) in enumerate(zip(dst, inner))]
            continue
        src = _content(node, ct, n)
        out = []
        for p, (d, (c, a)) in enumerate(zip(dst, src)):
            t = a * m[p] * opacity
            f = tuple(d[k] * c[k] for k in range(3)) if mode == BlendingMode.Multiply else c
            out.append(tuple(d[k] + (f[k] - d[k]) * t for k in range(3)))
        dst = out
    return dst


def _write_png16(path, size, pixels):
    rows = b""
    for y in range(size):
        rows += b"\x00"
        for x in range(size):
            for v in pixels[y * size + x]:
                rows += struct.pack(">H", max(0, min(65535, round(v * 65535))))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 16, 2, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(data)


_DOC_MAPS = {"basecolor": ChannelType.BaseColor, "metallic": ChannelType.Metallic, "roughness": ChannelType.Roughness, "normal": ChannelType.Normal, "height": ChannelType.Height}


# What an empty stack exports (captures must not depend on it).
_EMPTY = {
    ChannelType.BaseColor: (0.5, 0.5, 0.5),
    ChannelType.Metallic: (0.0, 0.0, 0.0),
    ChannelType.Roughness: (0.5, 0.5, 0.5),
    ChannelType.Normal: (0.5, 0.5, 1.0),
    ChannelType.Height: (0.0, 0.0, 0.0),
}


class ExportResult:
    def __init__(self, textures):
        self.textures = textures
        self.status = "Success"


def export_project_textures(config):
    scene.exports += 1
    preset = config["exportPresets"][0]
    out = config["exportPath"]
    files = {}
    for entry in config["exportList"]:
        ts = next(t for t in scene.texture_sets if t.name() == entry["rootPath"])
        n = ts.size * ts.size
        paths = []
        for m in preset["maps"]:
            src = m["channels"][0]
            if src["srcMapType"] == "documentMap":
                ct = _DOC_MAPS[src["srcMapName"]]
                pixels = _composite(ts.stack.roots, ct, [_EMPTY[ct]] * n)
            elif src["srcMapName"] in ts.mesh_maps:
                pixels = ts.mesh_maps[src["srcMapName"]]
            else:
                continue
            path = os.path.join(out, m["fileName"].replace("$textureSet", ts.name()) + ".png")
            _write_png16(path, ts.size, pixels)
            paths.append(path.replace("\\", "/"))
        files[(ts.name(), "")] = paths
    return ExportResult(files)


# -- ui ------------------------------------------------------------------------


class ApplicationMenu(enum.Enum):
    File = 0


def add_action(menu, action):
    pass


def delete_ui_element(e):
    pass


def get_main_window():
    return None
