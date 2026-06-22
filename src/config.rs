//! Configuration: the list of plugins to launch. Two sources, merged:
//!
//! 1. **`plugins.toml`** — explicit entries, each a full launch command. Use this
//!    when a plugin needs arguments or environment (e.g. `env FOO=bar python3 x.py`).
//!
//!    ```toml
//!    [[plugin]]
//!    command = "python3 /path/to/plugin.py"
//!    ```
//!
//! 2. **Drop-in directory** — every file in `~/.config/jetem/plugins/` loads
//!    automatically, no TOML edit. An executable file is run directly (its shebang
//!    chooses the interpreter); otherwise a known extension maps to one
//!    (`.py`→`python3`, `.js`→`node`, `.sh`→`sh`). Dropping the file is the opt-in.
//!
//! Both live under `$XDG_CONFIG_HOME/jetem/` (default `~/.config/jetem/`).

use std::path::{Path, PathBuf};

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

/// Load the TOML plugins, then append any drop-in plugins discovered in the
/// `plugins/` directory. An empty config if nothing is configured.
pub fn load() -> Config {
    let mut config = match config_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(text) => toml::from_str(&text).unwrap_or_default(),
        None => Config::default(),
    };
    config.plugin.extend(scan_drop_in(&config.plugin));
    config
}

/// Discover plugins dropped into `~/.config/jetem/plugins/`. Skips hidden
/// files, non-files, files of unknown type, and any file already referenced by a
/// TOML `command` (so it doesn't load twice). Sorted for a stable load order.
fn scan_drop_in(existing: &[PluginConfig]) -> Vec<PluginConfig> {
    let Some(dir) = plugins_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new(); // no drop-in dir is normal — not an error
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue; // hidden / editor temp files
        }
        let path_str = path.to_string_lossy().to_string();
        if existing.iter().any(|p| p.command.contains(&path_str)) {
            continue; // already launched via TOML
        }
        match command_for(&path) {
            Some(command) => out.push(PluginConfig { command }),
            None => eprintln!(
                "[jetem] skipping drop-in plugin (not executable, unknown type): {path_str}"
            ),
        }
    }
    out.sort_by(|a, b| a.command.cmp(&b.command));
    out
}

/// How to launch a dropped file: directly if executable (its shebang runs it),
/// else via a known extension's interpreter. `None` if we can't tell.
fn command_for(path: &Path) -> Option<String> {
    let path_str = path.to_string_lossy();
    if is_executable(path) {
        return Some(path_str.into_owned());
    }
    let interp = match path.extension().and_then(|e| e.to_str()) {
        Some("py") => "python3",
        Some("js") => "node",
        Some("sh") => "sh",
        _ => return None,
    };
    Some(format!("{interp} {path_str}"))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

pub(crate) fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("jetem"))
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("plugins.toml"))
}

fn plugins_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("plugins"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_in_maps_known_extensions() {
        // Non-existent paths aren't executable, so they fall to the extension map.
        assert_eq!(
            command_for(Path::new("/p/foo.py")).as_deref(),
            Some("python3 /p/foo.py")
        );
        assert_eq!(
            command_for(Path::new("/p/foo.js")).as_deref(),
            Some("node /p/foo.js")
        );
    }

    #[test]
    fn drop_in_skips_unknown_types() {
        assert_eq!(command_for(Path::new("/p/notes.txt")), None);
        assert_eq!(command_for(Path::new("/p/noext")), None);
    }
}
