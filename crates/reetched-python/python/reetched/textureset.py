"""Texture sets (one per material of the mesh) and their channels.

Channel ids: basecolor, metallic, roughness, normal, height, emissive, opacity,
specularlevel, specularedgecolor, anisotropylevel, anisotropyangle,
transmissive, scattering, scatteringcolor, coatopacity, coatcolor,
coatroughness, sheenopacity, sheencolor, sheenroughness, ambientocclusion.
Shaders: "metal_rough", "asm", "openpbr".
"""
from ._core import ReEtchedError, call

CHANNELS = (
    "basecolor", "metallic", "roughness", "normal", "height", "emissive", "opacity",
    "specularlevel", "specularedgecolor", "anisotropylevel", "anisotropyangle",
    "transmissive", "scattering", "scatteringcolor", "coatopacity", "coatcolor",
    "coatroughness", "sheenopacity", "sheencolor", "sheenroughness", "ambientocclusion",
)
SHADERS = ("metal_rough", "asm", "openpbr")


class TextureSet(object):
    """A texture set, looked up by name on every access so it never goes stale."""

    def __init__(self, name):
        self._name = name

    @property
    def name(self):
        return self._name

    def _info(self):
        for s in call("textureset.list")["sets"]:
            if s["name"] == self._name:
                return s
        raise ReEtchedError("texture set %r no longer exists" % self._name)

    def _update(self, **fields):
        call("textureset.update", texture_set=self._name, **fields)

    @property
    def resolution(self):
        """(width, height) in pixels."""
        return tuple(self._info()["resolution"])

    @resolution.setter
    def resolution(self, value):
        if isinstance(value, int):
            value = (value, value)
        self._update(resolution=[int(value[0]), int(value[1])])

    def channels(self):
        return list(self._info()["channels"])

    def has_channel(self, channel):
        return channel in self.channels()

    def add_channel(self, channel):
        self._update(add_channels=[channel])

    def remove_channel(self, channel):
        self._update(remove_channels=[channel])

    @property
    def shader(self):
        return self._info()["shader"]

    @shader.setter
    def shader(self, value):
        self._update(shader=value)

    def udim_tiles(self):
        """UDIM tile numbers (1001, 1002, ...); [1001] for a mesh without UDIMs."""
        return list(self._info()["udims"])

    def __eq__(self, other):
        return isinstance(other, TextureSet) and other._name == self._name

    def __hash__(self):
        return hash(self._name)

    def __repr__(self):
        return "TextureSet(%r)" % self._name


def all_texture_sets():
    return [TextureSet(s["name"]) for s in call("textureset.list")["sets"]]


def get_active():
    info = call("textureset.list")
    if info["active"] is None:
        return None
    return TextureSet(info["sets"][info["active"]]["name"])


def set_active(texture_set):
    call("textureset.set_active", texture_set=ref(texture_set))


def ref(texture_set):
    """What the app accepts as a texture set reference: a name or an index."""
    if isinstance(texture_set, TextureSet):
        return texture_set.name
    return texture_set
