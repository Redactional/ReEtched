from enum import Enum

from reetched import resource as _r


class Usage(Enum):
    TEXTURE = "texture"
    ENVIRONMENT = "environment"
    COLOR_LUT = "color_lut"
    ALPHA = "alpha"
    BASE_MATERIAL = "base_material"


class ResourceID(object):
    def __init__(self, context, name, version=None):
        self.context = context
        self.name = name
        self._rid = None  # ReEtched resource id, set for resources from this module

    def url(self):
        return "resource://%s/%s" % (self.context, self.name)

    def __repr__(self):
        return "ResourceID(%r, %r)" % (self.context, self.name)


class Resource(object):
    def __init__(self, core):
        self._core = core

    def identifier(self):
        rid = ResourceID("project", self._core.name)
        rid._rid = self._core.id
        return rid

    def gui_name(self):
        return self._core.name

    def __repr__(self):
        return "Resource(%r)" % self._core.name


def import_project_resource(file_path, resource_usage=None, name=None, group=None):
    # ponytail: usage, name and group are ignored; the app picks them from the file.
    return Resource(_r.import_project_resource(file_path))
