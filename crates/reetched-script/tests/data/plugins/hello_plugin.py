"""Test plugin: a menu action, an event listener and a load counter."""
import builtins

from reetched import event, logging, ui

# Counts imports so the test can see reload_plugin() really re-imported us.
builtins._hello_plugin_loads = getattr(builtins, "_hello_plugin_loads", 0) + 1


def _say_hello():
    print("hello from plugin")


def _on_saved(e):
    logging.info("hello_plugin saw a save to %s" % e.path)


def start_plugin():
    ui.add_action("Test Plugin/Say Hello", _say_hello)
    event.DISPATCHER.connect(event.ProjectSaved, _on_saved)
    logging.info("hello_plugin started")


def close_plugin():
    # No manual cleanup: the engine drops this plugin's actions and listeners.
    logging.info("hello_plugin closed")
