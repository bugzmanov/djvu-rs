//! Pure Rust DjVu renderer.
//!
//! # Example
//!
//! ```no_run
//! let doc = rdjvu::Document::open("file.djvu").unwrap();
//! println!("{} pages", doc.page_count());
//!
//! let page = doc.page(0).unwrap();
//! println!("{}x{} @ {} dpi", page.width(), page.height(), page.dpi());
//!
//! let pixmap = page.render().unwrap();
//! // pixmap.data: RGBA, pixmap.to_rgb(): RGB
//! ```

pub use rdjvu_core::{Bitmap, Error, Pixmap};

/// A parsed DjVu document. Owns its data and the parsed structure.
///
/// Parsing happens once at construction time. All subsequent `page()` and
/// `render()` calls reuse the parsed chunk tree with zero re-parsing overhead.
pub struct Document {
    // SAFETY: `parsed` borrows from `_data`. This is sound because:
    // 1. `_data` is heap-allocated (`Box<[u8]>`) — its address is stable across moves.
    // 2. Fields drop in declaration order: `parsed` drops first, releasing borrows
    //    before `_data` is freed.
    // 3. `_data` is never mutated after construction.
    parsed: rdjvu_document::Document<'static>,
    _data: Box<[u8]>,
}

impl Document {
    /// Open a DjVu file from disk.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let data = std::fs::read(path.as_ref())
            .map_err(|e| Error::FormatError(format!("failed to read file: {}", e)))?;
        Self::from_bytes(data)
    }

    /// Parse a DjVu document from owned bytes.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, Error> {
        let data = data.into_boxed_slice();
        // SAFETY: Extend the borrow lifetime to 'static. Sound because `data` is
        // heap-allocated, never moved or mutated, and outlives `parsed` (see field
        // drop order above).
        let stable_ref: &'static [u8] =
            unsafe { std::slice::from_raw_parts(data.as_ptr(), data.len()) };
        let parsed = rdjvu_document::Document::parse(stable_ref)?;
        Ok(Document { parsed, _data: data })
    }

    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.parsed.page_count()
    }

    /// Access a page by 0-based index.
    pub fn page(&self, index: usize) -> Result<Page<'_>, Error> {
        let inner = self.parsed.page(index)?;
        Ok(Page {
            width: inner.info.width,
            height: inner.info.height,
            dpi: inner.info.dpi,
            rotation: inner.info.rotation,
            index,
            doc: self,
        })
    }
}

/// A page within a DjVu document.
pub struct Page<'a> {
    width: u16,
    height: u16,
    dpi: u16,
    rotation: rdjvu_document::Rotation,
    index: usize,
    doc: &'a Document,
}

impl<'a> Page<'a> {
    /// Page width in pixels (before rotation).
    pub fn width(&self) -> u32 {
        self.width as u32
    }

    /// Page height in pixels (before rotation).
    pub fn height(&self) -> u32 {
        self.height as u32
    }

    /// Effective page width after rotation.
    pub fn display_width(&self) -> u32 {
        match self.rotation {
            rdjvu_document::Rotation::Cw90 | rdjvu_document::Rotation::Cw270 => self.height as u32,
            _ => self.width as u32,
        }
    }

    /// Effective page height after rotation.
    pub fn display_height(&self) -> u32 {
        match self.rotation {
            rdjvu_document::Rotation::Cw90 | rdjvu_document::Rotation::Cw270 => self.width as u32,
            _ => self.height as u32,
        }
    }

    /// Page resolution in dots per inch.
    pub fn dpi(&self) -> u16 {
        self.dpi
    }

    /// Render the page to an RGBA pixmap at native resolution.
    pub fn render(&self) -> Result<Pixmap, Error> {
        let page = self.doc.parsed.page(self.index)?;
        rdjvu_render::render(&page)
    }

    /// Render the page to an RGBA pixmap at a target size.
    ///
    /// Layers are composited directly at the target resolution —
    /// no intermediate full-size buffer is allocated.
    pub fn render_to_size(&self, width: u32, height: u32) -> Result<Pixmap, Error> {
        let page = self.doc.parsed.page(self.index)?;
        rdjvu_render::render_to_size(&page, width, height)
    }

