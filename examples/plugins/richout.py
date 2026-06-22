#!/usr/bin/env python3
"""richout — render structured command output as a table. Press Ctrl-A t after a
command whose output is JSON or aligned columns.

A "block renderer": it watches `command_end`, keeps the last block's output,
classifies it (JSON object/array, or whitespace-aligned columns), and asks the
host to draw a table via `host/showTable` — or a text panel for nested JSON.
Detection/parsing is *policy* and lives here; the table-drawing primitive lives
in core. Built on the terminal_plugin SDK.

Enable via ~/.config/terminal/plugins.toml:
    [[plugin]]
    command = "python3 /abs/path/to/examples/plugins/richout.py"
"""
import json
import os
import re
import sys

# Make the SDK importable whether run from the repo or elsewhere.
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "sdk"))
from terminal_plugin import Plugin  # noqa: E402

MAX_ROWS = 200  # cap so a huge output can't build an enormous table

plug = Plugin("richout")
_last_output = ""


@plug.on_event("command_end")
def _remember(params):
    global _last_output
    _last_output = params.get("output", "") or ""


@plug.command("rich.render", title="Render output as a table", keys="prefix t")
def render():
    text = _last_output.strip()
    if not text:
        plug.notify("nothing to render yet")
    elif not (_try_json(text) or _try_table(text)):
        plug.notify("last output isn't structured (need JSON or aligned columns)")


def _try_json(text):
    """A JSON array of objects -> a row each; a JSON object -> key/value; nested
    or scalar JSON -> a pretty-printed text panel."""
    if text[0] not in "{[":
        return False
    try:
        data = json.loads(text)
    except (ValueError, TypeError):
        return False
    if isinstance(data, list) and data and all(isinstance(d, dict) for d in data):
        cols = []
        for d in data:
            for k in d:
                if k not in cols:
                    cols.append(k)
        rows = [[_scalar(d.get(c, "")) for c in cols] for d in data[:MAX_ROWS]]
        plug.show_table("📦 JSON", cols, rows)
    elif isinstance(data, dict):
        rows = [[k, _scalar(v)] for k, v in list(data.items())[:MAX_ROWS]]
        plug.show_table("📦 JSON", ["key", "value"], rows)
    else:
        plug.show_panel("📦 JSON", json.dumps(data, indent=2))
    return True


def _try_table(text):
    """Whitespace-aligned columns (df, docker ps, kubectl get, …): columns are
    separated by runs of 2+ spaces, and the first line is the header."""
    lines = [l for l in text.splitlines() if l.strip()]
    if len(lines) < 2:
        return False
    split = [re.split(r"\s{2,}", l.strip()) for l in lines]
    ncols = len(split[0])
    if ncols < 2:
        return False
    # Require most rows to share the header's column count (else it's prose).
    if sum(1 for r in split if len(r) == ncols) < max(2, len(split) // 2):
        return False
    rows = [(r + [""] * ncols)[:ncols] for r in split[1:MAX_ROWS + 1]]
    plug.show_table("▦ table", split[0], rows)
    return True


def _scalar(v):
    """Render one JSON value as a single table cell."""
    if isinstance(v, str):
        return v
    if v is None:
        return ""
    if isinstance(v, (dict, list)):
        return json.dumps(v)
    return str(v)


if __name__ == "__main__":
    plug.run()
