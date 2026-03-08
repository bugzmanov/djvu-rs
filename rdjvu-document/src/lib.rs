use rdjvu_core::{Bitmap, Error, Pixmap};
use rdjvu_iff::{self, Chunk, DjvuFile};
use rdjvu_iw44::IW44Image;
use rdjvu_jb2::JB2Dict;

pub use rdjvu_iw44::NormalizedPlanes;

/// Rotation values from INFO chunk flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    None,
    Cw90,
    Cw180,
    Cw270,
}

/// Page metadata from the INFO chunk.
#[derive(Debug, Clone)]
pub struct PageInfo {
    pub width: u16,
    pub height: u16,
    pub dpi: u16,
    pub gamma: f32,
    pub rotation: Rotation,
    pub version: (u8, u8),
}

/// FGbz palette: per-blit color indices into an RGB palette.
#[derive(Debug, Clone)]
pub struct Palette {
    pub colors: Vec<(u8, u8, u8)>,
    pub indices: Vec<i16>,
}

/// Component type in DIRM directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentType {
    Shared,    // 0 — DJVI
    Page,      // 1 — DJVU
    Thumbnail, // 2 — THUM
}

/// A component entry from the DIRM directory.
#[derive(Debug, Clone)]
struct DirmEntry {
    comp_type: ComponentType,
    id: String,
}

/// A parsed DjVu document (single-page or multi-page bundled).
pub struct Document<'a> {
    file: DjvuFile<'a>,
    /// For DJVM: DIRM entries and FORM children (indexed by order in DIRM).
    dirm_entries: Vec<DirmEntry>,
    /// Indices into dirm_entries for page-type components only.
    page_indices: Vec<usize>,
    /// For single-page DJVU: true.
    is_single_page: bool,
}

impl<'a> Document<'a> {
    /// Parse a DjVu document from raw bytes.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let file = rdjvu_iff::parse(data)?;
        match &file.root {
            Chunk::Form { secondary_id, .. } if secondary_id == b"DJVU" => {
                Ok(Document {
                    file,
                    dirm_entries: vec![],
                    page_indices: vec![0], // single page at index 0
                    is_single_page: true,
                })
            }
            Chunk::Form { secondary_id, children, .. } if secondary_id == b"DJVM" => {
                // Find and parse DIRM chunk
                let dirm_chunk = children.iter().find_map(|c| match c {
                    Chunk::Leaf { id, data } if id == b"DIRM" => Some(*data),
                    _ => None,
                }).ok_or(Error::MissingChunk("DIRM"))?;

                let (dirm_entries, is_bundled) = parse_dirm(dirm_chunk)?;
                if !is_bundled {
                    return Err(Error::Unsupported("indirect DJVM not supported"));
                }

                let page_indices: Vec<usize> = dirm_entries.iter().enumerate()
                    .filter(|(_, e)| e.comp_type == ComponentType::Page)
                    .map(|(i, _)| i)
                    .collect();

                Ok(Document {
                    file,
                    dirm_entries,
                    page_indices,
                    is_single_page: false,
                })
            }
            _ => Err(Error::Unsupported("not a DJVU or DJVM document")),
        }
    }

    /// Number of pages (excluding thumbnails and shared components).
    pub fn page_count(&self) -> usize {
        self.page_indices.len()
    }

    /// Access a page by 0-based index.
    pub fn page(&self, index: usize) -> Result<Page<'_>, Error> {
        if index >= self.page_count() {
            return Err(Error::FormatError(format!("page index {} out of range ({})", index, self.page_count())));
        }

        if self.is_single_page {
            return Page::from_form(&self.file.root, self);
        }

        // Multi-page: find the FORM child corresponding to this page
        let dirm_index = self.page_indices[index];
        let form = self.get_component_form(dirm_index)?;
        Page::from_form(form, self)
    }

    /// Get the FORM chunk for a DIRM component by its dirm index.
    /// In bundled documents, FORM children after DIRM/NAVM correspond to DIRM entries in order.
    fn get_component_form(&self, dirm_index: usize) -> Result<&Chunk<'a>, Error> {
        let forms: Vec<&Chunk<'a>> = self.file.root.children().iter()
            .filter(|c| matches!(c, Chunk::Form { .. }))
            .collect();

        forms.get(dirm_index).copied()
            .ok_or(Error::FormatError(format!("component {} not found", dirm_index)))
    }

    /// Resolve an INCL reference to a shared DJVI component's children.
    fn resolve_incl(&self, ref_id: &str) -> Result<&Chunk<'a>, Error> {
        if self.is_single_page {
            return Err(Error::FormatError("INCL in single-page document".into()));
        }

        for (i, entry) in self.dirm_entries.iter().enumerate() {
            if entry.id == ref_id {
                return self.get_component_form(i);
            }
        }

        Err(Error::FormatError(format!("INCL target '{}' not found", ref_id)))
    }
}

