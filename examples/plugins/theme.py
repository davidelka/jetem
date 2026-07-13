#!/usr/bin/env python3
"""theme — switch the terminal's color theme at runtime, from a plugin.

Dogfoods `host/setTheme` (M13, plugin-driven theming): the whole UI — terminal
colors, pane divider, panel and recall overlays — recolors live, no restart.

  Ctrl-A y   cycle presets: default -> light -> solarized-dark -> (repeat)
  Ctrl-A p   flip the terminal background (a *partial* patch merged onto the
             current theme — one color changes while the rest stays put)

Theming policy (which presets, cycle order, the flip color) lives here in the
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
_flipped = False  # whether the background-flip patch is currently layered on

# The two poles we flip between. Each pairs a background with a readable
# foreground, so flipping to the *opposite* luminance keeps text legible
# (flipping bg alone would leave light text on a light bg, or vice-versa).
DARK = {"bg": "#101218", "fg": "#cccccc"}
LIGHT = {"bg": "#f7f7f2", "fg": "#2b2b2b"}


def _is_dark(hex_color):
    """Perceived luminance < 50% -> dark. Defaults to dark on a bad value."""
    try:
        r, g, b = (int(hex_color[i : i + 2], 16) for i in (1, 3, 5))
        return (0.299 * r + 0.587 * g + 0.114 * b) < 128
    except (ValueError, IndexError, TypeError):
        return True


def _apply():
    """(Re)apply the current preset, then the background flip if it's toggled on.
    Reapplying the preset first is what lets the flip toggle *off* cleanly."""
    plug.set_theme(preset=PRESETS[_idx])
    if _flipped:
        # Ask the host for the *actual* current background (host/getTheme) and
        # flip to the opposite luminance — exact, not guessed from the name.
        bg = plug.get_theme().get("terminal", {}).get("bg", "")
        plug.set_theme(patch={"terminal": LIGHT if _is_dark(bg) else DARK})


@plug.command("theme.cycle", title="Cycle color theme", keys="prefix y")
def cycle():
    global _idx
    _idx = (_idx + 1) % len(PRESETS)
    _apply()
    plug.notify(f"theme: {PRESETS[_idx]}")


@plug.command("theme.flipbg", title="Flip terminal background", keys="prefix p")
def flip_bg():
    global _flipped
    _flipped = not _flipped
    _apply()
    plug.notify(f"background: {'flipped' if _flipped else 'normal'}")


if __name__ == "__main__":
    plug.run()
