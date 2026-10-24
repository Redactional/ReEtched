from reetched import ReEtchedError

# Everything the app refuses surfaces as ReEtchedError; the Painter names are
# aliases so `except ProjectError:` style handlers still catch it.
ProjectError = ReEtchedError
ResourceNotFoundError = ReEtchedError
ServiceNotFoundError = ReEtchedError
