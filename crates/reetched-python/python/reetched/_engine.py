"""What the app drives: console, plugins, events, menu actions and timers.

The native side calls dispatch(op, args_json) and gets back a JSON envelope:
{"ok": result} or {"error": "text"}.
"""
import ast
import builtins
import importlib
import json
import linecache
import os
import sys
import traceback

from . import _core, event, ui


class _OpError(Exception):
    """Turns into an {"error": ...} answer without a Python traceback."""


# ---------------------------------------------------------------- output

# While run()/run_file()/trigger_action() execute, output is collected for the
# RunResult instead of being logged, so the console does not show it twice.
_capture = None


class _HostStream(object):
    encoding = "utf-8"
    errors = "replace"

    def __init__(self, level):
        self.level = level
        self._buf = ""

    def write(self, s):
        s = str(s)
        if _capture is not None:
            _capture.append(s)
            return len(s)
        self._buf += s
        if "\n" in self._buf:
            lines = self._buf.split("\n")
            self._buf = lines.pop()
            for line in lines:
                _core.log(self.level, line)
        return len(s)

    def flush(self):
        if self._buf and _capture is None:
            line, self._buf = self._buf, ""
            _core.log(self.level, line)

    def isatty(self):
        return False

    def writable(self):
        return True


sys.stdout = _HostStream("info")
sys.stderr = _HostStream("error")


def _user_traceback(tb):
    # Drop the engine's own frames so tracebacks start at the user's code.
    while tb is not None and tb.tb_frame.f_code.co_filename in (__file__, _core.__file__):
        tb = tb.tb_next
    return tb


def _captured(fn, *args):
    global _capture
    previous, _capture = _capture, []
    error = None
    try:
        fn(*args)
    except SystemExit:
        pass
    except BaseException as e:
        error = "".join(traceback.format_exception(type(e), e, _user_traceback(e.__traceback__)))
    finally:
        output = "".join(_capture)
        _capture = previous
    return {"output": output, "error": error}


# ---------------------------------------------------------------- console

_console = {"__name__": "__console__", "__builtins__": builtins}


def _exec_source(code, filename, namespace, echo):
    # Register the text so tracebacks can quote lines of console input too.
    linecache.cache[filename] = (len(code), None, code.splitlines(True), filename)
    tree = ast.parse(code, filename, "exec")
    last = None
    if echo and tree.body and isinstance(tree.body[-1], ast.Expr):
        last = ast.Expression(tree.body.pop().value)
    exec(compile(tree, filename, "exec"), namespace)
    if last is not None:
        value = eval(compile(last, filename, "eval"), namespace)
        if value is not None:
            namespace["_"] = value
            print(repr(value))


def op_run(code, filename="<console>"):
    return _captured(_exec_source, code, filename, _console, True)


def op_run_file(path):
    def run():
        with open(path, "rb") as f:
            code = f.read().decode("utf-8")
        namespace = {"__name__": "__main__", "__file__": path, "__builtins__": builtins}
        _exec_source(code, path, namespace, False)

    return _captured(run)


# ---------------------------------------------------------------- plugins

_plugin_dirs = []
_plugins = {}  # name -> state dict, in discovery order


def op_init(plugin_dirs, executable=None, site_packages=()):
    # sys.executable would be the app; point it at python so subprocess/pip work.
    if executable:
        sys.executable = executable
    # A venv's packages (REETCHED_PYTHON pointing into one) are not on the base path.
    for p in site_packages:
        if p not in sys.path:
            sys.path.append(p)
    # Re-initialising (a second Engine::start) closes what the old setup started.
    for p in list(_plugins.values()):
        _stop(p)
    _plugins.clear()
    for d in _plugin_dirs:
        if d in sys.path:
            sys.path.remove(d)
    _plugin_dirs[:] = [os.path.abspath(d) for d in plugin_dirs]
    # Plugin dirs go on sys.path so plugins import like normal modules/packages.
    sys.path[0:0] = _plugin_dirs
    _scan()
    return None


