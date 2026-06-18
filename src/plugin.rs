//! The plugin host: spawns plugin subprocesses and speaks newline-delimited
//! JSON-RPC 2.0 over their stdin/stdout (MCP-style). Plugins register commands,
//! keybindings, and event subscriptions; the host invokes their commands and
//! they call back with `host/*` actions. This is the out-of-process tier — the
//! "real design work" registry/protocol layer is here and runtime-independent.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

use serde::Deserialize;
use serde_json::{json, Value};
use winit::event_loop::EventLoopProxy;

use crate::window::UserEvent;

pub type PluginId = usize;

// --- protocol (what a plugin declares in its initialize response) -----------

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct Manifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub commands: Vec<CommandDef>,
    #[serde(default)]
    pub keybindings: Vec<KeyBinding>,
    #[serde(default)]
    pub events: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct CommandDef {
    pub id: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct KeyBinding {
    /// e.g. "prefix |" — a Ctrl-A chord.
    pub keys: String,
    pub command: String,
}

/// A message from a plugin, delivered to the event loop via `UserEvent::Plugin`.
#[derive(Debug)]
pub enum PluginInbound {
    /// The initialize response: what the plugin registers.
    Manifest(Manifest),
    /// A `host/*` action request the plugin wants the host to perform.
    HostAction {
        id: Value,
        method: String,
        params: Value,
    },
    /// The plugin process ended.
    Closed,
}

/// Parse one JSON-RPC line from a plugin into a `PluginInbound`.
pub fn parse_line(line: &str) -> Option<PluginInbound> {
    let msg: Value = serde_json::from_str(line).ok()?;
    if let Some(method) = msg.get("method").and_then(Value::as_str) {
        if method.starts_with("host/") {
            return Some(PluginInbound::HostAction {
                id: msg.get("id").cloned().unwrap_or(Value::Null),
                method: method.to_string(),
                params: msg.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        return None; // unknown method from plugin — ignore
    }
    // A result is the response to our `initialize` request: the manifest.
    let result = msg.get("result")?;
    let manifest = Manifest::deserialize(result).ok()?;
    Some(PluginInbound::Manifest(manifest))
}

// --- registry (commands / keybindings / event subscriptions) ----------------

#[derive(Default)]
pub struct Registry {
    /// command id -> owning plugin.
    pub commands: HashMap<String, PluginId>,
    /// keybinding chord -> command id.
    pub keymap: HashMap<String, String>,
    /// event name -> subscribed plugins.
    pub events: HashMap<String, Vec<PluginId>>,
}

impl Registry {
    pub fn apply_manifest(&mut self, plugin: PluginId, m: &Manifest) {
        for c in &m.commands {
            self.commands.insert(c.id.clone(), plugin);
        }
        for k in &m.keybindings {
            self.keymap.insert(k.keys.clone(), k.command.clone());
        }
        for e in &m.events {
            self.events.entry(e.clone()).or_default().push(plugin);
        }
    }

    /// Resolve a key chord to (command id, owning plugin).
    pub fn command_for_chord(&self, chord: &str) -> Option<(String, PluginId)> {
        let cmd = self.keymap.get(chord)?;
        let pid = self.commands.get(cmd)?;
        Some((cmd.clone(), *pid))
    }
}

// --- a running plugin process ----------------------------------------------

pub struct Plugin {
    pub name: String,
    /// Lines to write to the plugin's stdin (served by a writer thread).
    tx: mpsc::Sender<String>,
    child: Child,
}

impl Plugin {
    /// Launch `command` (a "prog arg arg" string), wiring stdio to JSON-RPC.
    /// The reader thread forwards parsed messages via `proxy`.
    pub fn spawn(id: PluginId, command: &str, proxy: EventLoopProxy<UserEvent>) -> Option<Self> {
        let mut parts = command.split_whitespace();
        let prog = parts.next()?;
        let args: Vec<&str> = parts.collect();

        let mut child = Command::new(prog)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;

        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;

        // Writer thread: drains the channel into the plugin's stdin.
        let (tx, rx) = mpsc::channel::<String>();
        thread::spawn(move || {
            for line in rx {
                if stdin.write_all(line.as_bytes()).is_err() || stdin.write_all(b"\n").is_err() {
                    break;
                }
                let _ = stdin.flush();
            }
        });

        // Reader thread: parse each stdout line, forward to the event loop.
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Some(msg) = parse_line(&line) {
                    let _ = proxy.send_event(UserEvent::Plugin { id, msg });
                }
            }
            let _ = proxy.send_event(UserEvent::Plugin {
                id,
                msg: PluginInbound::Closed,
            });
        });

        Some(Self {
            name: String::new(),
            tx,
            child,
        })
    }

    fn send(&self, msg: Value) {
        let _ = self.tx.send(msg.to_string());
    }

    /// Handshake: ask the plugin to register itself.
    pub fn initialize(&self) {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"host":"terminal"}}));
    }

    /// Tell the plugin one of its commands fired.
    pub fn invoke(&self, command: &str) {
        self.send(json!({"jsonrpc":"2.0","method":"command/invoke","params":{"command":command}}));
    }

    /// Reply to a `host/*` action request.
    pub fn reply(&self, id: Value, ok: bool) {
        self.send(json!({"jsonrpc":"2.0","id":id,"result":{"ok":ok}}));
    }

    /// Send an event notification (used from M10b).
    pub fn event(&self, name: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","method":format!("event/{name}"),"params":params}));
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_manifest_line() {
        let line = r#"{"jsonrpc":"2.0","id":1,"result":{"name":"m","commands":[{"id":"x"}],"keybindings":[{"keys":"prefix g","command":"x"}],"events":["command_end"]}}"#;
        match parse_line(line) {
            Some(PluginInbound::Manifest(m)) => {
                assert_eq!(m.name, "m");
                assert_eq!(m.commands[0].id, "x");
                assert_eq!(m.keybindings[0].keys, "prefix g");
                assert_eq!(m.events, vec!["command_end".to_string()]);
            }
            other => panic!("expected manifest, got {other:?}"),
        }
    }

    #[test]
    fn parse_host_action_line() {
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"host/splitPane","params":{"dir":"leftright"}}"#;
        match parse_line(line) {
            Some(PluginInbound::HostAction { method, params, .. }) => {
                assert_eq!(method, "host/splitPane");
                assert_eq!(params["dir"], "leftright");
            }
            other => panic!("expected host action, got {other:?}"),
        }
    }

    #[test]
    fn parse_ignores_plugin_notifications() {
        assert!(parse_line(r#"{"jsonrpc":"2.0","method":"log","params":{}}"#).is_none());
        assert!(parse_line("not json").is_none());
    }

    #[test]
    fn registry_maps_chord_to_command_and_plugin() {
        let m = Manifest {
            name: "mux".into(),
            commands: vec![CommandDef {
                id: "split".into(),
                title: String::new(),
            }],
            keybindings: vec![KeyBinding {
                keys: "prefix |".into(),
                command: "split".into(),
            }],
            events: vec![],
        };
        let mut reg = Registry::default();
        reg.apply_manifest(3, &m);
        assert_eq!(reg.command_for_chord("prefix |"), Some(("split".to_string(), 3)));
        assert_eq!(reg.command_for_chord("prefix x"), None);
    }
}
