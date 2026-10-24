"""Menu actions and timers.

add_action("My Tools/Make Grunge", fn) puts "Make Grunge" in a "My Tools"
submenu of the app's Python menu. Timers run on the app's UI thread between
frames, so their callbacks may call any reetched function.
"""
import itertools
import time

from . import _core

_ids = itertools.count(1)
_actions = {}  # id -> {"label", "callback", "owner"}
_timers = {}  # id -> {"due", "interval", "callback", "owner"}


def add_action(menu_label, callback):
    """Adds a Python menu entry; '/' in the label makes submenus. Returns a handle."""
    handle = next(_ids)
    _actions[handle] = {"label": str(menu_label), "callback": callback, "owner": _core.current_owner}
    return handle


def remove_action(handle):
    _actions.pop(handle, None)


def call_later(seconds, callback):
    """Runs callback() once after `seconds`. Returns a handle for cancel()."""
    return _add_timer(seconds, None, callback)


def call_every(seconds, callback):
    """Runs callback() every `seconds` until cancel(handle)."""
    return _add_timer(seconds, max(float(seconds), 0.0), callback)


def cancel(handle):
    _timers.pop(handle, None)


def _add_timer(seconds, interval, callback):
    handle = next(_ids)
    _timers[handle] = {
        "due": time.monotonic() + max(float(seconds), 0.0),
        "interval": interval,
        "callback": callback,
        "owner": _core.current_owner,
    }
    return handle


def _run_due_timers():
    now = time.monotonic()
    for handle, t in list(_timers.items()):
        if t["due"] > now or _timers.get(handle) is not t:
            continue
        if t["interval"] is None:
            del _timers[handle]
        else:
            # ponytail: frame-granular; a slow frame runs a periodic timer once, not catch-up.
            t["due"] = now + t["interval"]
        try:
            _core.invoke(t["owner"], t["callback"])
        except Exception:
            _core.report_exception("Error in timer callback (%s):" % (t["owner"] or "script"))


def _list_actions():
    return [{"id": h, "label": a["label"], "owner": a["owner"]} for h, a in _actions.items()]


def _forget_owner(owner):
    for table in (_actions, _timers):
        for handle in [h for h, v in table.items() if v["owner"] == owner]:
            del table[handle]
