"""Compatibility layer: the most-used parts of Substance 3D Painter's Python
API, implemented on top of `reetched`, so small Painter scripts run unchanged.
Anything Qt-based (widgets, docks, QAction menus) and JavaScript are not available.
"""
from . import exception, colormanagement, project, textureset, layerstack, resource, export, event, logging, ui, js
