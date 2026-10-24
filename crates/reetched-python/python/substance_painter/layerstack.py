from enum import Enum

from reetched import layerstack as _ls
from reetched.layerstack import BlendingMode
from .textureset import Stack, _channel_id, _channel_type


class NodeType(Enum):
    FillLayer = "fill"
    PaintLayer = "paint"
    GroupLayer = "group"


class NodeStack(Enum):
    Substack = 0
    Content = 1
    Mask = 2


class MaskBackground(Enum):
    White = "white"
    Black = "black"


class ScopedModification(object):
    """Context manager naming an undo step in Painter.

    ponytail: no-op; each call is still its own undo step in ReEtched.
    """

    def __init__(self, name):
        self.name = name

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class LayerNode(object):
    def __init__(self, core):
        self._core = core

    def uid(self):
        return self._core.id

    def get_type(self):
        return NodeType(self._core.kind)

    def get_stack(self):
        return Stack(self._core.texture_set)

    def get_name(self):
        return self._core.name

    def set_name(self, name):
        self._core.name = name

    def is_visible(self):
        return self._core.visible

    def set_visible(self, visible):
        self._core.visible = visible

    def get_blending_mode(self, channel_type=None):
        return self._core.blending_mode(_channel_id(channel_type or "basecolor"))

    def set_blending_mode(self, blending_mode, channel_type=None):
        self._core.set_blending_mode(blending_mode, _channel_id(channel_type or "basecolor"))

    def get_opacity(self, channel_type=None):
        return self._core.opacity(_channel_id(channel_type or "basecolor"))

    def set_opacity(self, opacity, channel_type=None):
        self._core.set_opacity(opacity, _channel_id(channel_type or "basecolor"))

    def has_mask(self):
        return self._core.has_mask()

    def add_mask(self, mask_background=MaskBackground.White):
        self._core.add_mask(mask_background.value if isinstance(mask_background, MaskBackground) else mask_background)

    def remove_mask(self):
        self._core.remove_mask()

    def get_parent(self):
        p = self._core.parent()
        return None if p is None else _wrap(p)

    def sub_layers(self):
        return [_wrap(c) for c in self._core.children()]

    def __eq__(self, other):
        return isinstance(other, LayerNode) and other._core == self._core

    def __hash__(self):
        return hash(self._core)

    def __repr__(self):
        return "%s(%d)" % (type(self).__name__, self._core.id)


class FillLayerNode(LayerNode):
    @property
    def active_channels(self):
        return {_channel_type(c) for c in self._core.active_channels}

    @active_channels.setter
    def active_channels(self, channels):
        self._core.active_channels = [_channel_id(c) for c in channels]

    def set_source(self, channel_type, source):
        """source: a float, an (r, g, b) tuple, a colormanagement.Color or a ResourceID."""
        channel = _channel_id(channel_type)
        rid = getattr(source, "_rid", None)
        if rid is not None:
            self._core.set_channel_bitmap(channel, rid)
        else:
            self._core.set_channel_value(channel, getattr(source, "value", source))

    def get_source(self, channel_type):
        return self._core.channel_value(_channel_id(channel_type))


class PaintLayerNode(LayerNode):
    pass


class GroupLayerNode(LayerNode):
    pass


_KINDS = {"fill": FillLayerNode, "paint": PaintLayerNode, "group": GroupLayerNode}


def _wrap(core):
    return _KINDS[core.kind](core)


class InsertPosition(object):
    def __init__(self, core):
        self._core = core

    @staticmethod
    def from_textureset_stack(stack):
        return InsertPosition(_ls.InsertPosition.from_texture_set_stack(stack._name))

    @staticmethod
    def above_node(node):
        return InsertPosition(_ls.InsertPosition.above_node(node._core))

    @staticmethod
    def below_node(node):
        return InsertPosition(_ls.InsertPosition.below_node(node._core))

    @staticmethod
    def inside_node(node, node_stack=NodeStack.Substack):
        if node_stack != NodeStack.Substack:
            raise NotImplementedError("only NodeStack.Substack positions are supported")
        return InsertPosition(_ls.InsertPosition.inside_node(node._core))


def insert_fill(position):
    return _wrap(_ls.insert_fill(position._core))


def insert_paint(position):
    return _wrap(_ls.insert_paint(position._core))


def insert_group(position):
    return _wrap(_ls.insert_group(position._core))


def delete_node(node):
    _ls.delete_node(node._core)


def move_node(node, position):
    _ls.move_node(node._core, position._core)


def get_root_layer_nodes(stack):
    return [_wrap(n) for n in _ls.get_root_layer_nodes(stack._name)]


def get_selected_nodes(stack=None):
    return [_wrap(n) for n in _ls.get_selected_nodes()]


def set_selected_nodes(nodes):
    _ls.set_selected_nodes([n._core for n in nodes])
