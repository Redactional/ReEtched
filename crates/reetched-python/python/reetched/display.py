"""3D view display settings. Tone mappings: "aces", "agx", "neutral", "linear", "ocio"."""
import os

from ._core import call


def get_exposure():
    return call("display.get")["exposure"]


def set_exposure(stops):
    call("display.set", exposure=float(stops))


def get_environment():
    return call("display.get")["environment"]


def set_environment(path):
    call("display.set", environment=os.fspath(path))


def get_tone_mapping():
    return call("display.get")["tone_mapping"]


def set_tone_mapping(name):
    call("display.set", tone_mapping=name)
