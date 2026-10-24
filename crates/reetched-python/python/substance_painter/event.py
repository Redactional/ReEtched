from reetched import event as _ev
from reetched.event import Event


class ProjectOpened(_ev.ProjectOpened):
    pass


class ProjectCreated(_ev.ProjectCreated):
    pass


class ProjectSaved(_ev.ProjectSaved):
    pass


class ProjectAboutToSave(_ev.ProjectAboutToSave):
    @property
    def file_path(self):
        return self.data.get("path")


class ProjectAboutToClose(_ev.ProjectClosed):
    # ponytail: ReEtched reports the close after it happens, not before.
    pass


class ProjectEditionEntered(Event):
    # Painter fires this once a project is ready to edit: after open or create.
    _sources = (_ev.ProjectOpened, _ev.ProjectCreated)


class ExportTexturesAboutToStart(_ev.ExportTexturesStarted):
    @property
    def textures(self):
        return self.data.get("textures", {})


class ExportTexturesEnded(_ev.ExportTexturesEnded):
    @property
    def status(self):
        from .export import ExportStatus
        return ExportStatus.Success if self.data.get("ok", True) else ExportStatus.Error

    @property
    def message(self):
        return self.data.get("message", "")

    @property
    def textures(self):
        return {(ts, ""): files for ts, files in self.data.get("files", {}).items()}


class LayerStacksModelDataChanged(_ev.LayerStackChanged):
    pass


class BakingProcessEnded(_ev.BakingEnded):
    pass


class _Dispatcher(object):
    """Connects Painter-named event classes to the ReEtched dispatcher.

    The callback receives an instance of the class it connected to, so
    Painter-only attributes (e.g. ExportTexturesEnded.textures) work.
    """

    def __init__(self):
        self._wrappers = {}

    def connect(self, event_class, callback):
        # The ReEtched dispatcher emits its own classes; listen on the nearest one.
        sources = event_class.__dict__.get("_sources") or (
            next(c for c in event_class.__mro__ if c.__module__ == _ev.__name__),
        )

        def wrapper(e):
            callback(e if type(e) is event_class else event_class(e.data))

        self._wrappers[(event_class, callback)] = (sources, wrapper)
        for source in sources:
            _ev.DISPATCHER.connect(source, wrapper)

    def disconnect(self, event_class, callback):
        sources, wrapper = self._wrappers.pop((event_class, callback), ((), None))
        for source in sources:
            _ev.DISPATCHER.disconnect(source, wrapper)


DISPATCHER = _Dispatcher()