/// A single page within a DjVu document.
pub struct Page<'a> {
    pub info: PageInfo,
    form: &'a Chunk<'a>,
    doc: &'a Document<'a>,
}

impl<'a> Page<'a> {
    fn from_form(form: &'a Chunk<'a>, doc: &'a Document<'a>) -> Result<Self, Error> {
        let info_chunk = form.find_first(b"INFO")
            .ok_or(Error::MissingChunk("INFO"))?;
        let info = parse_info(info_chunk.data())?;
        Ok(Page { info, form, doc })
    }

    pub fn has_mask(&self) -> bool {
        self.form.find_first(b"Sjbz").is_some()
    }

    pub fn has_background(&self) -> bool {
        self.form.find_first(b"BG44").is_some()
    }

    pub fn has_foreground(&self) -> bool {
        self.form.find_first(b"FG44").is_some()
    }

    pub fn has_palette(&self) -> bool {
        self.form.find_first(b"FGbz").is_some()
    }

    /// Decode the JB2 mask layer, resolving shared dictionaries via INCL.
    pub fn decode_mask(&self) -> Result<Option<Bitmap>, Error> {
        let sjbz = match self.form.find_first(b"Sjbz") {
            Some(c) => c.data(),
            None => return Ok(None),
        };

        let shared_dict = self.resolve_shared_dict()?;

        let bitmap = rdjvu_jb2::decode(sjbz, shared_dict.as_ref())
            .map_err(|e| Error::FormatError(e.to_string()))?;
        Ok(Some(bitmap))
    }

    /// Decode the JB2 mask with per-pixel blit index map (for FGbz palette compositing).
    pub fn decode_mask_indexed(&self) -> Result<Option<(Bitmap, Vec<i32>)>, Error> {
        let sjbz = match self.form.find_first(b"Sjbz") {
            Some(c) => c.data(),
            None => return Ok(None),
        };

        let shared_dict = self.resolve_shared_dict()?;

        let result = rdjvu_jb2::decode_indexed(sjbz, shared_dict.as_ref())
            .map_err(|e| Error::FormatError(e.to_string()))?;
        Ok(Some(result))
    }

    /// Decode the IW44 background layer.
    pub fn decode_background(&self) -> Result<Option<Pixmap>, Error> {
        let img = match self.decode_iw44_layer(b"BG44")? {
            Some(img) => img,
            None => return Ok(None),
        };
        let pm = img.to_pixmap()
            .map_err(|e| Error::FormatError(e.to_string()))?;
        Ok(Some(pm))
    }

    /// Decode the IW44 foreground layer.
    pub fn decode_foreground(&self) -> Result<Option<Pixmap>, Error> {
        let img = match self.decode_iw44_layer(b"FG44")? {
            Some(img) => img,
            None => return Ok(None),
        };
        let pm = img.to_pixmap()
            .map_err(|e| Error::FormatError(e.to_string()))?;
        Ok(Some(pm))
    }

    pub fn decode_background_planes(&self) -> Result<Option<NormalizedPlanes>, Error> {
        let img = match self.decode_iw44_layer(b"BG44")? {
            Some(img) => img,
            None => return Ok(None),
        };
        let planes = img.to_normalized_planes_subsample(1)
            .map_err(|e| Error::FormatError(e.to_string()))?;
        Ok(Some(planes))
    }

    /// Parse the FGbz palette chunk.
    pub fn decode_palette(&self) -> Result<Option<Palette>, Error> {
        let fgbz = match self.form.find_first(b"FGbz") {
            Some(c) => c.data(),
            None => return Ok(None),
        };
        let palette = parse_fgbz(fgbz)?;
        Ok(Some(palette))
    }

