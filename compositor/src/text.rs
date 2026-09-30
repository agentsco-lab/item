//! Text for the shell: fonts from the system, rasterized on the CPU with
//! fontdue into a texture, once per change of the text. Moving a label does
//! not touch it again: the texture only takes a new place in the frame.
//!
//! Labels are rasterized at the output's scale, so a glyph's pixels are the
//! panel's pixels.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Size, Transform};

use crate::layout::SCALE;

pub struct Font(fontdue::Font);

impl Font {
    /// The first of `paths` that loads.
    pub fn load(paths: &[&str]) -> Option<Font> {
        paths.iter().find_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            match fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                Ok(font) => {
                    tracing::info!("font: {path}");
                    Some(Font(font))
                }
                Err(e) => {
                    tracing::warn!("font {path}: {e}");
                    None
                }
            }
        })
    }

    /// `text` in one line, `size` logical px high, white: premultiplied
    /// RGBA and its size in physical px.
    fn rasterize(&self, text: &str, size: f32, color: [f32; 4]) -> (Vec<u8>, i32, i32) {
        let px = size * SCALE as f32;
        let line = self.0.horizontal_line_metrics(px).expect("a horizontal font");
        let mut glyphs = Vec::new();
        let mut pen = 0.0f32;
        let mut prev = None;
        for c in text.chars() {
            if let Some(p) = prev {
                pen += self.0.horizontal_kern(p, c, px).unwrap_or(0.0);
            }
            let (metrics, coverage) = self.0.rasterize(c, px);
            glyphs.push((pen.round() as i32 + metrics.xmin, metrics, coverage));
            pen += metrics.advance_width;
            prev = Some(c);
        }
        // Even, so the logical size is whole.
        let even = |v: i32| (v + 1) & !1;
        let (w, h) = (even((pen.ceil() as i32).max(1)), even((line.ascent - line.descent).ceil() as i32));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (x0, m, coverage) in glyphs {
            // fontdue's ymin is the glyph's bottom above the baseline.
            let y0 = line.ascent.round() as i32 - m.height as i32 - m.ymin;
            for gy in 0..m.height as i32 {
                for gx in 0..m.width as i32 {
                    let (x, y) = (x0 + gx, y0 + gy);
                    if x < 0 || y < 0 || x >= w || y >= h {
                        continue;
                    }
                    let a = coverage[(gy * m.width as i32 + gx) as usize] as f32 / 255.0 * color[3];
                    let i = ((y * w + x) * 4) as usize;
                    for (k, c) in color[..3].iter().enumerate() {
                        rgba[i + k] = rgba[i + k].max((c * a * 255.0).round() as u8);
                    }
                    rgba[i + 3] = rgba[i + 3].max((a * 255.0).round() as u8);
                }
            }
        }
        (rgba, w, h)
    }
}

/// A line of text as a texture, remade only when the text changes.
pub struct Label {
    size: f32,
    color: [f32; 4],
    text: String,
    pub buffer: MemoryRenderBuffer,
    /// Logical px.
    pub extent: Size<i32, Logical>,
}

impl Label {
    pub fn new(size: f32, color: [f32; 4]) -> Label {
        Label {
            size,
            color,
            text: String::new(),
            buffer: MemoryRenderBuffer::new(Fourcc::Abgr8888, (1, 1), SCALE, Transform::Normal, None),
            extent: (0, 0).into(),
        }
    }

    /// Sets the text; returns whether it changed.
    pub fn set(&mut self, font: &Font, text: &str) -> bool {
        if self.text == text {
            return false;
        }
        let (rgba, w, h) = font.rasterize(text, self.size, self.color);
        // Abgr8888 is R, G, B, A in memory.
        self.buffer = MemoryRenderBuffer::from_slice(&rgba, Fourcc::Abgr8888, (w, h), SCALE, Transform::Normal, None);
        self.extent = (w / SCALE, h / SCALE).into();
        self.text = text.to_owned();
        true
    }
}
