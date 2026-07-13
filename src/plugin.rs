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

/// The JSON-RPC plugin protocol version the host speaks. Sent in `initialize`;
/// plugins may echo it back in their manifest so the host can warn on a mismatch.
pub const PROTOCOL_VERSION: u32 = 1;

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
    /// The protocol version the plugin was written against (optional). The host
    /// warns if it doesn't match [`PROTOCOL_VERSION`].
    #[serde(default, rename = "protocolVersion")]
    pub protocol_version: Option<u32>,
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

// --- registry (unified binding table / commands / event subscriptions) -------

use crate::keys::{canonical, CoreAction, KeyConfig};

/// What a chord triggers: a built-in action handled by the window, or a plugin
/// command (by id + owning plugin).
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Core(CoreAction),
    Plugin { command: String, plugin: PluginId },
}

/// The single source of truth for "which chord does what". Core actions and
/// plugin commands share **one** `keymap`, so `keys.toml` can rebind either.
/// Precedence (later wins on a chord collision): core defaults → plugin manifest
/// chords → user `keys.toml` overrides.
pub struct Registry {
    /// command id -> owning plugin.
    pub commands: HashMap<String, PluginId>,
    /// canonical chord -> action.
    pub keymap: HashMap<String, Action>,
    /// event name -> subscribed plugins.
    pub events: HashMap<String, Vec<PluginId>>,
    /// The canonical chord that opens a prefix sequence (default `ctrl+a`).
    pub prefix: String,
    /// User overrides: command id -> canonical chord (replace the plugin's own).
    command_overrides: HashMap<String, String>,
}

impl Registry {
    /// Seed the table from the key config: the prefix, every core action's chord,
    /// and the stored per-command overrides (applied as plugins register).
    pub fn new(cfg: &KeyConfig) -> Self {
        let mut keymap = HashMap::new();
        for (action, chord) in &cfg.core {
            keymap.insert(chord.clone(), Action::Core(*action));
        }
        Self {
            commands: HashMap::new(),
            keymap,
            events: HashMap::new(),
            prefix: cfg.prefix.clone(),
            command_overrides: cfg.commands.clone(),
        }
    }

    pub fn apply_manifest(&mut self, plugin: PluginId, m: &Manifest) {
        for c in &m.commands {
            self.commands.insert(c.id.clone(), plugin);
        }
        for k in &m.keybindings {
            // A user override for this command wins over the plugin's declared
            // chord; otherwise use what the plugin asked for (canonicalized).
            let chord = self
                .command_overrides
                .get(&k.command)
                .cloned()
                .or_else(|| canonical(&k.keys));
            let Some(chord) = chord else { continue };
            let action = Action::Plugin { command: k.command.clone(), plugin };
            if let Some(prev) = self.keymap.insert(chord.clone(), action) {
                if !matches!(&prev, Action::Plugin { command, .. } if *command == k.command) {
                    eprintln!("[keys] chord {chord:?} rebound to {} (was {prev:?})", k.command);
                }
            }
        }
        for e in &m.events {
            self.events.entry(e.clone()).or_default().push(plugin);
        }
    }

    /// Resolve a canonical chord to its action, if any.
    pub fn action_for_chord(&self, chord: &str) -> Option<&Action> {
        self.keymap.get(chord)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Registry::new(&KeyConfig::default())
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

    /// Handshake: ask the plugin to register itself, advertising our protocol
    /// version so the plugin can adapt (or refuse) if it speaks a different one.
    pub fn initialize(&self) {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"host":"jetem","protocolVersion":PROTOCOL_VERSION}}));
    }

    /// Tell the plugin one of its commands fired.
    pub fn invoke(&self, command: &str) {
        self.send(json!({"jsonrpc":"2.0","method":"command/invoke","params":{"command":command}}));
    }

    /// Reply to a `host/*` action request with `{ok: bool}`.
    pub fn reply(&self, id: Value, ok: bool) {
        self.reply_value(id, json!({ "ok": ok }));
    }

    /// Reply to a `host/*` request with an arbitrary result payload (e.g. the
    /// theme for `host/getTheme`).
    pub fn reply_value(&self, id: Value, result: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"result":result}));
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
            protocol_version: None,
        };
        let mut reg = Registry::default();
        reg.apply_manifest(3, &m);
        assert_eq!(
            reg.action_for_chord("prefix |"),
            Some(&Action::Plugin { command: "split".into(), plugin: 3 })
        );
        assert_eq!(reg.action_for_chord("prefix x"), None);
        // A core default still resolves in the same table.
        assert!(matches!(reg.action_for_chord("prefix r"), Some(Action::Core(_))));
    }

    #[test]
    fn user_override_beats_plugin_chord() {
        use crate::keys::KeyConfig;
        let mut cfg = KeyConfig::default();
        cfg.commands.insert("split".into(), "prefix v".into());
        let mut reg = Registry::new(&cfg);
        let m = Manifest {
            name: "mux".into(),
            commands: vec![CommandDef { id: "split".into(), title: String::new() }],
            keybindings: vec![KeyBinding { keys: "prefix |".into(), command: "split".into() }],
            events: vec![],
            protocol_version: None,
        };
        reg.apply_manifest(1, &m);
        // The override chord wins; the plugin's declared chord is not bound.
        assert!(matches!(reg.action_for_chord("prefix v"), Some(Action::Plugin { .. })));
        assert_eq!(reg.action_for_chord("prefix |"), None);
    }
}