    fn resolve_shared_dict(&self) -> Result<Option<JB2Dict>, Error> {
        // First check for INCL → external DJVI component with Djbz
        if let Some(incl) = self.form.find_first(b"INCL") {
            let ref_id = std::str::from_utf8(incl.data())
                .map_err(|_| Error::FormatError("invalid INCL UTF-8".into()))?
                .trim_end_matches('\0')
                .trim();

            let shared_form = self.doc.resolve_incl(ref_id)?;
            let djbz = match shared_form.find_first(b"Djbz") {
                Some(c) => c.data(),
                None => return Err(Error::MissingChunk("Djbz in shared component")),
            };

            let dict = rdjvu_jb2::decode_dict(djbz, None)
                .map_err(|e| Error::FormatError(e.to_string()))?;
            return Ok(Some(dict));
        }

        // Then check for inline Djbz in the same FORM as Sjbz
        if let Some(djbz) = self.form.find_first(b"Djbz") {
            let dict = rdjvu_jb2::decode_dict(djbz.data(), None)
                .map_err(|e| Error::FormatError(e.to_string()))?;
            return Ok(Some(dict));
        }

        Ok(None)
    }

    fn decode_iw44_layer(&self, chunk_id: &[u8; 4]) -> Result<Option<IW44Image>, Error> {
        let chunks: Vec<&[u8]> = self.form.find_all(chunk_id)
            .into_iter()
            .map(|c| c.data())
            .collect();

        if chunks.is_empty() {
            return Ok(None);
        }

        let mut img = IW44Image::new();
        for chunk_data in &chunks {
            img.decode_chunk(chunk_data)
                .map_err(|e| Error::FormatError(e.to_string()))?;
        }
        Ok(Some(img))
    }
}

// ============================================================
// INFO chunk parser
// ============================================================

fn parse_info(data: &[u8]) -> Result<PageInfo, Error> {
    if data.len() < 5 {
        return Err(Error::InvalidLength);
    }

    let width = u16::from_be_bytes([data[0], data[1]]);
    let height = u16::from_be_bytes([data[2], data[3]]);
    let minver = data[4];
    let majver = if data.len() > 5 { data[5] } else { 0 };

    // DPI is little-endian (unusual for IFF)
    let raw_dpi = if data.len() >= 8 {
        u16::from_le_bytes([data[6], data[7]])
    } else {
        300
    };
    let dpi = if (25..=6000).contains(&raw_dpi) { raw_dpi } else { 300 };

    let raw_gamma = if data.len() >= 9 { data[8] } else { 22 };
    let gamma_val = raw_gamma.clamp(3, 50);
    let gamma = gamma_val as f32 / 10.0;

    let flags = if data.len() >= 10 { data[9] } else { 0 };
    let rotation = match flags & 0x07 {
        5 => Rotation::Cw90,
        2 => Rotation::Cw180,
        6 => Rotation::Cw270,
        _ => Rotation::None,
    };

    Ok(PageInfo {
        width,
        height,
        dpi,
        gamma,
        rotation,
        version: (majver, minver),
    })
}

// ============================================================
// DIRM chunk parser
// ============================================================

fn parse_dirm(data: &[u8]) -> Result<(Vec<DirmEntry>, bool), Error> {
    if data.len() < 3 {
        return Err(Error::InvalidLength);
    }

    let dflags = data[0];
    let is_bundled = (dflags >> 7) != 0;
    let nfiles = u16::from_be_bytes([data[1], data[2]]) as usize;

    let mut pos = 3;

    // Skip offsets array for bundled documents
    if is_bundled {
        let offsets_size = nfiles * 4;
        if pos + offsets_size > data.len() {
            return Err(Error::UnexpectedEof);
        }
        pos += offsets_size;
    }

    // Remaining bytes are BZZ-compressed metadata
    let bzz_data = &data[pos..];
    let meta = rdjvu_bzz::decode(bzz_data)
        .map_err(|e| Error::FormatError(e.to_string()))?;

    // Parse metadata: for each component, read size(3), flags(1), id(strNT), name?(strNT), title?(strNT)
    let mut mpos = 0;
    // First: skip sizes (3 bytes each)
    mpos += nfiles * 3;

    // Read flags for all components
    if mpos + nfiles > meta.len() {
        return Err(Error::UnexpectedEof);
    }
    let flags: Vec<u8> = meta[mpos..mpos + nfiles].to_vec();
    mpos += nfiles;

    // Read IDs and optional name/title strings
    let mut entries = Vec::with_capacity(nfiles);
    for i in 0..nfiles {
        let id = read_str_nt(&meta, &mut mpos)?;
        let has_name = (flags[i] & 0x80) != 0;
        let has_title = (flags[i] & 0x40) != 0;
        if has_name {
            let _ = read_str_nt(&meta, &mut mpos)?;
        }
        if has_title {
            let _ = read_str_nt(&meta, &mut mpos)?;
        }

        let comp_type = match flags[i] & 0x3f {
            0 => ComponentType::Shared,
            1 => ComponentType::Page,
            2 => ComponentType::Thumbnail,
            t => return Err(Error::Unsupported(
                // Use a static message since we can't format dynamic values into &'static str
                if t <= 63 { "unknown DIRM component type" } else { "unknown DIRM component type" }
            )),
        };

        entries.push(DirmEntry { comp_type, id });
    }

    Ok((entries, is_bundled))
}

