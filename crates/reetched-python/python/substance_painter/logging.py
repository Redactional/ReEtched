from reetched import logging as _log

ERROR = "error"
WARNING = "warning"
INFO = "info"
DBG_INFO = "info"

info = _log.info
warning = _log.warning
error = _log.error


def log(severity, channel, message):
    text = "[%s] %s" % (channel, message) if channel else message
    {"error": _log.error, "warning": _log.warning}.get(severity, _log.info)(text)
