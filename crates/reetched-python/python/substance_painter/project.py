from enum import Enum

from reetched import project as _p


class ProjectSaveMode(Enum):
    Full = 0
    Incremental = 1


is_open = _p.is_open
file_path = _p.file_path
name = _p.name
needs_saving = _p.needs_saving
close = _p.close


def open(project_file_path, mode=None):
    _p.open(project_file_path)


def save(mode=ProjectSaveMode.Full):
    _p.save()


def save_as(project_file_path, mode=ProjectSaveMode.Full):
    _p.save(project_file_path)


def create(mesh_file_path, mesh_map_file_paths=None, template_file_path=None, settings=None):
    _p.create(mesh_file_path, template_file_path)


def is_busy():
    # Scripts run between frames, never while the app is mid-operation.
    return False


def execute_when_not_busy(callback):
    callback()
