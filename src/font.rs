//! Font loading + glyph rasterization. We load a monospace TTF, derive the
//! fixed cell geometry from its metrics, and rasterize each char to a coverage
//! bitmap on demand (cached, so a repeated character is only rasterized once).

use std::collections::HashMap;
use std::rc::Rc;

use fontdue::{Font as FdFont, FontSettings, Metrics};

/// A rasterized glyph: per-pixel coverage (0 = bg, 255 = full fg) plus the
/// metrics needed to position the bitmap within a cell.
pub struct Glyph {
    pub metrics: Metrics,
    pub coverage: Vec<u8>,
}

pub struct Font {
    inner: FdFont,
    px: f32,
    /// Fixed cell width/height in pixels (monospace ⇒ every cell is identical).
    pub cell_w: usize,
    pub cell_h: usize,
    /// Distance from the top of a cell down to the text baseline.
    pub baseline: usize,
    cache: HashMap<char, Rc<Glyph>>,
}

impl Font {
    /// Load a TTF from disk and compute cell geometry at the given pixel size.
    pub fn load(path: &str, px: f32) -> anyhow::Result<Self> {
        let data = std::fs::read(path)?;
        let inner = FdFont::from_bytes(data, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("failed to parse font {path}: {e}"))?;

        let line = inner
            .horizontal_line_metrics(px)
            .ok_or_else(|| anyhow::anyhow!("font has no horizontal line metrics"))?;

        // Monospace: every glyph shares the same advance, so 'M' is representative.
        let cell_w = inner.metrics('M', px).advance_width.ceil() as usize;
        let cell_h = line.new_line_size.ceil() as usize; // ascent - descent + line_gap
        let baseline = line.ascent.ceil() as usize;

        Ok(Self {
            inner,
            px,
            cell_w,
            cell_h,
            baseline,
            cache: HashMap::new(),
        })
    }

    /// Rasterize `ch` (cached). Returns shared coverage bitmap + metrics.
    pub fn glyph(&mut self, ch: char) -> Rc<Glyph> {
        if let Some(g) = self.cache.get(&ch) {
            return g.clone();
        }
        let (metrics, coverage) = self.inner.rasterize(ch, self.px);
        let glyph = Rc::new(Glyph { metrics, coverage });
        self.cache.insert(ch, glyph.clone());
        glyph
    }
}
