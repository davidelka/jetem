//! Configurable keybindings. Every key action — the prefix, the built-in core
//! actions, and plugin commands — is addressed by a **canonical chord string**,
//! so `~/.config/jetem/keys.toml` can remap any of them without touching code.
//!
//! Two chord shapes:
//! - **Global** — modifiers + a key, e.g. `ctrl+shift+c`. Matched on a single
//!   keypress (used for the prefix itself, copy/paste, scrollback scroll).
//! - **Prefixed** — `prefix r`: a key pressed *after* the prefix key.
//!
//! Both are normalized to a canonical form (modifiers in a fixed order, lowercased)
//! so a config string and a live `KeyEvent` compare as equal `String`s — the map
//! key type for the whole binding table in `plugin::Registry`.

use std::collections::HashMap;

use serde::Deserialize;
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// A built-in action handled in-process by the window (not a plugin command).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreAction {
    /// Command-block recall overlay.
    Recall,
    /// Scrollback text search.
    Search,
    /// Send a literal prefix byte (Ctrl-A) to the shell.
    LiteralPrefix,
    Copy,
    Paste,
    ScrollUp,
    ScrollDown,
}

impl CoreAction {
    /// The built-in chord for each action (reproduced when no `keys.toml` exists).
    fn default_chord(self) -> &'static str {
        match self {
            CoreAction::Recall => "prefix r",
            CoreAction::Search => "prefix /",
            CoreAction::LiteralPrefix => "prefix a",
            CoreAction::Copy => "ctrl+shift+c",
            CoreAction::Paste => "ctrl+shift+v",
            CoreAction::ScrollUp => "shift+pageup",
            CoreAction::ScrollDown => "shift+pagedown",
        }
    }
    const ALL: [CoreAction; 7] = [
        CoreAction::Recall,
        CoreAction::Search,
        CoreAction::LiteralPrefix,
        CoreAction::Copy,
        CoreAction::Paste,
        CoreAction::ScrollUp,
        CoreAction::ScrollDown,
    ];
}

/// The name of a key as it appears in a chord string: a single lowercased char
/// (`a`, `/`, `|`) or a well-known named key (`pageup`, `enter`, `up`, …).
pub fn key_name(logical: &Key) -> Option<String> {
    Some(match logical {
        Key::Character(s) => s.to_lowercase(),
        Key::Named(NamedKey::PageUp) => "pageup".into(),
        Key::Named(NamedKey::PageDown) => "pagedown".into(),
        Key::Named(NamedKey::Enter) => "enter".into(),
        Key::Named(NamedKey::Space) => "space".into(),
        Key::Named(NamedKey::Tab) => "tab".into(),
        Key::Named(NamedKey::Escape) => "esc".into(),
        Key::Named(NamedKey::Backspace) => "backspace".into(),
        Key::Named(NamedKey::ArrowUp) => "up".into(),
        Key::Named(NamedKey::ArrowDown) => "down".into(),
        Key::Named(NamedKey::ArrowLeft) => "left".into(),
        Key::Named(NamedKey::ArrowRight) => "right".into(),
        Key::Named(NamedKey::Home) => "home".into(),
        Key::Named(NamedKey::End) => "end".into(),
        _ => return None,
    })
}

/// Canonical modifier prefix (`ctrl+`, `ctrl+shift+`, …) in a fixed order so the
/// same combination always renders identically.
fn mods_prefix(mods: ModifiersState) -> String {
    let mut s = String::new();
    if mods.control_key() {
        s.push_str("ctrl+");
    }
    if mods.alt_key() {
        s.push_str("alt+");
    }
    if mods.shift_key() {
        s.push_str("shift+");
    }
    if mods.super_key() {
        s.push_str("super+");
    }
    s
}

/// The canonical global chord a live keypress represents, e.g. `ctrl+shift+c`.
pub fn event_global_chord(event: &KeyEvent, mods: ModifiersState) -> Option<String> {
    Some(format!("{}{}", mods_prefix(mods), key_name(&event.logical_key)?))
}

/// The canonical prefixed chord a post-prefix keypress represents, e.g. `prefix r`.
pub fn event_prefixed_chord(event: &KeyEvent) -> Option<String> {
    Some(format!("prefix {}", key_name(&event.logical_key)?))
}

