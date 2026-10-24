"""The open project: open, create, save, close, undo/redo."""
import os

from ._core import call


def _info():
    return call("project.info")


def is_open():
    return _info()["open"]


def file_path():
    """Path of the project file, or None if it was never saved."""
    return _info()["path"]


def name():
    return _info()["name"]


def needs_saving():
    return _info()["dirty"]


def open(path):
    call("project.open", path=os.fspath(path))


def save(path=None):
    """Saves (to `path` if given, i.e. "save as"). Returns the file path written."""
    return call("project.save", path=None if path is None else os.fspath(path))["path"]


def create(mesh_path, template=None):
    call("project.create", mesh=os.fspath(mesh_path), template=template)


def close():
    call("project.close")


def undo():
    """Undoes one step. Returns False when there was nothing to undo."""
    return call("history.undo")["done"]


def redo():
    return call("history.redo")["done"]
