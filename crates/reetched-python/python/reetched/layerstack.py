"""Layer stacks: fill, paint and group layers of a texture set.

    from reetched import layerstack, textureset
    ts = textureset.get_active()
    fill = layerstack.insert_fill(layerstack.InsertPosition.from_texture_set_stack(ts))
    fill.name = "Rust"
    fill.active_channels = ["basecolor", "roughness"]
    fill.set_channel_value("basecolor", (0.4, 0.12, 0.05))
    fill.set_blending_mode(layerstack.BlendingMode.Multiply, "basecolor")

Node objects hold a texture set name and a node id; every property reads the
app's current state, so they never go stale (but a deleted node raises).
"""
from enum import Enum

from ._core import ReEtchedError, call
from . import textureset as _textureset


class BlendingMode(str, Enum):
    Normal = "Normal"
    Passthrough = "Passthrough"
    Disable = "Disable"
    Replace = "Replace"
    Multiply = "Multiply"
    Divide = "Divide"
    InverseDivide = "InverseDivide"
    Darken = "Darken"
    Lighten = "Lighten"
    LinearDodge = "LinearDodge"
    Subtract = "Subtract"
    InverseSubtract = "InverseSubtract"
    Difference = "Difference"
    Exclusion = "Exclusion"
    SignedAddition = "SignedAddition"
    Overlay = "Overlay"
    Screen = "Screen"
    LinearBurn = "LinearBurn"
    ColorBurn = "ColorBurn"
    ColorDodge = "ColorDodge"
    SoftLight = "SoftLight"
    HardLight = "HardLight"
    VividLight = "VividLight"
    LinearLight = "LinearLight"
    PinLight = "PinLight"
    Tint = "Tint"
    Saturation = "Saturation"
    Color = "Color"
    Value = "Value"
    NormalMapCombine = "NormalMapCombine"
    NormalMapDetail = "NormalMapDetail"
    NormalMapInverseDetail = "NormalMapInverseDetail"


def _find(nodes, node_id):
    for n in nodes:
        if n["id"] == node_id:
            return n
        found = _find(n.get("children") or [], node_id)
        if found is not None:
            return found
    return None


def _tree(texture_set):
    return call("layers.tree", texture_set=texture_set)["nodes"]


class LayerNode(object):
    kind = None

    def __init__(self, texture_set, node_id):
        self.texture_set = texture_set  # name
        self.id = node_id

    def _data(self):
        d = _find(_tree(self.texture_set), self.id)
        if d is None:
            raise ReEtchedError("layer %d no longer exists" % self.id)
        return d

    def _update(self, **fields):
        call("layers.update", id=self.id, **fields)

    @property
    def name(self):
        return self._data()["name"]

    @name.setter
    def name(self, value):
        self._update(name=str(value))

    @property
    def visible(self):
        return self._data()["visible"]

    @visible.setter
    def visible(self, value):
        self._update(visible=bool(value))

    def blending_mode(self, channel):
        return BlendingMode(self._data()["blending"].get(channel, "Normal"))

    def set_blending_mode(self, mode, channel):
        self._update(blending={channel: BlendingMode(mode).value})

    def opacity(self, channel):
        return self._data()["opacity"].get(channel, 1.0)

    def set_opacity(self, value, channel):
        self._update(opacity={channel: float(value)})

    def has_mask(self):
        return self._data()["has_mask"]

    def add_mask(self, background="white"):
        """Adds a mask filled with "white" (everything visible) or "black"."""
        self._update(mask=str(getattr(background, "value", background)).lower())

    def remove_mask(self):
        self._update(mask="none")

    def children(self):
        return []

    def parent(self):
        """The group containing this node, or None at the root."""
        def walk(nodes, parent):
            for n in nodes:
                if n["id"] == self.id:
                    return parent
                found = walk(n.get("children") or [], n)
                if found is not False:
                    return found
            return False

        p = walk(_tree(self.texture_set), None)
        return None if not p else GroupLayerNode(self.texture_set, p["id"])

    def __eq__(self, other):
        return isinstance(other, LayerNode) and other.id == self.id

    def __hash__(self):
        return hash(self.id)

    def __repr__(self):
        return "%s(%r, %d)" % (type(self).__name__, self.texture_set, self.id)


class FillLayerNode(LayerNode):
    kind = "fill"

    @property
    def active_channels(self):
        return list(self._data()["active_channels"])

    @active_channels.setter
    def active_channels(self, channels):
        self._update(active_channels=list(channels))

    def channel_value(self, channel):
        """A float, an (r, g, b) tuple, a {"resource": id} dict, or None if unset."""
        v = self._data()["fill"].get(channel)
        return tuple(v) if isinstance(v, list) else v

    def set_channel_value(self, channel, value):
        """value: a float (grayscale channels) or linear (r, g, b)."""
        if isinstance(value, (int, float)):
            value = float(value)
        else:
            value = [float(c) for c in value]
        self._update(fill={channel: value})

    def set_channel_bitmap(self, channel, resource):
        rid = resource if isinstance(resource, int) else resource.id
        self._update(fill={channel: {"resource": rid}})


class PaintLayerNode(LayerNode):
    kind = "paint"


class GroupLayerNode(LayerNode):
    kind = "group"

    def children(self):
        return [_wrap(self.texture_set, c) for c in self._data().get("children") or []]


_KINDS = {"fill": FillLayerNode, "paint": PaintLayerNode, "group": GroupLayerNode}


def _wrap(texture_set, data):
    return _KINDS[data["kind"]](texture_set, data["id"])


class InsertPosition(object):
    """Where a new or moved layer goes. Build it with one of the static methods."""

    def __init__(self, texture_set, parent=None, above=None, below=None):
        self.texture_set = texture_set
        self.parent = parent
        self.above = above
        self.below = below

    @staticmethod
    def from_texture_set_stack(texture_set):
        """Top of the texture set's root stack."""
        return InsertPosition(_textureset.ref(texture_set))

    @staticmethod
    def above_node(node):
        return InsertPosition(node.texture_set, above=node.id)

    @staticmethod
    def below_node(node):
        return InsertPosition(node.texture_set, below=node.id)

    @staticmethod
    def inside_node(group):
        """Top of a group's content."""
        return InsertPosition(group.texture_set, parent=group.id)

    def _args(self):
        return {"parent": self.parent, "above": self.above, "below": self.below}


def _insert(kind, position):
    r = call("layers.insert", texture_set=position.texture_set, kind=kind, **position._args())
    return _KINDS[kind](position.texture_set, r["id"])


def insert_fill(position):
    return _insert("fill", position)


def insert_paint(position):
    return _insert("paint", position)


def insert_group(position):
    return _insert("group", position)


def delete_node(node):
    call("layers.delete", id=node.id)


def move_node(node, position):
    call("layers.move", id=node.id, **position._args())


def get_root_layer_nodes(texture_set):
    name = _textureset.ref(texture_set)
    return [_wrap(name, d) for d in _tree(name)]


def get_selected_nodes():
    """Selected layers of the active texture set (top first)."""
    sel = call("layers.selection")
    if sel["texture_set"] is None:
        return []
    nodes = _tree(sel["texture_set"])
    return [_wrap(sel["texture_set"], _find(nodes, i)) for i in sel["ids"] if _find(nodes, i)]


def set_selected_nodes(nodes):
    call("layers.select", ids=[n.id for n in nodes])
