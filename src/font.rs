//! Font loading + glyph rasterization. We load a monospace TTF, derive the
//! fixed cell geometry from its metrics, and rasterize each char to a coverage
//! bitmap on demand (cached, so a repeated character is only rasterized once).

use std::collections::HashMap;
use std::rc::Rc;

use fontdue::{Font as FdFont, FontSettings, Metrics};
use serde::Deserialize;

/// The built-in font defaults (reproduced when there's no `font.toml`).
const DEFAULT_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
const DEFAULT_FALLBACKS: &[&str] = &[
    "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf",
];
const DEFAULT_PX: f32 = 16.0;

/// User font configuration from `~/.config/jetem/font.toml` — the primary font
/// path, pixel size, and fallback chain. Every field is optional; omitted ones
/// take the built-in default, so a `font.toml` can set just the size.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub path: String,
    pub size: f32,
    pub fallbacks: Vec<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_PATH.to_string(),
            size: DEFAULT_PX,
            fallbacks: DEFAULT_FALLBACKS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl FontConfig {
    /// Load `~/.config/jetem/font.toml`, falling back to defaults for the whole
    /// file (missing/malformed) or any individual omitted field.
    pub fn load() -> Self {
        match font_config_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => toml::from_str(&text).unwrap_or_default(),
            None => Self::default(),
        }
    }
}

fn font_config_path() -> Option<std::path::PathBuf> {
    crate::config::config_dir().map(|d| d.join("font.toml"))
}

/// A rasterized glyph: per-pixel coverage (0 = bg, 255 = full fg) plus the
/// metrics needed to position the bitmap within a cell.
pub struct Glyph {
    pub metrics: Metrics,
    pub coverage: Vec<u8>,
}

pub struct Font {
    /// The primary font; its metrics fix the cell geometry (assumed monospace).
    inner: FdFont,
    /// Fallback fonts, tried in order for any char the primary lacks — this is
    /// what lets non-Latin scripts (e.g. Hebrew, which DejaVu Sans Mono has no
    /// glyphs for) render at all. Cell geometry still comes from `inner`.
    fallbacks: Vec<FdFont>,
    px: f32,
    /// Fixed cell width/height in pixels (monospace ⇒ every cell is identical).
    pub cell_w: usize,
    pub cell_h: usize,
    /// Distance from the top of a cell down to the text baseline.
    pub baseline: usize,
    cache: HashMap<char, Rc<Glyph>>,
}

impl Font {
    /// Load a primary TTF plus zero or more fallback TTFs, computing cell geometry
    /// from the primary at the given pixel size. Fallback paths that don't exist
    /// or fail to parse are skipped (a missing fallback just means fewer scripts
    /// render, never a launch failure).
    pub fn load(path: &str, fallbacks: &[&str], px: f32) -> anyhow::Result<Self> {
        let data = std::fs::read(path)?;
        let inner = FdFont::from_bytes(data, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("failed to parse font {path}: {e}"))?;

        let loaded_fallbacks = fallbacks
            .iter()
            .filter_map(|p| {
                let data = std::fs::read(p).ok()?;
                FdFont::from_bytes(data, FontSettings::default()).ok()
            })
            .collect();

        let line = inner
            .horizontal_line_metrics(px)
            .ok_or_else(|| anyhow::anyhow!("font has no horizontal line metrics"))?;

        // Monospace: every glyph shares the same advance, so 'M' is representative.
        let cell_w = inner.metrics('M', px).advance_width.ceil() as usize;
        let cell_h = line.new_line_size.ceil() as usize; // ascent - descent + line_gap
        let baseline = line.ascent.ceil() as usize;

        Ok(Self {
            inner,
            fallbacks: loaded_fallbacks,
            px,
            cell_w,
            cell_h,
            baseline,
            cache: HashMap::new(),
        })
    }

    /// Load from a [`FontConfig`]. If the configured primary font can't be
    /// loaded (e.g. a bad path in `font.toml`), fall back to the built-in default
    /// font rather than refusing to launch — the fallbacks and size are kept.
    pub fn from_config(cfg: &FontConfig) -> anyhow::Result<Self> {
        let fallbacks: Vec<&str> = cfg.fallbacks.iter().map(String::as_str).collect();
        match Self::load(&cfg.path, &fallbacks, cfg.size) {
            Ok(font) => Ok(font),
            Err(e) => {
                eprintln!("[font] can't load {:?} ({e}); using the default font", cfg.path);
                Self::load(DEFAULT_PATH, &fallbacks, cfg.size)
            }
        }
    }

    /// The font that actually has a glyph for `ch`: the primary if it covers it,
    /// else the first fallback that does, else the primary (which rasterizes to a
    /// blank/`.notdef` — same as before fallbacks existed).
    fn font_for(&self, ch: char) -> &FdFont {
        if self.inner.lookup_glyph_index(ch) != 0 {
            return &self.inner;
        }
        self.fallbacks
            .iter()
            .find(|f| f.lookup_glyph_index(ch) != 0)
            .unwrap_or(&self.inner)
    }

    /// Rasterize `ch` (cached). Returns shared coverage bitmap + metrics.
    pub fn glyph(&mut self, ch: char) -> Rc<Glyph> {
        if let Some(g) = self.cache.get(&ch) {
            return g.clone();
        }
        let (metrics, coverage) = self.font_for(ch).rasterize(ch, self.px);
        let glyph = Rc::new(Glyph { metrics, coverage });
        self.cache.insert(ch, glyph.clone());
        glyph
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
    const HE_FALLBACK: &str = "/usr/share/fonts/truetype/freefont/FreeMono.ttf";

    #[test]
    fn font_config_partial_override_keeps_defaults() {
        // Only `size` is set; path + fallbacks must stay at the built-in defaults.
        let c: FontConfig = toml::from_str("size = 20.0").unwrap();
        assert_eq!(c.size, 20.0);
        assert_eq!(c.path, DEFAULT_PATH);
        assert_eq!(c.fallbacks.len(), DEFAULT_FALLBACKS.len());
        // A fully-default config reproduces today's hardcoded values.
        let d = FontConfig::default();
        assert_eq!(d.size, 16.0);
        assert_eq!(d.path, DEFAULT_PATH);
    }

    #[test]
    fn fallback_renders_a_glyph_the_primary_lacks() {
        // System-font dependent: skip cleanly if either font isn't installed.
        if !std::path::Path::new(PRIMARY).exists() || !std::path::Path::new(HE_FALLBACK).exists() {
            return;
        }
        let mut font = Font::load(PRIMARY, &[HE_FALLBACK], 16.0).unwrap();
        // The primary (DejaVu Sans Mono) has no Hebrew; without a fallback this
        // would rasterize to an empty (width 0) bitmap and draw nothing.
        let he = font.glyph('\u{05D2}'); // ג (gimel)
        assert!(he.metrics.width > 0, "Hebrew glyph should render via fallback");
        // Latin still comes from the primary and renders as before.
        assert!(font.glyph('A').metrics.width > 0);
    }
}
