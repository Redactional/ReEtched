"""Test plugin shipped as a package, importing its own submodule."""
from . import helpers


def start_plugin():
    helpers.announce()
