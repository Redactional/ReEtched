"""Project resources (bitmaps, environments, ...) usable by layers."""
import os

from ._core import call


class Resource(object):
    def __init__(self, data):
        self.id = data["id"]
        self.name = data["name"]
        self.kind = data.get("kind")

    def __eq__(self, other):
        return isinstance(other, Resource) and other.id == self.id

    def __hash__(self):
        return hash(self.id)

    def __repr__(self):
        return "Resource(%d, %r)" % (self.id, self.name)


def import_project_resource(path):
    """Imports a file into the project. Returns its Resource."""
    return Resource(call("resource.import", path=os.fspath(path)))


def list_project_resources():
    return [Resource(r) for r in call("resource.list")["resources"]]
