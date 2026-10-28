# Troubleshooting

This page covers the most common problems: ReEtched not starting on your graphics card, Python scripting not being found, and recovering work after a crash. It also shows how to get a detailed log and how to report a bug.

## ReEtched closes as soon as it starts

ReEtched needs a graphics card that supports **Vulkan**, **Metal** or **DirectX 12**, and that can use enough textures at once. If your GPU or its driver falls short, ReEtched closes right after it starts.

If you start ReEtched from a terminal on macOS or Linux, it prints the reason. The two messages you're most likely to see are:

- `no compatible GPU found`: ReEtched couldn't find a graphics card it can use.
- `this GPU can bind only N textures per shader stage; ReEtched needs 23`: your graphics card is too old or too basic for ReEtched's layer compositing.

Things to try:

1. Update your graphics driver from your GPU maker's website.
2. On a laptop with two graphics chips, make sure ReEtched runs on the dedicated one. ReEtched asks for the high-performance GPU, but your system's graphics settings can override that.
3. Don't run ReEtched over remote desktop or in a virtual machine without GPU support. These often only offer a basic software driver.

If none of that helps, your GPU can't run ReEtched.

### The wireframe options are grayed out

If **Wireframe (3D view)** and **UV wireframe (2D view)** in **DISPLAY SETTINGS** are grayed out, your GPU doesn't support drawing lines that way. Everything else still works.

## Python and scripting

Python is optional. Without it, ReEtched works normally, but the **Python** menu's **Run script…** is grayed out and plugins don't load. The bottom of the **Python** menu, and the top of the Python console, say why scripting is off.

### Python not found

The message `Scripting is off: Python 3.8+ not found` means ReEtched couldn't find a suitable Python. ReEtched looks for Python in this order:

1. the Python you chose in Settings, if any
2. the `REETCHED_PYTHON` environment variable, if set (a Python program or its install folder)
3. the `py`, `python3` and `python` commands
4. on Windows, the standard install locations

To fix it:

1. Install Python 3.8 or newer. The installers from [python.org](https://www.python.org/downloads/) work.
2. Restart ReEtched.

### Pointing ReEtched at a specific Python

If Python is installed but ReEtched doesn't find it, or finds the wrong one:

1. Choose **File → Settings…**.
2. Next to **Python**, click **Choose…**.
3. Select the Python program: `python.exe` in the install folder on Windows, or the `python3` program on macOS and Linux.
4. Close Settings and restart ReEtched. Python is picked up at startup.

To go back to finding Python automatically, click **Automatic** next to **Python**, then restart.

### Other Python messages

| Message | What it means |
|---|---|
| `Python 3.x is too old (3.8+ needed)` | The Python ReEtched found is older than 3.8. Install a newer one, or choose a newer one in Settings |
| `Python 3.x has no shared library to embed` | That Python install can't be used from inside another app. Some minimal or custom-built Pythons are like this. Use an install from python.org or your system's package manager instead |
| `Plugin … failed to start` | A plugin you turned on raised an error at startup. The **LOG** shows the error. Turn the plugin off in **Python → Plugins…** if it keeps failing |

See [Python scripting](scripting.md) for how scripting works.

## The debug log

The **LOG** panel shows the messages most people need. When you're tracking down a problem, or a developer asks you for a log, turn on the more detailed debug log:

1. Choose **File → Settings…**.
2. Under **DEVELOPER**, check **Show the console window (debug log)**.

On Windows, a separate window titled **ReEtched debug log** opens. Its close button is turned off on purpose, because closing it would also quit ReEtched. To close it, clear the same checkbox in Settings.

On macOS and Linux, the setting is called **Debug log (in the terminal ReEtched was started from)**. Start ReEtched from a terminal, turn the setting on, and the log appears in that terminal.

The setting is remembered, so turn it off again when you're done.

## Recovering after a crash

ReEtched autosaves your project every few minutes (every 5 by default). If it closes unexpectedly, the next start shows **RECOVER WORK**:

- **Recover** opens the autosave. If the project had been saved before, **Save** writes back to the original file. Save straight away.
- **Discard** deletes the autosave.

If you don't see this window after a crash, autosave may be off, or ReEtched closed before it had autosaved anything. Check **Autosave every** in **File → Settings…**; 0 means off. See [Projects and meshes](projects-and-meshes.md#autosave-and-crash-recovery).

## Where your settings live

ReEtched keeps your settings, autosaves, project templates, plugins and personal asset library in one folder. To open it, choose **File → Settings…** and click **Open the settings folder**.

By default the folder is:

| System | Folder |
|---|---|
| Windows | `%APPDATA%\ReEtched` |
| macOS | `~/Library/Application Support/ReEtched` |
| Linux | `~/.config/ReEtched` |

Inside it you'll find:

| Item | What it holds |
|---|---|
| `settings.json` | Your settings, keyboard shortcuts and recent projects |
| `sessions` | Autosaves of running sessions |
| `templates` | Project templates you saved |
| `plugins` | Python plugins |
| `library` | Your own asset library (**My library**) |
| `cache` | Caches ReEtched can rebuild |

If ReEtched behaves strangely after you changed a lot of settings, quit it and move `settings.json` somewhere else. ReEtched starts with default settings and creates a new file. Your projects aren't affected.

## The update check fails

**Help → Check for updates** needs an internet connection and the `curl` program, which comes with Windows 10 and later, macOS and most Linux systems. If the check fails, ReEtched says *Could not check for updates*, followed by the reason. You can always get the latest version from the [releases page](https://github.com/Redactional/ReEtched/releases/latest).

## Reporting a bug

If something doesn't work the way this manual says, report it:

1. Go to [github.com/Redactional/ReEtched/issues/new/choose](https://github.com/Redactional/ReEtched/issues/new/choose). **Help → Report a problem** opens the issue tracker too.
2. Pick the kind of report and fill it in.

Include:

- your ReEtched version and graphics card. Both are on the right side of the status bar; the version is also in **Help → About ReEtched**.
- your operating system
- the steps that cause the problem, starting from opening ReEtched
- what you expected and what happened instead
- the relevant part of the debug log (see [The debug log](#the-debug-log))
- if you can share it, a small project or mesh that shows the problem