/// Normalize a config-file chord string to canonical form. Accepts either
/// `prefix <key>` or a modifier combo like `Ctrl+Shift+C` in any order/case.
/// Returns `None` for an unparseable spec (unknown modifier / empty key).
pub fn canonical(spec: &str) -> Option<String> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("prefix ").or_else(|| spec.strip_prefix("prefix+")) {
        let key = rest.trim().to_lowercase();
        return (!key.is_empty()).then(|| format!("prefix {key}"));
    }
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut sup = false;
    let mut key = None;
    for tok in spec.split('+') {
        match tok.trim().to_lowercase().as_str() {
            "" => return None,
            "ctrl" | "control" => ctrl = true,
            "alt" | "opt" | "option" => alt = true,
            "shift" => shift = true,
            "super" | "cmd" | "meta" | "win" => sup = true,
            k => {
                if key.is_some() {
                    return None; // two non-modifier tokens
                }
                key = Some(k.to_string());
            }
        }
    }
    let key = key?;
    // "prefix" is the sequence keyword, never a bindable key on its own.
    if key == "prefix" {
        return None;
    }
    let mut out = String::new();
    if ctrl {
        out.push_str("ctrl+");
    }
    if alt {
        out.push_str("alt+");
    }
    if shift {
        out.push_str("shift+");
    }
    if sup {
        out.push_str("super+");
    }
    out.push_str(&key);
    Some(out)
}

/// Raw `keys.toml` shape (all optional): a `prefix`, a `[core]` table of
/// action → chord overrides, and a `[commands]` table of plugin-command → chord.
#[derive(Debug, Default, Deserialize)]
struct RawKeys {
    prefix: Option<String>,
    #[serde(default)]
    core: HashMap<CoreAction, String>,
    #[serde(default)]
    commands: HashMap<String, String>,
}

/// The resolved key configuration: a canonical prefix chord, canonical chords for
/// every core action (defaults filled in), and any plugin-command overrides.
#[derive(Debug, Clone)]
pub struct KeyConfig {
    pub prefix: String,
    pub core: HashMap<CoreAction, String>,
    pub commands: HashMap<String, String>,
}

impl Default for KeyConfig {
    fn default() -> Self {
        let core = CoreAction::ALL
            .iter()
            .map(|a| (*a, canonical(a.default_chord()).unwrap()))
            .collect();
        Self { prefix: "ctrl+a".into(), core, commands: HashMap::new() }
    }
}

impl KeyConfig {
    /// Load `~/.config/jetem/keys.toml`, falling back to (and filling gaps from)
    /// the built-in defaults. A malformed file yields the defaults, like the theme.
    pub fn load() -> Self {
        let raw: RawKeys = match keys_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => toml::from_str(&text).unwrap_or_default(),
            None => RawKeys::default(),
        };
        let mut cfg = KeyConfig::default();
        if let Some(p) = raw.prefix.as_deref().and_then(canonical) {
            cfg.prefix = p;
        }
        for (action, spec) in &raw.core {
            if let Some(c) = canonical(spec) {
                cfg.core.insert(*action, c);
            }
        }
        for (cmd, spec) in &raw.commands {
            if let Some(c) = canonical(spec) {
                cfg.commands.insert(cmd.clone(), c);
            }
        }
        cfg
    }
}

fn keys_path() -> Option<std::path::PathBuf> {
    crate::config::config_dir().map(|d| d.join("keys.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_modifier_order_and_case() {
        assert_eq!(canonical("Shift+Ctrl+C").as_deref(), Some("ctrl+shift+c"));
        assert_eq!(canonical("ctrl+shift+c").as_deref(), Some("ctrl+shift+c"));
        assert_eq!(canonical("CMD+k").as_deref(), Some("super+k"));
        assert_eq!(canonical("prefix R").as_deref(), Some("prefix r"));
        assert_eq!(canonical("shift+pageup").as_deref(), Some("shift+pageup"));
    }

    #[test]
    fn rejects_bad_specs() {
        assert_eq!(canonical("ctrl+a+b"), None); // two keys
        assert_eq!(canonical("ctrl+"), None); // empty key
        assert_eq!(canonical("prefix "), None); // empty prefixed key
    }

    #[test]
    fn defaults_reproduce_current_bindings() {
        let c = KeyConfig::default();
        assert_eq!(c.prefix, "ctrl+a");
        assert_eq!(c.core[&CoreAction::Recall], "prefix r");
        assert_eq!(c.core[&CoreAction::Copy], "ctrl+shift+c");
        assert_eq!(c.core[&CoreAction::ScrollUp], "shift+pageup");
    }
}
