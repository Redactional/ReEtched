"""Messages to the app's log panel. (print() also lands there, as info.)"""
from . import _core


def info(message):
    _core.log("info", message)


def warning(message):
    _core.log("warning", message)


def error(message):
    _core.log("error", message)