def _scan():
    for d in _plugin_dirs:
        try:
            entries = sorted(os.listdir(d))
        except OSError:
            continue
        for entry in entries:
            if entry.startswith(("_", ".")):
                continue
            path = os.path.join(d, entry)
            if entry.endswith(".py") and os.path.isfile(path):
                name = entry[:-3]
            elif os.path.isfile(os.path.join(path, "__init__.py")):
                name = entry
            else:
                continue
            # The first directory wins, matching sys.path order.
            _plugins.setdefault(name, {
                "name": name, "path": path, "module": None,
                "enabled": False, "started": False, "error": None,
            })


def _plugin(name):
    _scan()
    p = _plugins.get(name)
    if p is None:
        raise _OpError("no plugin named %r" % name)
    return p


def _start(p):
    p["enabled"] = True
    if p["started"]:
        return
    p["error"] = None
    name = p["name"]
    try:
        if p["module"] is None:
            p["module"] = _core.invoke(name, importlib.import_module, name)
        start = getattr(p["module"], "start_plugin", None)
        if start is not None:
            _core.invoke(name, start)
        p["started"] = True
    except BaseException as e:
        p["error"] = "".join(traceback.format_exception(type(e), e, _user_traceback(e.__traceback__)))
        _core.log("error", "Plugin %s failed to start:\n%s" % (name, p["error"].rstrip()))
        _forget_owner(name)


def _stop(p):
    p["enabled"] = False
    name = p["name"]
    if p["started"]:
        p["started"] = False
        close = getattr(p["module"], "close_plugin", None)
        try:
            if close is not None:
                _core.invoke(name, close)
        except BaseException as e:
            p["error"] = "".join(traceback.format_exception(type(e), e, _user_traceback(e.__traceback__)))
            _core.log("error", "Plugin %s failed to close:\n%s" % (name, p["error"].rstrip()))
    _forget_owner(name)


def _forget_owner(name):
    event.DISPATCHER._forget_owner(name)
    ui._forget_owner(name)


def op_plugins():
    _scan()
    return [
        {k: p[k] for k in ("name", "path", "enabled", "started", "error")}
        for p in _plugins.values()
    ]


def op_set_plugin_enabled(name, on):
    p = _plugin(name)
    if on:
        _start(p)
        if p["error"]:
            raise _OpError(p["error"])
    else:
        p["error"] = None  # a stale start error is not a reason to refuse disabling
        _stop(p)
        if p["error"]:
            raise _OpError(p["error"])
    return None


def op_reload_plugin(name):
    p = _plugin(name)
    was_enabled = p["enabled"]
    _stop(p)
    for mod in list(sys.modules):
        if mod == name or mod.startswith(name + "."):
            del sys.modules[mod]
    p["module"] = None
    p["error"] = None
    importlib.invalidate_caches()
    if was_enabled:
        _start(p)
        if p["error"]:
            raise _OpError(p["error"])
    return None


# ---------------------------------------------------------------- events, actions, timers

def op_event(name, data=None):
    event.DISPATCHER._emit(name, data or {})
    if name == "shutdown":
        # Like Painter: plugins get close_plugin() when the app exits.
        for p in list(_plugins.values()):
            _stop(p)
    return None


def op_actions():
    return ui._list_actions()


def op_trigger_action(id):
    action = ui._actions.get(id)
    if action is None:
        return {"output": "", "error": "no menu action with id %d" % id}
    return _captured(_core.invoke, action["owner"], action["callback"])


def op_tick():
    ui._run_due_timers()
    return None


_OPS = {
    "init": op_init,
    "run": op_run,
    "run_file": op_run_file,
    "plugins": op_plugins,
    "set_plugin_enabled": op_set_plugin_enabled,
    "reload_plugin": op_reload_plugin,
    "event": op_event,
    "actions": op_actions,
    "trigger_action": op_trigger_action,
    "tick": op_tick,
}


def dispatch(op, args_json):
    try:
        _core.flush_pending()
        fn = _OPS.get(op)
        if fn is None:
            raise _OpError("unknown engine op %r" % op)
        result = {"ok": fn(**json.loads(args_json))}
    except _OpError as e:
        result = {"error": str(e)}
    except BaseException:
        result = {"error": traceback.format_exc()}
    finally:
        sys.stdout.flush()
        sys.stderr.flush()
    return json.dumps(result)
