class Color(object):
    """A linear RGB color, as accepted by FillLayerNode.set_source()."""

    def __init__(self, r, g, b):
        self.value = (float(r), float(g), float(b))

    def __repr__(self):
        return "Color(%g, %g, %g)" % self.value
