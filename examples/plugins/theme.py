#!/usr/bin/env python3
"""theme — switch the terminal's color theme at runtime, from a plugin.

Dogfoods `host/setTheme` (M13, plugin-driven theming): the whole UI — terminal
colors, pane divider, panel and recall overlays — recolors live, no restart.

  Ctrl-A y   cycle presets: default -> light -> solarized-dark -> (repeat)
  Ctrl-A p   toggle a red accent (a *partial* patch merged onto the current
             theme, proving one color can change while the rest stays put)

Theming policy (which presets, cycle order, the accent color) lives here in the
plugin; core just owns the Theme and applies presets/patches. Built on the SDK.

Enable via ~/.config/jetem/plugins.toml:
    [[plugin]]
    command = "python3 /abs/path/to/examples/plugins/theme.py"
"""
import os
import sys

# Make the SDK importable whether run from the repo or elsewhere.
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "sdk"))
from jetem_plugin import Plugin  # noqa: E402

# Preset names the host knows (built-ins). A user can add ~/.config/jetem/themes/
# <name>.toml files and list them here too.
PRESETS = ["default", "light", "solarized-dark"]

plug = Plugin("theme")
_idx = 0          # index into PRESETS of the currently-applied preset
_accent = False   # whether the red-accent patch is currently layered on


def _apply():
    """(Re)apply the current preset, then the accent patch if it's toggled on.
    Reapplying the preset first is what lets the accent toggle *off* cleanly."""
    plug.set_theme(preset=PRESETS[_idx])
    if _accent:
        plug.set_theme(patch={"panel": {"title": "#ff3b30"},
                              "ui": {"focus_border": "#ff3b30"}})


@plug.command("theme.cycle", title="Cycle color theme", keys="prefix y")
def cycle():
    global _idx
    _idx = (_idx + 1) % len(PRESETS)
    _apply()
    plug.notify(f"theme: {PRESETS[_idx]}")


@plug.command("theme.accent", title="Toggle red accent", keys="prefix p")
def accent():
    global _accent
    _accent = not _accent
    _apply()
    plug.notify(f"accent: {'on' if _accent else 'off'}")


if __name__ == "__main__":
    plug.run()
