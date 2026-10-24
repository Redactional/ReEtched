from enum import Enum

from reetched import textureset as _ts


class ChannelType(Enum):
    # Value = the ReEtched channel id. Painter channels ReEtched lacks map to
    # ids the app rejects with an error when used.
    BaseColor = "basecolor"
    Metallic = "metallic"
    Roughness = "roughness"
    Normal = "normal"
    Height = "height"
    Emissive = "emissive"
    Opacity = "opacity"
    Specularlevel = "specularlevel"
    SpecularEdgeColor = "specularedgecolor"
    Anisotropylevel = "anisotropylevel"
    Anisotropyangle = "anisotropyangle"
    Transmissive = "transmissive"
    Scattering = "scattering"
    ScatterColor = "scatteringcolor"
    CoatOpacity = "coatopacity"
    CoatColor = "coatcolor"
    CoatRoughness = "coatroughness"
    SheenOpacity = "sheenopacity"
    SheenColor = "sheencolor"
    SheenRoughness = "sheenroughness"
    AO = "ambientocclusion"
    Displacement = "displacement"
    Glossiness = "glossiness"
    Specular = "specular"
    Diffuse = "diffuse"
    Reflection = "reflection"
    Ior = "ior"


class ChannelFormat(Enum):
    sRGB8 = 0
    L8 = 1
    RGB8 = 2
    L16 = 3
    RGB16 = 4
    L16F = 5
    RGB16F = 6
    L32F = 7
    RGB32F = 8


class Resolution(object):
    def __init__(self, width, height):
        self.width = int(width)
        self.height = int(height)

    def __eq__(self, other):
        return isinstance(other, Resolution) and (other.width, other.height) == (self.width, self.height)

    def __repr__(self):
        return "Resolution(%d, %d)" % (self.width, self.height)


def _channel_id(channel_type):
    return channel_type.value if isinstance(channel_type, ChannelType) else str(channel_type)


def _channel_type(channel_id):
    try:
        return ChannelType(channel_id)
    except ValueError:
        return channel_id


class Channel(object):
    def __init__(self, channel_type):
        self._type = channel_type

    def type(self):
        return self._type

    def __repr__(self):
        return "Channel(%r)" % (self._type,)


class TextureSet(object):
    def __init__(self, name):
        self._ts = _ts.TextureSet(name)

    def name(self):
        return self._ts.name

    def get_resolution(self):
        return Resolution(*self._ts.resolution)

    def set_resolution(self, resolution):
        self._ts.resolution = (resolution.width, resolution.height)

    def all_stacks(self):
        return [Stack(self._ts.name)]

    def get_stack(self, layered_stack_name=""):
        # ReEtched texture sets have exactly one stack.
        return Stack(self._ts.name)

    def is_layered_material(self):
        return False

    def has_uv_tiles(self):
        return self._ts.udim_tiles() != [1001]

    def all_uv_tiles(self):
        return self._ts.udim_tiles()

    def __eq__(self, other):
        return isinstance(other, TextureSet) and other.name() == self.name()

    def __repr__(self):
        return "TextureSet(%r)" % self.name()


class Stack(object):
    def __init__(self, texture_set_name):
        self._name = texture_set_name

    @staticmethod
    def from_name(texture_set_name, stack_name=""):
        return Stack(texture_set_name)

    def name(self):
        return ""

    def material(self):
        return TextureSet(self._name)

    def all_channels(self):
        return {_channel_type(c): Channel(_channel_type(c)) for c in _ts.TextureSet(self._name).channels()}

    def has_channel(self, channel_type):
        return _channel_id(channel_type) in _ts.TextureSet(self._name).channels()

    def get_channel(self, channel_type):
        return Channel(channel_type)

    def add_channel(self, channel_type, channel_format=None, label=None):
        _ts.TextureSet(self._name).add_channel(_channel_id(channel_type))
        return Channel(channel_type)

    def remove_channel(self, channel_type):
        _ts.TextureSet(self._name).remove_channel(_channel_id(channel_type))

    def __eq__(self, other):
        return isinstance(other, Stack) and other._name == self._name

    def __repr__(self):
        return "Stack(%r)" % self._name


def all_texture_sets():
    return [TextureSet(t.name) for t in _ts.all_texture_sets()]


def get_active_stack():
    active = _ts.get_active()
    return None if active is None else Stack(active.name)


def set_active_stack(stack):
    _ts.set_active(stack._name)
