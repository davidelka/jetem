//! Configuration: for now just the list of plugins to launch, read from
//! `$XDG_CONFIG_HOME/terminal/plugins.toml` (default `~/.config/...`). Plugins
//! are explicit opt-in — nothing runs unless listed here.
//!
//! ```toml
//! [[plugin]]
//! command = "python3 /path/to/plugin.py"
//! ```

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub plugin: Vec<PluginConfig>,
}

#[derive(Debug, Deserialize)]
pub struct PluginConfig {
    pub command: String,
}

/// Load the config, or an empty config if absent/unparseable.
pub fn load() -> Config {
    match config_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(text) => toml::from_str(&text).unwrap_or_default(),
        None => Config::default(),
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("terminal").join("plugins.toml"))
}
