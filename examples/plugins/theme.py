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

# Presets whose background is light (everything else is treated as dark).
LIGHT_PRESETS = {"light"}
# The two poles we flip between. Each pairs a background with a readable
# foreground, so flipping to the *opposite* luminance keeps text legible
# (flipping bg alone would leave light text on a light bg, or vice-versa).
DARK = {"bg": "#101218", "fg": "#cccccc"}
LIGHT = {"bg": "#f7f7f2", "fg": "#2b2b2b"}


def _apply():
    """(Re)apply the current preset, then the background flip if it's toggled on.
    Reapplying the preset first is what lets the flip toggle *off* cleanly (it
    restores the preset's own background)."""
    plug.set_theme(preset=PRESETS[_idx])
    if _flipped:
        # Flip to the opposite of the current preset's background luminance.
        opposite = DARK if PRESETS[_idx] in LIGHT_PRESETS else LIGHT
        plug.set_theme(patch={"terminal": opposite})


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