    /// Render the page scaled by a factor (e.g. 0.5 = half size, 2.0 = double).
    pub fn render_scaled(&self, scale: f32) -> Result<Pixmap, Error> {
        let dw = self.display_width();
        let dh = self.display_height();
        let w = ((dw as f32 * scale).round() as u32).max(1);
        let h = ((dh as f32 * scale).round() as u32).max(1);
        // render_to_size works in pre-rotation coords
        let (tw, th) = match self.rotation {
            rdjvu_document::Rotation::Cw90 | rdjvu_document::Rotation::Cw270 => (h, w),
            _ => (w, h),
        };
        let page = self.doc.parsed.page(self.index)?;
        rdjvu_render::render_to_size(&page, tw, th)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("references/djvujs/library/assets")
    }

    fn golden_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/golden/composite")
    }

    #[test]
    fn open_and_page_count() {
        let cases: &[(&str, usize)] = &[
            ("boy_jb2.djvu", 1),
            ("chicken.djvu", 1),
            ("navm_fgbz.djvu", 6),
            ("DjVu3Spec_bundled.djvu", 71),
            ("colorbook.djvu", 62),
        ];
        for (file, expected) in cases {
            let doc = Document::open(assets_path().join(file)).unwrap();
            assert_eq!(doc.page_count(), *expected, "{}", file);
        }
    }

    #[test]
    fn page_dimensions() {
        let doc = Document::open(assets_path().join("chicken.djvu")).unwrap();
        let page = doc.page(0).unwrap();
        assert_eq!(page.width(), 181);
        assert_eq!(page.height(), 240);
        assert_eq!(page.dpi(), 100);
    }

    #[test]
    fn render_all_test_files() {
        let files = [
            "boy_jb2.djvu",
            "boy.djvu",
            "chicken.djvu",
            "carte.djvu",
            "navm_fgbz.djvu",
            "DjVu3Spec_bundled.djvu",
            "colorbook.djvu",
            "big-scanned-page.djvu",
        ];
        for file in &files {
            let doc = Document::open(assets_path().join(file)).unwrap();
            let page = doc.page(0).unwrap();
            let pixmap = page.render().unwrap();
            assert!(pixmap.width > 0, "{}: zero width", file);
            assert!(pixmap.height > 0, "{}: zero height", file);
            assert_eq!(
                pixmap.data.len(),
                pixmap.width as usize * pixmap.height as usize * 4,
                "{}: data length mismatch", file
            );
        }
    }

    #[test]
    fn render_pixel_exact_chicken() {
        let doc = Document::open(assets_path().join("chicken.djvu")).unwrap();
        let pixmap = doc.page(0).unwrap().render().unwrap();
        let expected = std::fs::read(golden_path().join("chicken.ppm")).unwrap();
        assert_eq!(pixmap.to_ppm(), expected);
    }

    #[test]
    fn render_pixel_exact_boy_jb2() {
        let doc = Document::open(assets_path().join("boy_jb2.djvu")).unwrap();
        let pixmap = doc.page(0).unwrap().render().unwrap();
        let expected = std::fs::read(golden_path().join("boy_jb2.ppm")).unwrap();
        assert_eq!(pixmap.to_ppm(), expected);
    }

    #[test]
    fn to_rgb_output() {
        let doc = Document::open(assets_path().join("chicken.djvu")).unwrap();
        let pixmap = doc.page(0).unwrap().render().unwrap();
        let rgb = pixmap.to_rgb();
        assert_eq!(rgb.len(), pixmap.width as usize * pixmap.height as usize * 3);
    }

    #[test]
    fn from_bytes() {
        let data = std::fs::read(assets_path().join("boy_jb2.djvu")).unwrap();
        let doc = Document::from_bytes(data).unwrap();
        assert_eq!(doc.page_count(), 1);
    }

    #[test]
    fn display_dimensions_with_rotation() {
        let doc = Document::open(assets_path().join("boy_jb2_rotate90.djvu")).unwrap();
        let page = doc.page(0).unwrap();
        let w = page.width();
        let h = page.height();
        // Rotated 90° swaps dimensions
        assert_eq!(page.display_width(), h);
        assert_eq!(page.display_height(), w);
    }
}
