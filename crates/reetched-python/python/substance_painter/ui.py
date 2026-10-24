"""Menu basics only: there is no Qt in ReEtched, so widgets and docks are unavailable."""
from enum import Enum

from reetched import ui as _ui


class ApplicationMenu(Enum):
    File = "File"
    Edit = "Edit"
    Mode = "Mode"
    Window = "Window"
    Viewport = "Viewport"
    Help = "Help"


def add_action(menu, action, callback=None):
    """add_action(menu, action) with any object that has text() and trigger()
    (a QAction-like object), or add_action(menu, "Label", callback).
    The entry appears under Python > <menu name>. Returns a handle."""
    if callback is None:
        label, callback = action.text(), action.trigger
    else:
        label = str(action)
    return _ui.add_action("%s/%s" % (getattr(menu, "value", menu), label), callback)


def delete_ui_element(element):
    _ui.remove_action(element)


def _no_qt(*args, **kwargs):
    raise NotImplementedError("Qt widgets are not available in ReEtched")


add_dock_widget = add_toolbar = add_plugins_toolbar_widget = get_main_window = _no_qt
