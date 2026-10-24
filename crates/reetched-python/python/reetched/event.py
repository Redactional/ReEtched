"""App events. Connect a callback to an event class; it receives an instance.

    from reetched import event
    def on_saved(e):
        print("saved to", e.path)
    event.DISPATCHER.connect(event.ProjectSaved, on_saved)

Every key of the app's event data is also an attribute of the event object
(and the whole dict is in `e.data`). Connecting to `Event` receives everything.
"""
from . import _core


class Event(object):
    #: The app's name for this event.
    name = None

    def __init__(self, data=None):
        self.data = dict(data or {})
        for key, value in self.data.items():
            if not hasattr(type(self), key):
                setattr(self, key, value)

    def __repr__(self):
        return "%s(%r)" % (type(self).__name__, self.data)


class ProjectOpened(Event):
    name = "project_opened"


class ProjectCreated(Event):
    name = "project_created"


class ProjectSaved(Event):
    name = "project_saved"


class ProjectClosed(Event):
    name = "project_closed"


class ProjectAboutToSave(Event):
    name = "project_about_to_save"


class ExportTexturesStarted(Event):
    name = "export_started"


class ExportTexturesEnded(Event):
    name = "export_finished"


class TextureSetChanged(Event):
    name = "texture_set_changed"


class LayerStackChanged(Event):
    name = "layers_changed"


class BakingEnded(Event):
    name = "baking_finished"


class Shutdown(Event):
    name = "shutdown"


_BY_NAME = {cls.name: cls for cls in Event.__subclasses__()}


class Dispatcher(object):
    def __init__(self):
        self._connections = []  # [event class, callback, owning plugin]

    def connect(self, event_class, callback):
        self._connections.append([event_class, callback, _core.current_owner])

    def disconnect(self, event_class, callback):
        self._connections = [
            c for c in self._connections if not (c[0] is event_class and c[1] == callback)
        ]

    def _forget_owner(self, owner):
        self._connections = [c for c in self._connections if c[2] != owner]

    def _emit(self, name, data):
        cls = _BY_NAME.get(name)
        if cls is None:
            # Unknown to this API version: still deliver to Event listeners.
            ev = Event(data)
            ev.name = name
        else:
            ev = cls(data)
        for event_class, callback, owner in list(self._connections):
            if isinstance(ev, event_class):
                try:
                    _core.invoke(owner, callback, ev)
                except Exception:
                    _core.report_exception("Error in %s callback for %s:" % (owner or "script", name))


DISPATCHER = Dispatcher()