fn read_str_nt(data: &[u8], pos: &mut usize) -> Result<String, Error> {
    let start = *pos;
    while *pos < data.len() && data[*pos] != 0 {
        *pos += 1;
    }
    if *pos >= data.len() {
        return Err(Error::UnexpectedEof);
    }
    let s = std::str::from_utf8(&data[start..*pos])
        .map_err(|_| Error::FormatError("invalid UTF-8 in DIRM".into()))?;
    *pos += 1; // skip null terminator
    Ok(s.to_string())
}

// ============================================================
// FGbz palette parser
// ============================================================

fn parse_fgbz(data: &[u8]) -> Result<Palette, Error> {
    if data.len() < 3 {
        return Err(Error::InvalidLength);
    }

    let version = data[0];
    if (version & 0x7f) != 0 {
        return Err(Error::Unsupported("unsupported FGbz version"));
    }

    let palette_size = u16::from_be_bytes([data[1], data[2]]) as usize;
    let color_bytes = palette_size * 3;
    if data.len() < 3 + color_bytes {
        return Err(Error::UnexpectedEof);
    }

    // Colors are stored as BGR triplets
    let mut colors = Vec::with_capacity(palette_size);
    for i in 0..palette_size {
        let base = 3 + i * 3;
        let b = data[base];
        let g = data[base + 1];
        let r = data[base + 2];
        colors.push((r, g, b));
    }

    let mut indices = Vec::new();
    if (version & 0x80) != 0 {
        let idx_start = 3 + color_bytes;
        if idx_start + 3 > data.len() {
            return Err(Error::UnexpectedEof);
        }
        let data_size = ((data[idx_start] as u32) << 16)
            | ((data[idx_start + 1] as u32) << 8)
            | (data[idx_start + 2] as u32);

        let bzz_data = &data[idx_start + 3..];
        let decoded = rdjvu_bzz::decode(bzz_data)
            .map_err(|e| Error::FormatError(e.to_string()))?;

        // Each index is i16be
        let num_indices = data_size as usize;
        if decoded.len() < num_indices * 2 {
            return Err(Error::UnexpectedEof);
        }
        indices.reserve(num_indices);
        for i in 0..num_indices {
            let idx = i16::from_be_bytes([decoded[i * 2], decoded[i * 2 + 1]]);
            indices.push(idx);
        }
    }

    Ok(Palette { colors, indices })
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
            .join("tests/golden/document")
    }

    #[test]
    fn page_counts() {
        let cases: &[(&str, usize)] = &[
            ("boy_jb2.djvu", 1),
            ("boy.djvu", 1),
            ("chicken.djvu", 1),
            ("navm_fgbz.djvu", 6),
            ("DjVu3Spec_bundled.djvu", 71),
            ("colorbook.djvu", 62),
        ];
        for (file, expected) in cases {
            let data = std::fs::read(assets_path().join(file)).unwrap();
            let doc = Document::parse(&data).unwrap();
            assert_eq!(doc.page_count(), *expected, "page count mismatch for {}", file);
        }
    }

    #[test]
    fn page_dimensions_navm_fgbz() {
        let data = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();

        let golden = std::fs::read_to_string(golden_path().join("navm_fgbz_sizes.txt")).unwrap();
        for (i, line) in golden.lines().enumerate() {
            let page = doc.page(i).unwrap();
            let expected = format!("width={} height={}", page.info.width, page.info.height);
            assert_eq!(expected, line.trim(), "size mismatch for navm_fgbz page {}", i + 1);
        }
    }

    #[test]
    fn page_dimensions_djvu3spec() {
        let data = std::fs::read(assets_path().join("DjVu3Spec_bundled.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();

        let golden = std::fs::read_to_string(golden_path().join("djvu3spec_bundled_sizes.txt")).unwrap();
        for (i, line) in golden.lines().enumerate() {
            if line.trim().is_empty() { continue; }
            let page = doc.page(i).unwrap();
            let expected = format!("width={} height={}", page.info.width, page.info.height);
            assert_eq!(expected, line.trim(), "size mismatch for djvu3spec page {}", i + 1);
        }
    }

    #[test]
    fn layer_availability() {
        // boy_jb2: mask only
        let data = std::fs::read(assets_path().join("boy_jb2.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let p = doc.page(0).unwrap();
        assert!(p.has_mask());
        assert!(!p.has_background());
        assert!(!p.has_foreground());
        assert!(!p.has_palette());

        // chicken: background only
        let data = std::fs::read(assets_path().join("chicken.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let p = doc.page(0).unwrap();
        assert!(!p.has_mask());
        assert!(p.has_background());
        assert!(!p.has_foreground());
        assert!(!p.has_palette());

        // navm_fgbz p1: mask + palette + background
        let data = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let p = doc.page(0).unwrap();
        assert!(p.has_mask());
        assert!(p.has_background());
        assert!(!p.has_foreground());
        assert!(p.has_palette());
    }

    #[test]
    fn decode_mask_matches_direct_boy_jb2() {
        let data = std::fs::read(assets_path().join("boy_jb2.djvu")).unwrap();

        // Via document API
        let doc = Document::parse(&data).unwrap();
        let mask_via_doc = doc.page(0).unwrap().decode_mask().unwrap().unwrap();

        // Via direct JB2 decode
        let file = rdjvu_iff::parse(&data).unwrap();
        let sjbz = file.root.find_first(b"Sjbz").unwrap();
        let mask_direct = rdjvu_jb2::decode(sjbz.data(), None).unwrap();

        assert_eq!(mask_via_doc.data, mask_direct.data, "mask data mismatch");
    }

    #[test]
    fn decode_mask_with_shared_dict() {
        // navm_fgbz page 1 uses INCL → dict0006.iff
        let data = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let mask = doc.page(0).unwrap().decode_mask().unwrap();
        assert!(mask.is_some(), "expected mask for navm_fgbz p1");
        let bm = mask.unwrap();
        assert_eq!(bm.width, 2550);
        assert_eq!(bm.height, 3300);
    }

    #[test]
    fn decode_background_chicken() {
        let data = std::fs::read(assets_path().join("chicken.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let bg = doc.page(0).unwrap().decode_background().unwrap();
        assert!(bg.is_some());
        let pm = bg.unwrap();
        assert_eq!(pm.width, 181);
        assert_eq!(pm.height, 240);
    }

    #[test]
    fn decode_palette_navm_fgbz() {
        let data = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let pal = doc.page(0).unwrap().decode_palette().unwrap();
        assert!(pal.is_some());
        let p = pal.unwrap();
        assert_eq!(p.colors.len(), 2); // FGbz with 2 colors per dump
    }

    #[test]
    fn page_info_dpi() {
        let data = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let p = doc.page(0).unwrap();
        assert_eq!(p.info.dpi, 300);
    }

    #[test]
    fn debug_bg_lowres_vs_ddjvu() {
        let cases = [
            ("carte.djvu", 0usize, "/tmp/rdjvu_debug/carte_bg_sub3.ppm"),
            ("colorbook.djvu", 0usize, "/tmp/rdjvu_debug/colorbook_p1_bg_sub3.ppm"),
            ("navm_fgbz.djvu", 3usize, "/tmp/rdjvu_debug/navm_p4_bg_sub3.ppm"),
        ];
        for (file, page_idx, ref_file) in cases {
            let ref_path = std::path::Path::new(ref_file);
            if !ref_path.exists() {
                continue;
            }
            let data = std::fs::read(assets_path().join(file)).unwrap();
            let doc = Document::parse(&data).unwrap();
            let page = doc.page(page_idx).unwrap();
            let bg = page.decode_background().unwrap().unwrap();
            let actual = bg.to_ppm();
            let expected = std::fs::read(ref_path).unwrap();
            let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let a = &actual[header_end..];
            let e = &expected[header_end..];
            let mut diff_px = 0usize;
            let mut abs = [0u64; 3];
            let px = (a.len().min(e.len())) / 3;
            for p in 0..px {
                let i = p * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    diff_px += 1;
                }
                abs[0] += (a[i] as i32 - e[i] as i32).unsigned_abs() as u64;
                abs[1] += (a[i + 1] as i32 - e[i + 1] as i32).unsigned_abs() as u64;
                abs[2] += (a[i + 2] as i32 - e[i + 2] as i32).unsigned_abs() as u64;
            }
            eprintln!(
                "{} p{} bg-lowres mismatch_px={} mean_abs=({:.3},{:.3},{:.3}) dims_a={} dims_e={}",
                file,
                page_idx + 1,
                diff_px,
                abs[0] as f64 / px as f64,
                abs[1] as f64 / px as f64,
                abs[2] as f64 / px as f64,
                a.len() / 3,
                e.len() / 3
            );
        }
    }
}
