#!/usr/bin/env python3
"""terminal_plugin — a tiny SDK for writing `terminal` plugins in Python.

Hides the JSON-RPC-over-stdio plumbing described in docs/plugin-api.md. Register
commands and event handlers, then call .run():

    from terminal_plugin import Plugin

    plug = Plugin("hello")

    @plug.command("hello.hi", title="Say hi", keys="prefix g")
    def hi():
        plug.notify("hi 👋 from a plugin")

    plug.run()

Host actions are methods on the Plugin instance (notify, show_panel, show_table,
close_panel, write_to_pane, split_pane, focus_pane, close_pane). Stdout writes are
serialized with a lock, so it's safe to call them from worker threads.
"""
import json
import sys
import threading

PROTOCOL_VERSION = 1  # the protocol version this SDK targets


class Plugin:
    def __init__(self, name):
        self.name = name
        self.host_protocol_version = None  # filled in from the initialize handshake
        self._commands = {}     # id -> callable()
        self._titles = {}       # id -> title
        self._keybindings = []  # [{"keys", "command"}]
        self._events = {}       # name -> callable(params)
        self._lock = threading.Lock()

    # --- registration -----------------------------------------------------

    def command(self, command_id, title="", keys=None):
        """Decorator: register a command, optionally bound to a chord (e.g.
        "prefix g"). The handler takes no arguments."""
        def deco(fn):
            self._commands[command_id] = fn
            self._titles[command_id] = title
            if keys:
                self._keybindings.append({"keys": keys, "command": command_id})
            return fn
        return deco

    def on_event(self, name):
        """Decorator: subscribe to an event (e.g. "command_end"). The handler
        receives the event's params dict."""
        def deco(fn):
            self._events[name] = fn
            return fn
        return deco

    # --- host actions -----------------------------------------------------

    def _send(self, method, params):
        msg = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
        with self._lock:
            sys.stdout.write(json.dumps(msg) + "\n")
            sys.stdout.flush()

    def notify(self, text):
        self._send("host/notify", {"text": text})

    def log(self, text, level="info"):
        """Write a line to the host's log (the terminal's stderr), prefixed with
        this plugin's name and `level`. For debugging, not user-facing."""
        self._send("host/log", {"text": text, "level": level})

    def show_panel(self, title, body, interactive=False):
        self._send("host/showPanel", {"title": title, "body": body, "input": interactive})

    def close_panel(self):
        self._send("host/closePanel", {})

    def show_table(self, title, headers, rows):
        """Render a table. `headers` is a list of strings; `rows` a list of lists."""
        self._send("host/showTable", {"title": title, "headers": headers, "rows": rows})

    def write_to_pane(self, text):
        self._send("host/writeToFocusedPane", {"text": text})

    def split_pane(self, direction="leftright"):
        self._send("host/splitPane", {"dir": direction})

    def focus_pane(self, direction):
        self._send("host/focusPane", {"dir": direction})

    def close_pane(self):
        self._send("host/closePane", {})

    # --- run loop ---------------------------------------------------------

    def _manifest(self):
        return {
            "name": self.name,
            "protocolVersion": PROTOCOL_VERSION,
            "commands": [{"id": cid, "title": self._titles[cid]} for cid in self._commands],
            "keybindings": self._keybindings,
            "events": list(self._events),
        }

    def _reply(self, req_id, result):
        with self._lock:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": result}) + "\n")
            sys.stdout.flush()

    def run(self):
        """Block on stdin, dispatching host messages until the host closes it.

        Lines that are pure responses (the host's `{"ok":...}` replies to our
        actions) carry no `method` and are simply ignored.
        """
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            method = msg.get("method")
            if method == "initialize":
                self.host_protocol_version = msg.get("params", {}).get("protocolVersion")
                self._reply(msg.get("id"), self._manifest())
            elif method == "command/invoke":
                fn = self._commands.get(msg.get("params", {}).get("command"))
                if fn:
                    fn()
            elif method and method.startswith("event/"):
                fn = self._events.get(method[len("event/"):])
                if fn:
                    fn(msg.get("params", {}))
