"""Mesh map baking."""
import os

from ._core import call

MAPS = (
    "normal", "world_normal", "id", "ambient_occlusion", "curvature",
    "position", "thickness", "height", "opacity",
)


def bake(texture_sets=None, resolution=2048, maps=MAPS, high_poly=None):
    """Bakes mesh maps and returns when done.

    texture_sets: names (None = all). high_poly: path of a high-poly mesh
    (None = bake from the low-poly mesh itself). Returns the texture set names baked.
    """
    if texture_sets is not None:
        texture_sets = [getattr(ts, "name", ts) for ts in texture_sets]
    return call(
        "bake",
        texture_sets=texture_sets,
        resolution=int(resolution),
        maps=list(maps),
        high_poly=None if high_poly is None else os.fspath(high_poly),
    )["baked"]
