"""ReEtched Python API.

Modules: project, textureset, layerstack, resource, export, baking, display,
event, logging, ui. All calls run on the app's UI thread; from another thread
use ui.call_later() to get back onto it.
"""
from ._core import ReEtchedError
from . import project, textureset, layerstack, resource, export, baking, display, event, logging, ui

__version__ = "0.1.0"
