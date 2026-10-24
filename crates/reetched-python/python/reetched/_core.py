"""The single doorway to the ReEtched app.

Every module of the API ends up in call(), which sends a method name and JSON
arguments to the app and returns the decoded JSON result.
"""
import json
import traceback

import _reetched


class ReEtchedError(Exception):
    """The app could not do what a script asked (no project open, unknown node, ...)."""


def call(method, **args):
    ok, text = _reetched.call(method, json.dumps(args))
    if not ok:
        raise ReEtchedError(text)
    return json.loads(text) if text else None


# Name of the plugin whose code is running right now (None for the console).
# Event connections, menu actions and timers remember it, so closing a plugin
# removes everything it registered.
current_owner = None


def invoke(owner, fn, *args):
    global current_owner
    previous, current_owner = current_owner, owner
    try:
        return fn(*args)
    finally:
        current_owner = previous


# Log lines written while no host call is possible (e.g. print() from a
# background thread) wait here until the app next runs Python.
_pending = []


def log(level, text):
    if not _reetched.has_host():
        _pending.append((level, text))
        return
    flush_pending()
    call("log", level=level, text=str(text))


def flush_pending():
    while _pending and _reetched.has_host():
        level, text = _pending.pop(0)
        call("log", level=level, text=text)


def report_exception(context):
    """Logs the exception being handled as an error, prefixed by context."""
    log("error", "%s\n%s" % (context, traceback.format_exc().rstrip()))
