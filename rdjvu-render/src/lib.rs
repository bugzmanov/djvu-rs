use rdjvu_core::{Bitmap, Error, Pixmap};
use rdjvu_document::{Page, Palette, Rotation};

/// Render a DjVu page to an RGBA pixmap at native resolution.
pub fn render(page: &Page) -> Result<Pixmap, Error> {
    render_to_size(page, page.info.width as u32, page.info.height as u32)
}

/// Render a DjVu page to an RGBA pixmap at a target size.
///
/// The target size is in pre-rotation coordinates (i.e. matching the page's
/// native width/height orientation). Rotation is applied after compositing.
pub fn render_to_size(page: &Page, w: u32, h: u32) -> Result<Pixmap, Error> {
    let page_w = page.info.width as u32;
    let page_h = page.info.height as u32;

    let has_palette = page.has_palette();

    let output = if has_palette {
        render_with_palette(page, w, h, page_w, page_h)?
    } else {
        let mask = page.decode_mask()?;
        let bg = page.decode_background()?;
        let fg = page.decode_foreground()?;

        match (&mask, &bg, &fg) {
            (None, Some(bg), _) => composite_bg_only(w, h, bg, page_w, page_h),
            (Some(mask), None, None) => composite_bilevel(w, h, mask, page_w, page_h),
            (Some(mask), Some(bg), Some(fg)) => composite_3layer(w, h, mask, bg, fg, page_w, page_h),
            (Some(mask), Some(bg), None) => composite_mask_bg(w, h, mask, bg, page_w, page_h),
            (Some(mask), None, Some(fg)) => composite_mask_fg(w, h, mask, fg, page_w, page_h),
            (None, None, _) => Pixmap::white(w, h),
        }
    };

    let output = apply_rotation(output, page.info.rotation);
    Ok(output)
}

// ============================================================
// BG-only: upscale background to page dimensions
// ============================================================

fn composite_bg_only(w: u32, h: u32, bg: &Pixmap, page_w: u32, page_h: u32) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = map_to_page(x, y, w, h, page_w, page_h);
            let (r, g, b) = sample_bilinear_virtual(bg, px, py, page_w, page_h);
            out.set_rgb(x, y, r, g, b);
        }
    }
    out
}

// ============================================================
// Mask-only: black where mask=1, white where mask=0
// ============================================================

fn composite_bilevel(w: u32, h: u32, mask: &Bitmap, page_w: u32, page_h: u32) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    for y in 0..h {
        for x in 0..w {
            if sample_mask(mask, x, y, w, h, page_w, page_h) {
                out.set_rgb(x, y, 0, 0, 0);
            }
        }
    }
    out
}

// ============================================================
// Mask + BG: background where mask=0, black text where mask=1
// ============================================================

fn composite_mask_bg(w: u32, h: u32, mask: &Bitmap, bg: &Pixmap, page_w: u32, page_h: u32) -> Pixmap {
    let mut out = composite_bg_only(w, h, bg, page_w, page_h);
    for y in 0..h {
        for x in 0..w {
            if sample_mask(mask, x, y, w, h, page_w, page_h) {
                out.set_rgb(x, y, 0, 0, 0);
            }
        }
    }
    out
}

// ============================================================
// Mask + FG (no BG): foreground color where mask=1, white elsewhere
// ============================================================

fn composite_mask_fg(w: u32, h: u32, mask: &Bitmap, fg: &Pixmap, page_w: u32, page_h: u32) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = map_to_page(x, y, w, h, page_w, page_h);
            if px < mask.width && py < mask.height && mask.get(px, py) {
                let (r, g, b) = sample_nearest_virtual(fg, px, py, page_w, page_h);
                out.set_rgb(x, y, r, g, b);
            }
        }
    }
    out
}

// ============================================================
// 3-layer: mask selects between FG and BG
// ============================================================

fn composite_3layer(w: u32, h: u32, mask: &Bitmap, bg: &Pixmap, fg: &Pixmap, page_w: u32, page_h: u32) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = map_to_page(x, y, w, h, page_w, page_h);
            if px < mask.width && py < mask.height && mask.get(px, py) {
                let (r, g, b) = sample_nearest_virtual(fg, px, py, page_w, page_h);
                out.set_rgb(x, y, r, g, b);
            } else {
                let (r, g, b) = sample_bilinear_virtual(bg, px, py, page_w, page_h);
                out.set_rgb(x, y, r, g, b);
            }
        }
    }
    out
}

// ============================================================
// Palette composite: mask + BG + FGbz palette colors
// ============================================================

fn render_with_palette(page: &Page, w: u32, h: u32, page_w: u32, page_h: u32) -> Result<Pixmap, Error> {
    let mask_indexed = page.decode_mask_indexed()?;
    let bg = page.decode_background()?;
    let palette = page.decode_palette()?;

    match (mask_indexed, bg, palette) {
        (Some((mask, blit_map)), Some(bg), Some(pal)) => {
            Ok(composite_palette(w, h, &mask, &blit_map, &bg, &pal, page_w, page_h))
        }
        (Some((mask, blit_map)), None, Some(pal)) => {
            Ok(composite_palette_no_bg(w, h, &mask, &blit_map, &pal, page_w, page_h))
        }
        (None, Some(bg), _) => Ok(composite_bg_only(w, h, &bg, page_w, page_h)),
        _ => Ok(Pixmap::white(w, h)),
    }
}

fn palette_color(pal: &Palette, blit_idx: i32) -> (u8, u8, u8) {
    if blit_idx < 0 {
        return (0, 0, 0);
    }
    let bi = blit_idx as usize;
    if bi < pal.indices.len() {
        let ci = pal.indices[bi] as usize;
        if ci < pal.colors.len() {
            return pal.colors[ci];
        }
    }
    // Invalid index mapping: render black to avoid silently painting wrong color.
    (0, 0, 0)
}

fn composite_palette(w: u32, h: u32, mask: &Bitmap, blit_map: &[i32], bg: &Pixmap, pal: &Palette, page_w: u32, page_h: u32) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mx, my) = map_to_page(x, y, w, h, page_w, page_h);
            let is_fg = mx < mask.width && my < mask.height && mask.get(mx, my);
            if is_fg {
                let mi = my as usize * mask.width as usize + mx as usize;
                let (r, g, b) = if mi < blit_map.len() {
                    palette_color(pal, blit_map[mi])
                } else {
                    (0, 0, 0)
                };
                out.set_rgb(x, y, r, g, b);
            } else {
                let (r, g, b) = sample_bilinear_virtual(bg, mx, my, page_w, page_h);
                out.set_rgb(x, y, r, g, b);
            }
        }
    }
    out
}

fn composite_palette_no_bg(w: u32, h: u32, mask: &Bitmap, blit_map: &[i32], pal: &Palette, page_w: u32, page_h: u32) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mx, my) = map_to_page(x, y, w, h, page_w, page_h);
            if mx < mask.width && my < mask.height && mask.get(mx, my) {
                let mi = my as usize * mask.width as usize + mx as usize;
                let (r, g, b) = if mi < blit_map.len() {
                    palette_color(pal, blit_map[mi])
                } else {
                    (0, 0, 0)
                };
                out.set_rgb(x, y, r, g, b);
            }
        }
    }
    out
}

// ============================================================
// Generalized sampling: maps output coords → source coords
// ============================================================

/// Map output pixel (x,y) at output size (ow,oh) to page coordinates (page_w, page_h).
fn map_to_page(x: u32, y: u32, ow: u32, oh: u32, page_w: u32, page_h: u32) -> (u32, u32) {
    let px = if ow == page_w { x } else {
        ((x as f64 + 0.5) * page_w as f64 / ow as f64) as u32
    };
    let py = if oh == page_h { y } else {
        ((y as f64 + 0.5) * page_h as f64 / oh as f64) as u32
    };
    (px.min(page_w - 1), py.min(page_h - 1))
}

/// Sample mask at output coordinates, mapping through page coordinates.
fn sample_mask(mask: &Bitmap, x: u32, y: u32, ow: u32, oh: u32, page_w: u32, page_h: u32) -> bool {
    let (mx, my) = map_to_page(x, y, ow, oh, page_w, page_h);
    mx < mask.width && my < mask.height && mask.get(mx, my)
}

fn layer_virtual_geometry(src: &Pixmap, page_w: u32, page_h: u32) -> (u32, u32, u32, u32, u32) {
    let red_w = (page_w + src.width - 1) / src.width;
    let red_h = (page_h + src.height - 1) / src.height;
    let reduction = red_w.max(red_h).max(1);
    let virt_w = (page_w / reduction).max(1);
    let virt_h = (page_h / reduction).max(1);
    let virt_page_w = virt_w * reduction;
    let virt_page_h = virt_h * reduction;
    (reduction, virt_w, virt_h, virt_page_w, virt_page_h)
}

fn sample_nearest_virtual(src: &Pixmap, page_x: u32, page_y: u32, page_w: u32, page_h: u32) -> (u8, u8, u8) {
    let (reduction, _, _, virt_page_w, virt_page_h) = layer_virtual_geometry(src, page_w, page_h);
    let px = page_x.min(virt_page_w - 1);
    // IW44 rows are flipped during decode. When the raw layer height contains
    // one padding row beyond the logical reduced height, the padding ends up on
    // the top edge of the page and must be trimmed there rather than at the
    // bottom. Horizontal padding is already dropped on the right by virt_page_w.
    let y_shift = src.height.saturating_mul(reduction).saturating_sub(page_h);
    let py = page_y.saturating_add(y_shift).min(virt_page_h - 1);
    let sx = (px / reduction).min(src.width - 1);
    let sy = (py / reduction).min(src.height - 1);
    src.get_rgb(sx, sy)
}

fn sample_bilinear_virtual(src: &Pixmap, page_x: u32, page_y: u32, page_w: u32, page_h: u32) -> (u8, u8, u8) {
    let (_, virt_w, virt_h, virt_page_w, virt_page_h) = layer_virtual_geometry(src, page_w, page_h);
    let sw = virt_w as f64;
    let sh = virt_h as f64;
    let dw = virt_page_w as f64;
    let dh = virt_page_h as f64;
    let px = page_x.min(virt_page_w - 1);
    let py = page_y.min(virt_page_h - 1);
    let sx = ((px as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
    let sy = ((py as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);

    let sx0 = sx as u32;
    let sy0 = sy as u32;
    let sx1 = (sx0 + 1).min(virt_w.saturating_sub(1)).min(src.width - 1);
    let sy1 = (sy0 + 1).min(virt_h.saturating_sub(1)).min(src.height - 1);
    let fx = ((sx - sx0 as f64) * 256.0 + 0.5).floor().clamp(0.0, 255.0) as u32;
    let fy = ((sy - sy0 as f64) * 256.0 + 0.5).floor().clamp(0.0, 255.0) as u32;
    let (r00, g00, b00) = src.get_rgb(sx0.min(src.width - 1), sy0.min(src.height - 1));
    let (r10, g10, b10) = src.get_rgb(sx1, sy0.min(src.height - 1));
    let (r01, g01, b01) = src.get_rgb(sx0.min(src.width - 1), sy1);
    let (r11, g11, b11) = src.get_rgb(sx1, sy1);
    let interp_h = |v0: u8, v1: u8| -> u32 {
        ((v0 as u32 * (256 - fx) + v1 as u32 * fx + 128) >> 8).clamp(0, 255)
    };
    let interp_v = |v0: u32, v1: u32| -> u8 {
        ((v0 * (256 - fy) + v1 * fy + 128) >> 8).clamp(0, 255) as u8
    };
    (
        interp_v(interp_h(r00, r10), interp_h(r01, r11)),
        interp_v(interp_h(g00, g10), interp_h(g01, g11)),
        interp_v(interp_h(b00, b10), interp_h(b01, b11)),
    )
}

#[cfg(test)]
/// Bilinear sample from src layer, mapping output coords (x,y) at size (ow,oh) to src coords.
fn sample_bilinear(src: &Pixmap, x: u32, y: u32, ow: u32, oh: u32) -> (u8, u8, u8) {
    let sw = src.width as f64;
    let sh = src.height as f64;
    let sx = ((x as f64 + 0.5) * sw / ow as f64 - 0.5).clamp(0.0, sw - 1.0);
    let sy = ((y as f64 + 0.5) * sh / oh as f64 - 0.5).clamp(0.0, sh - 1.0);

    let sx0 = sx as u32;
    let sy0 = sy as u32;
    let sx1 = (sx0 + 1).min(src.width - 1);
    let sy1 = (sy0 + 1).min(src.height - 1);
    let fx = sx - sx0 as f64;
    let fy = sy - sy0 as f64;
    let (r00, g00, b00) = src.get_rgb(sx0, sy0);
    let (r10, g10, b10) = src.get_rgb(sx1, sy0);
    let (r01, g01, b01) = src.get_rgb(sx0, sy1);
    let (r11, g11, b11) = src.get_rgb(sx1, sy1);
    let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
        let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
            + v10 as f64 * fx * (1.0 - fy)
            + v01 as f64 * (1.0 - fx) * fy
            + v11 as f64 * fx * fy;
        (v + 0.5).clamp(0.0, 255.0) as u8
    };
    (
        interp(r00, r10, r01, r11),
        interp(g00, g10, g01, g11),
        interp(b00, b10, b01, b11),
    )
}

// ============================================================
// Rotation
// ============================================================

fn apply_rotation(src: Pixmap, rotation: Rotation) -> Pixmap {
    match rotation {
        Rotation::None => src,
        Rotation::Cw90 => rotate_cw90(&src),
        Rotation::Cw180 => rotate_180(&src),
        Rotation::Cw270 => rotate_cw270(&src),
    }
}

fn rotate_cw90(src: &Pixmap) -> Pixmap {
    let w = src.height;
    let h = src.width;
    let mut out = Pixmap::white(w, h);
    for y in 0..src.height {
        for x in 0..src.width {
            let (r, g, b) = src.get_rgb(x, y);
            let nx = src.height - 1 - y;
            let ny = x;
            out.set_rgb(nx, ny, r, g, b);
        }
    }
    out
}

fn rotate_180(src: &Pixmap) -> Pixmap {
    let mut out = Pixmap::white(src.width, src.height);
    for y in 0..src.height {
        for x in 0..src.width {
            let (r, g, b) = src.get_rgb(x, y);
            out.set_rgb(src.width - 1 - x, src.height - 1 - y, r, g, b);
        }
    }
    out
}

fn rotate_cw270(src: &Pixmap) -> Pixmap {
    let w = src.height;
    let h = src.width;
    let mut out = Pixmap::white(w, h);
    for y in 0..src.height {
        for x in 0..src.width {
            let (r, g, b) = src.get_rgb(x, y);
            let nx = y;
            let ny = src.width - 1 - x;
            out.set_rgb(nx, ny, r, g, b);
        }
    }
    out
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rdjvu_document::Document;

    #[derive(Clone, Copy)]
    struct DiffStats {
        pixel_count: usize,
        pixel_mismatches: usize,
        byte_mismatches: usize,
        sum_abs_r: u64,
        sum_abs_g: u64,
        sum_abs_b: u64,
        max_abs_r: u8,
        max_abs_g: u8,
        max_abs_b: u8,
    }

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

    fn render_page(file: &str, page_idx: usize) -> Pixmap {
        let data = std::fs::read(assets_path().join(file)).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(page_idx).unwrap();
        render(&page).unwrap()
    }

    fn ppm_header_end(ppm: &[u8]) -> usize {
        let p1 = ppm.iter().position(|&b| b == b'\n').unwrap() + 1;
        let p2 = p1 + ppm[p1..].iter().position(|&b| b == b'\n').unwrap() + 1;
        p2 + ppm[p2..].iter().position(|&b| b == b'\n').unwrap() + 1
    }

    fn diff_stats(actual_ppm: &[u8], expected_ppm: &[u8]) -> DiffStats {
        let a = &actual_ppm[ppm_header_end(actual_ppm)..];
        let e = &expected_ppm[ppm_header_end(expected_ppm)..];
        let px = a.len().min(e.len()) / 3;
        let mut stats = DiffStats {
            pixel_count: px,
            pixel_mismatches: 0,
            byte_mismatches: 0,
            sum_abs_r: 0,
            sum_abs_g: 0,
            sum_abs_b: 0,
            max_abs_r: 0,
            max_abs_g: 0,
            max_abs_b: 0,
        };
        for p in 0..px {
            let i = p * 3;
            let dr = (a[i] as i16 - e[i] as i16).unsigned_abs() as u8;
            let dg = (a[i + 1] as i16 - e[i + 1] as i16).unsigned_abs() as u8;
            let db = (a[i + 2] as i16 - e[i + 2] as i16).unsigned_abs() as u8;
            if dr != 0 || dg != 0 || db != 0 {
                stats.pixel_mismatches += 1;
            }
            stats.byte_mismatches += (dr != 0) as usize + (dg != 0) as usize + (db != 0) as usize;
            stats.sum_abs_r += dr as u64;
            stats.sum_abs_g += dg as u64;
            stats.sum_abs_b += db as u64;
            stats.max_abs_r = stats.max_abs_r.max(dr);
            stats.max_abs_g = stats.max_abs_g.max(dg);
            stats.max_abs_b = stats.max_abs_b.max(db);
        }
        stats
    }

    fn assert_ppm_match(pixmap: &Pixmap, golden_file: &str) {
        let actual = pixmap.to_ppm();
        let expected = std::fs::read(golden_path().join(golden_file)).unwrap();
        assert_eq!(actual.len(), expected.len(), "{}: size mismatch {} vs {}", golden_file, actual.len(), expected.len());

        let stats = diff_stats(&actual, &expected);
        let total = stats.pixel_count * 3;
        if stats.byte_mismatches > 0 {
            panic!(
                "{}: {} pixel-bytes differ ({}/{} = {:.1}%)",
                golden_file, stats.byte_mismatches,
                stats.byte_mismatches, total,
                stats.byte_mismatches as f64 / total as f64 * 100.0
            );
        }
    }


    #[test]
    fn render_chicken_bg_only() {
        let pm = render_page("chicken.djvu", 0);
        assert_ppm_match(&pm, "chicken.ppm");
    }

    #[test]
    fn render_boy_jb2_mask_only() {
        let pm = render_page("boy_jb2.djvu", 0);
        assert_ppm_match(&pm, "boy_jb2.ppm");
    }

    #[test]
    fn render_carte_3layer() {
        let pm = render_page("carte.djvu", 0);
        assert_ppm_match(&pm, "carte_p1.ppm");
    }

    #[test]
    fn render_navm_fgbz_p1() {
        let pm = render_page("navm_fgbz.djvu", 0);
        assert_ppm_match(&pm, "navm_fgbz_p1.ppm");
    }

    #[test]
    fn render_navm_fgbz_p4_palette() {
        let pm = render_page("navm_fgbz.djvu", 3);
        assert_ppm_match(&pm, "navm_fgbz_p4.ppm");
    }

    #[test]
    fn render_colorbook_p1() {
        let pm = render_page("colorbook.djvu", 0);
        assert_ppm_match(&pm, "colorbook_p1.ppm");
    }

    #[test]
    fn render_djvu3spec_p5() {
        let pm = render_page("DjVu3Spec_bundled.djvu", 4);
        assert_ppm_match(&pm, "djvu3spec_p5.ppm");
    }

    #[test]
    fn render_boy_jb2_rot90() {
        let pm = render_page("boy_jb2_rotate90.djvu", 0);
        assert_eq!(pm.width, 256, "rotated width");
        assert_eq!(pm.height, 192, "rotated height");
        assert_ppm_match(&pm, "boy_jb2_rot90.ppm");
    }

    #[test]
    fn debug_colorbook_navm_layer_mismatch() {
        let compare = |actual: &Pixmap, ref_path: &str, tag: &str| {
            let rp = std::path::Path::new(ref_path);
            if !rp.exists() {
                return;
            }
            let expected = std::fs::read(rp).unwrap();
            let actual = actual.to_ppm();
            let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let a = &actual[header_end..];
            let e = &expected[header_end..];
            let px = (a.len().min(e.len())) / 3;
            let mut diff_px = 0usize;
            for p in 0..px {
                let i = p * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    diff_px += 1;
                }
            }
            eprintln!("{} mismatch_px={}", tag, diff_px);
        };

        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let mask = page.decode_mask().unwrap().unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let fg = page.decode_foreground().unwrap().unwrap();
        let comp = render(&page).unwrap();
        compare(
            &composite_bg_only(page.info.width as u32, page.info.height as u32, &bg, page.info.width as u32, page.info.height as u32),
            "/tmp/rdjvu_debug/colorbook_p1_bg.ppm",
            "colorbook bg",
        );
        compare(&composite_mask_fg(page.info.width as u32, page.info.height as u32, &mask, &fg, page.info.width as u32, page.info.height as u32), "/tmp/rdjvu_debug/colorbook_p1_fg.ppm", "colorbook fg");
        compare(&comp, "/tmp/rdjvu_debug/colorbook_p1_bg.ppm", "colorbook full-vs-bg");

        let data = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(3).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let comp = render(&page).unwrap();
        compare(
            &composite_bg_only(page.info.width as u32, page.info.height as u32, &bg, page.info.width as u32, page.info.height as u32),
            "/tmp/rdjvu_debug/navm_p4_bg.ppm",
            "navm p4 bg",
        );
        compare(&comp, "/tmp/rdjvu_debug/navm_p4_bg.ppm", "navm p4 full-vs-bg");
    }

    #[test]
    fn debug_carte_layer_mismatch() {
        let compare = |actual: &Pixmap, ref_path: &str, tag: &str| {
            let rp = std::path::Path::new(ref_path);
            if !rp.exists() {
                return;
            }
            let expected = std::fs::read(rp).unwrap();
            let actual = actual.to_ppm();
            let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let a = &actual[header_end..];
            let e = &expected[header_end..];
            let px = (a.len().min(e.len())) / 3;
            let mut diff_px = 0usize;
            for p in 0..px {
                let i = p * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    diff_px += 1;
                }
            }
            eprintln!("{} mismatch_px={}", tag, diff_px);
        };

        let data = std::fs::read(assets_path().join("carte.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let mask = page.decode_mask().unwrap().unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let fg = page.decode_foreground().unwrap().unwrap();
        let comp = render(&page).unwrap();

        compare(
            &composite_bg_only(
                page.info.width as u32,
                page.info.height as u32,
                &bg,
                page.info.width as u32,
                page.info.height as u32,
            ),
            "/tmp/rdjvu_debug/carte_bg.ppm",
            "carte bg",
        );
        compare(
            &composite_mask_fg(page.info.width as u32, page.info.height as u32, &mask, &fg, page.info.width as u32, page.info.height as u32),
            "/tmp/rdjvu_debug/carte_fg.ppm",
            "carte fg",
        );
        compare(&comp, "/tmp/rdjvu_debug/carte_bg.ppm", "carte full-vs-bg");
    }

    #[test]
    fn debug_navm_bg_scaler_candidates() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/navm_p4_bg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(3).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;

        let compare = |name: &str, sample: &dyn Fn(u32, u32) -> (u8, u8, u8)| {
            let mut out = Pixmap::white(w, h);
            for y in 0..h {
                for x in 0..w {
                    let (r, g, b) = sample(x, y);
                    out.set_rgb(x, y, r, g, b);
                }
            }
            let actual = out.to_ppm();
            let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let a = &actual[header_end..];
            let e = &expected[header_end..];
            let px = (a.len().min(e.len())) / 3;
            let mut diff_px = 0usize;
            for p in 0..px {
                let i = p * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    diff_px += 1;
                }
            }
            eprintln!("navm bg scaler {} mismatch_px={}", name, diff_px);
        };

        let scale = (w as f64 / bg.width as f64).round().max(1.0) as u32;
        compare("nearest_round_scale", &|x, y| {
            let sx = (x / scale).min(bg.width - 1);
            let sy = (y / scale).min(bg.height - 1);
            bg.get_rgb(sx, sy)
        });
        compare("bilinear_round_scale", &|x, y| sample_bilinear(&bg, x, y, w, h));

        let bilinear_map = |x: u32, y: u32, mode: &str, round_mode: &str| -> (u8, u8, u8) {
            let sw = bg.width as f64;
            let sh = bg.height as f64;
            let dw = w as f64;
            let dh = h as f64;
            let (sx, sy) = match mode {
                // center-of-pixel mapping
                "center" => (
                    ((x as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0),
                    ((y as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0),
                ),
                // corner mapping
                "corner" => (
                    (x as f64 * sw / dw).clamp(0.0, sw - 1.0),
                    (y as f64 * sh / dh).clamp(0.0, sh - 1.0),
                ),
                // align first/last sample to first/last source pixel
                "edge" => (
                    if w > 1 { (x as f64 * (sw - 1.0) / (dw - 1.0)).clamp(0.0, sw - 1.0) } else { 0.0 },
                    if h > 1 { (y as f64 * (sh - 1.0) / (dh - 1.0)).clamp(0.0, sh - 1.0) } else { 0.0 },
                ),
                _ => unreachable!(),
            };
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(bg.height - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = bg.get_rgb(sx0, sy0);
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0);
            let (r01, g01, b01) = bg.get_rgb(sx0, sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                    + v10 as f64 * fx * (1.0 - fy)
                    + v01 as f64 * (1.0 - fx) * fy
                    + v11 as f64 * fx * fy;
                match round_mode {
                    "nearest" => (v + 0.5).clamp(0.0, 255.0) as u8,
                    "floor" => v.floor().clamp(0.0, 255.0) as u8,
                    _ => unreachable!(),
                }
            };
            (interp(r00, r10, r01, r11), interp(g00, g10, g01, g11), interp(b00, b10, b01, b11))
        };

        for mode in ["center", "corner", "edge"] {
            for round_mode in ["nearest", "floor"] {
                let name = format!("bilinear_{}_{}", mode, round_mode);
                compare(&name, &|x, y| bilinear_map(x, y, mode, round_mode));
            }
        }

        compare("nearest_true_dims", &|x, y| {
            let sx = ((x as f64) * bg.width as f64 / w as f64).floor() as u32;
            let sy = ((y as f64) * bg.height as f64 / h as f64).floor() as u32;
            bg.get_rgb(sx.min(bg.width - 1), sy.min(bg.height - 1))
        });

        compare("bilinear_center_fixed16", &|x, y| {
            let sw = bg.width as i64;
            let sh = bg.height as i64;
            let dw = w as i64;
            let dh = h as i64;
            let sx_fp = (((2 * x as i64 + 1) * sw << 16) / (2 * dw)) - (1 << 15);
            let sy_fp = (((2 * y as i64 + 1) * sh << 16) / (2 * dh)) - (1 << 15);
            let sx_fp = sx_fp.clamp(0, (sw - 1) << 16);
            let sy_fp = sy_fp.clamp(0, (sh - 1) << 16);
            let sx0 = (sx_fp >> 16) as u32;
            let sy0 = (sy_fp >> 16) as u32;
            let sx1 = (sx0 + 1).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(bg.height - 1);
            let fx = (sx_fp & 0xffff) as i64;
            let fy = (sy_fp & 0xffff) as i64;
            let wx0 = 65536 - fx;
            let wy0 = 65536 - fy;
            let wx1 = fx;
            let wy1 = fy;
            let (r00, g00, b00) = bg.get_rgb(sx0, sy0);
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0);
            let (r01, g01, b01) = bg.get_rgb(sx0, sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let acc = v00 as i64 * wx0 * wy0
                    + v10 as i64 * wx1 * wy0
                    + v01 as i64 * wx0 * wy1
                    + v11 as i64 * wx1 * wy1;
                ((acc + (1 << 31)) >> 32).clamp(0, 255) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });

        compare("bilinear_center_gamma22", &|x, y| {
            let sw = bg.width as f64;
            let sh = bg.height as f64;
            let dw = w as f64;
            let dh = h as f64;
            let sx = ((x as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((y as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(bg.height - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = bg.get_rgb(sx0, sy0);
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0);
            let (r01, g01, b01) = bg.get_rgb(sx0, sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let gamma = 2.2f64;
            let to_lin = |v: u8| (v as f64 / 255.0).powf(gamma);
            let to_srgb = |v: f64| (v.clamp(0.0, 1.0).powf(1.0 / gamma) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = to_lin(v00) * (1.0 - fx) * (1.0 - fy)
                    + to_lin(v10) * fx * (1.0 - fy)
                    + to_lin(v01) * (1.0 - fx) * fy
                    + to_lin(v11) * fx * fy;
                to_srgb(v)
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });
    }

    #[test]
    fn debug_carte_bg_scaler_candidates() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/carte_bg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("carte.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;

        let compare = |name: &str, sample: &dyn Fn(u32, u32) -> (u8, u8, u8)| {
            let mut out = Pixmap::white(w, h);
            for y in 0..h {
                for x in 0..w {
                    let (r, g, b) = sample(x, y);
                    out.set_rgb(x, y, r, g, b);
                }
            }
            let actual = out.to_ppm();
            let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let a = &actual[header_end..];
            let e = &expected[header_end..];
            let px = (a.len().min(e.len())) / 3;
            let mut diff_px = 0usize;
            for p in 0..px {
                let i = p * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    diff_px += 1;
                }
            }
            eprintln!("carte bg scaler {} mismatch_px={}", name, diff_px);
        };

        let scale = (w as f64 / bg.width as f64).round().max(1.0) as u32;
        compare("nearest_round_scale", &|x, y| {
            let sx = (x / scale).min(bg.width - 1);
            let sy = (y / scale).min(bg.height - 1);
            bg.get_rgb(sx, sy)
        });
        compare("bilinear_round_scale", &|x, y| sample_bilinear(&bg, x, y, w, h));

        let bilinear_map = |x: u32, y: u32, mode: &str, round_mode: &str| -> (u8, u8, u8) {
            let sw = bg.width as f64;
            let sh = bg.height as f64;
            let dw = w as f64;
            let dh = h as f64;
            let (sx, sy) = match mode {
                "center" => (
                    ((x as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0),
                    ((y as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0),
                ),
                "corner" => (
                    (x as f64 * sw / dw).clamp(0.0, sw - 1.0),
                    (y as f64 * sh / dh).clamp(0.0, sh - 1.0),
                ),
                "edge" => (
                    if w > 1 { (x as f64 * (sw - 1.0) / (dw - 1.0)).clamp(0.0, sw - 1.0) } else { 0.0 },
                    if h > 1 { (y as f64 * (sh - 1.0) / (dh - 1.0)).clamp(0.0, sh - 1.0) } else { 0.0 },
                ),
                _ => unreachable!(),
            };
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(bg.height - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = bg.get_rgb(sx0, sy0);
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0);
            let (r01, g01, b01) = bg.get_rgb(sx0, sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                    + v10 as f64 * fx * (1.0 - fy)
                    + v01 as f64 * (1.0 - fx) * fy
                    + v11 as f64 * fx * fy;
                match round_mode {
                    "nearest" => (v + 0.5).clamp(0.0, 255.0) as u8,
                    "floor" => v.floor().clamp(0.0, 255.0) as u8,
                    _ => unreachable!(),
                }
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        };

        for mode in ["center", "corner", "edge"] {
            for round_mode in ["nearest", "floor"] {
                let name = format!("bilinear_{}_{}", mode, round_mode);
                compare(&name, &|x, y| bilinear_map(x, y, mode, round_mode));
            }
        }

        compare("nearest_true_dims", &|x, y| {
            let sx = ((x as f64) * bg.width as f64 / w as f64).floor() as u32;
            let sy = ((y as f64) * bg.height as f64 / h as f64).floor() as u32;
            bg.get_rgb(sx.min(bg.width - 1), sy.min(bg.height - 1))
        });

        compare("bilinear_center_fixed16", &|x, y| {
            let sw = bg.width as i64;
            let sh = bg.height as i64;
            let dw = w as i64;
            let dh = h as i64;
            let sx_fp = (((2 * x as i64 + 1) * sw << 16) / (2 * dw)) - (1 << 15);
            let sy_fp = (((2 * y as i64 + 1) * sh << 16) / (2 * dh)) - (1 << 15);
            let sx_fp = sx_fp.clamp(0, (sw - 1) << 16);
            let sy_fp = sy_fp.clamp(0, (sh - 1) << 16);
            let sx0 = (sx_fp >> 16) as u32;
            let sy0 = (sy_fp >> 16) as u32;
            let sx1 = (sx0 + 1).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(bg.height - 1);
            let fx = (sx_fp & 0xffff) as i64;
            let fy = (sy_fp & 0xffff) as i64;
            let wx0 = 65536 - fx;
            let wy0 = 65536 - fy;
            let wx1 = fx;
            let wy1 = fy;
            let (r00, g00, b00) = bg.get_rgb(sx0, sy0);
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0);
            let (r01, g01, b01) = bg.get_rgb(sx0, sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let acc = v00 as i64 * wx0 * wy0
                    + v10 as i64 * wx1 * wy0
                    + v01 as i64 * wx0 * wy1
                    + v11 as i64 * wx1 * wy1;
                ((acc + (1 << 31)) >> 32).clamp(0, 255) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });
    }

    #[test]
    fn debug_colorbook_bg_scaler_candidates() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/colorbook_p1_bg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;

        let compare = |name: &str, sample: &dyn Fn(u32, u32) -> (u8, u8, u8)| {
            let mut out = Pixmap::white(w, h);
            for y in 0..h {
                for x in 0..w {
                    let (r, g, b) = sample(x, y);
                    out.set_rgb(x, y, r, g, b);
                }
            }
            let actual = out.to_ppm();
            let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let a = &actual[header_end..];
            let e = &expected[header_end..];
            let px = (a.len().min(e.len())) / 3;
            let mut diff_px = 0usize;
            for p in 0..px {
                let i = p * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    diff_px += 1;
                }
            }
            eprintln!("colorbook bg scaler {} mismatch_px={}", name, diff_px);
        };

        let scale = (w as f64 / bg.width as f64).round().max(1.0) as u32;
        compare("nearest_round_scale", &|x, y| {
            let sx = (x / scale).min(bg.width - 1);
            let sy = (y / scale).min(bg.height - 1);
            bg.get_rgb(sx, sy)
        });
        compare("bilinear_round_scale", &|x, y| sample_bilinear(&bg, x, y, w, h));

        compare("nearest_true_dims", &|x, y| {
            let sx = ((x as u64 * bg.width as u64) / w as u64) as u32;
            let sy = ((y as u64 * bg.height as u64) / h as u64) as u32;
            bg.get_rgb(sx.min(bg.width - 1), sy.min(bg.height - 1))
        });

        compare("bilinear_true_dims", &|x, y| {
            let sw = bg.width as f64;
            let sh = bg.height as f64;
            let dw = w as f64;
            let dh = h as f64;
            let sx = ((x as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((y as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(bg.height - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = bg.get_rgb(sx0, sy0);
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0);
            let (r01, g01, b01) = bg.get_rgb(sx0, sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                    + v10 as f64 * fx * (1.0 - fy)
                    + v01 as f64 * (1.0 - fx) * fy
                    + v11 as f64 * fx * fy;
                (v + 0.5).clamp(0.0, 255.0) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });

        let scale = ((w + bg.width - 1) / bg.width).max((h + bg.height - 1) / bg.height).max(1);
        let eff_w = (w / scale).max(1);
        let eff_h = (h / scale).max(1);
        let virt_w = eff_w * scale;
        let virt_h = eff_h * scale;
        compare("nearest_virtual_floor3", &|x, y| {
            let sx = (x / scale).min(eff_w.saturating_sub(1));
            let sy = (y / scale).min(eff_h.saturating_sub(1));
            bg.get_rgb(sx.min(bg.width - 1), sy.min(bg.height - 1))
        });

        compare("bilinear_virtual_floor3", &|x, y| {
            let sw = eff_w as f64;
            let sh = eff_h as f64;
            let dw = virt_w as f64;
            let dh = virt_h as f64;
            let sx = ((x.min(virt_w - 1) as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((y.min(virt_h - 1) as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(eff_w.saturating_sub(1));
            let sy1 = (sy0 + 1).min(eff_h.saturating_sub(1));
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1.min(bg.height - 1));
            let (r11, g11, b11) = bg.get_rgb(sx1.min(bg.width - 1), sy1.min(bg.height - 1));
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                    + v10 as f64 * fx * (1.0 - fy)
                    + v01 as f64 * (1.0 - fx) * fy
                    + v11 as f64 * fx * fy;
                (v + 0.5).clamp(0.0, 255.0) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });

        compare("bilinear_logical_true_dims", &|x, y| {
            let sw = eff_w as f64;
            let sh = eff_h as f64;
            let dw = w as f64;
            let dh = h as f64;
            let sx = ((x as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((y as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(eff_w.saturating_sub(1)).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(eff_h.saturating_sub(1)).min(bg.height - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                    + v10 as f64 * fx * (1.0 - fy)
                    + v01 as f64 * (1.0 - fx) * fy
                    + v11 as f64 * fx * fy;
                (v + 0.5).clamp(0.0, 255.0) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });

        compare("bilinear_logical_true_dims_fixed16", &|x, y| {
            let sw = eff_w as i64;
            let sh = eff_h as i64;
            let dw = w as i64;
            let dh = h as i64;
            let sx_fp = (((2 * x as i64 + 1) * sw << 16) / (2 * dw)) - (1 << 15);
            let sy_fp = (((2 * y as i64 + 1) * sh << 16) / (2 * dh)) - (1 << 15);
            let sx_fp = sx_fp.clamp(0, (sw - 1) << 16);
            let sy_fp = sy_fp.clamp(0, (sh - 1) << 16);
            let sx0 = (sx_fp >> 16) as u32;
            let sy0 = (sy_fp >> 16) as u32;
            let sx1 = (sx0 + 1).min(eff_w.saturating_sub(1)).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(eff_h.saturating_sub(1)).min(bg.height - 1);
            let fx = (sx_fp & 0xffff) as i64;
            let fy = (sy_fp & 0xffff) as i64;
            let wx0 = 65536 - fx;
            let wy0 = 65536 - fy;
            let wx1 = fx;
            let wy1 = fy;
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let acc = v00 as i64 * wx0 * wy0
                    + v10 as i64 * wx1 * wy0
                    + v01 as i64 * wx0 * wy1
                    + v11 as i64 * wx1 * wy1;
                ((acc + (1 << 31)) >> 32).clamp(0, 255) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });

        let bilinear_virtual = |x: u32, y: u32, mode: &str, round_mode: &str| -> (u8, u8, u8) {
            let sw = eff_w as f64;
            let sh = eff_h as f64;
            let dw = virt_w as f64;
            let dh = virt_h as f64;
            let cx = x.min(virt_w - 1) as f64;
            let cy = y.min(virt_h - 1) as f64;
            let (sx, sy) = match mode {
                "center" => (
                    ((cx + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0),
                    ((cy + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0),
                ),
                "corner" => (
                    (cx * sw / dw).clamp(0.0, sw - 1.0),
                    (cy * sh / dh).clamp(0.0, sh - 1.0),
                ),
                "edge" => (
                    if virt_w > 1 { (cx * (sw - 1.0) / (dw - 1.0)).clamp(0.0, sw - 1.0) } else { 0.0 },
                    if virt_h > 1 { (cy * (sh - 1.0) / (dh - 1.0)).clamp(0.0, sh - 1.0) } else { 0.0 },
                ),
                _ => unreachable!(),
            };
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(eff_w.saturating_sub(1)).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(eff_h.saturating_sub(1)).min(bg.height - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                    + v10 as f64 * fx * (1.0 - fy)
                    + v01 as f64 * (1.0 - fx) * fy
                    + v11 as f64 * fx * fy;
                match round_mode {
                    "nearest" => (v + 0.5).clamp(0.0, 255.0) as u8,
                    "floor" => v.floor().clamp(0.0, 255.0) as u8,
                    _ => unreachable!(),
                }
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        };

        for mode in ["center", "corner", "edge"] {
            for round_mode in ["nearest", "floor"] {
                let name = format!("bilinear_virtual_{}_{}", mode, round_mode);
                compare(&name, &|x, y| bilinear_virtual(x, y, mode, round_mode));
            }
        }

        compare("bilinear_virtual_center_fixed16", &|x, y| {
            let sw = eff_w as i64;
            let sh = eff_h as i64;
            let dw = virt_w as i64;
            let dh = virt_h as i64;
            let px = x.min(virt_w - 1) as i64;
            let py = y.min(virt_h - 1) as i64;
            let sx_fp = (((2 * px + 1) * sw << 16) / (2 * dw)) - (1 << 15);
            let sy_fp = (((2 * py + 1) * sh << 16) / (2 * dh)) - (1 << 15);
            let sx_fp = sx_fp.clamp(0, (sw - 1) << 16);
            let sy_fp = sy_fp.clamp(0, (sh - 1) << 16);
            let sx0 = (sx_fp >> 16) as u32;
            let sy0 = (sy_fp >> 16) as u32;
            let sx1 = (sx0 + 1).min(eff_w.saturating_sub(1)).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(eff_h.saturating_sub(1)).min(bg.height - 1);
            let fx = (sx_fp & 0xffff) as i64;
            let fy = (sy_fp & 0xffff) as i64;
            let wx0 = 65536 - fx;
            let wy0 = 65536 - fy;
            let wx1 = fx;
            let wy1 = fy;
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let acc = v00 as i64 * wx0 * wy0
                    + v10 as i64 * wx1 * wy0
                    + v01 as i64 * wx0 * wy1
                    + v11 as i64 * wx1 * wy1;
                ((acc + (1 << 31)) >> 32).clamp(0, 255) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });
    }

    #[test]
    fn debug_colorbook_fg_scaler_candidates() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/colorbook_p1_fg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let mask = page.decode_mask().unwrap().unwrap();
        let fg = page.decode_foreground().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;

        let compare = |name: &str, sample: &dyn Fn(u32, u32) -> (u8, u8, u8)| {
            let mut out = Pixmap::white(w, h);
            let mw = mask.width.min(w);
            let mh = mask.height.min(h);
            for y in 0..mh {
                for x in 0..mw {
                    if mask.get(x, y) {
                        let (r, g, b) = sample(x, y);
                        out.set_rgb(x, y, r, g, b);
                    }
                }
            }
            let actual = out.to_ppm();
            let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
            let a = &actual[header_end..];
            let e = &expected[header_end..];
            let px = (a.len().min(e.len())) / 3;
            let mut diff_px = 0usize;
            for p in 0..px {
                let i = p * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    diff_px += 1;
                }
            }
            eprintln!("colorbook fg scaler {} mismatch_px={}", name, diff_px);
        };

        let scale = (w as f64 / fg.width as f64).round().max(1.0) as u32;
        compare("nearest_round_scale", &|x, y| {
            let sx = (x / scale).min(fg.width - 1);
            let sy = (y / scale).min(fg.height - 1);
            fg.get_rgb(sx, sy)
        });
        compare("bilinear_round_scale", &|x, y| sample_bilinear(&fg, x, y, w, h));

        compare("nearest_true_dims", &|x, y| {
            let sx = ((x as u64 * fg.width as u64) / w as u64) as u32;
            let sy = ((y as u64 * fg.height as u64) / h as u64) as u32;
            fg.get_rgb(sx.min(fg.width - 1), sy.min(fg.height - 1))
        });

        let (reduction, virt_w, virt_h, virt_page_w, virt_page_h) = layer_virtual_geometry(&fg, w, h);
        compare("nearest_virtual_floor3", &|x, y| {
            let px = x.min(virt_page_w - 1);
            let py = y.min(virt_page_h - 1);
            let sx = (px / reduction).min(virt_w - 1).min(fg.width - 1);
            let sy = (py / reduction).min(virt_h - 1).min(fg.height - 1);
            fg.get_rgb(sx, sy)
        });

        compare("nearest_virtual_true_dims", &|x, y| {
            let sx = ((x as u64 * virt_w as u64) / w as u64) as u32;
            let sy = ((y as u64 * virt_h as u64) / h as u64) as u32;
            fg.get_rgb(sx.min(virt_w - 1).min(fg.width - 1), sy.min(virt_h - 1).min(fg.height - 1))
        });

        compare("nearest_virtual_true_dims_center", &|x, y| {
            let sx = (((2 * x as u64 + 1) * virt_w as u64) / (2 * w as u64)) as u32;
            let sy = (((2 * y as u64 + 1) * virt_h as u64) / (2 * h as u64)) as u32;
            fg.get_rgb(sx.min(virt_w - 1).min(fg.width - 1), sy.min(virt_h - 1).min(fg.height - 1))
        });

        for x_shift in 0..reduction.min(3) {
            for y_shift in 0..reduction.min(3) {
                let name = format!("nearest_virtual_shift_{}_{}", x_shift, y_shift);
                compare(&name, &|x, y| {
                    let px = (x + x_shift).min(virt_page_w - 1);
                    let py = (y + y_shift).min(virt_page_h - 1);
                    let sx = (px / reduction).min(virt_w - 1).min(fg.width - 1);
                    let sy = (py / reduction).min(virt_h - 1).min(fg.height - 1);
                    fg.get_rgb(sx, sy)
                });
            }
        }

        compare("nearest_virtual_round", &|x, y| {
            let px = (x + reduction / 2).min(virt_page_w - 1);
            let py = (y + reduction / 2).min(virt_page_h - 1);
            let sx = (px / reduction).min(virt_w - 1).min(fg.width - 1);
            let sy = (py / reduction).min(virt_h - 1).min(fg.height - 1);
            fg.get_rgb(sx, sy)
        });

        compare("bilinear_true_dims", &|x, y| {
            let sw = fg.width as f64;
            let sh = fg.height as f64;
            let dw = w as f64;
            let dh = h as f64;
            let sx = ((x as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((y as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(fg.width - 1);
            let sy1 = (sy0 + 1).min(fg.height - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let (r00, g00, b00) = fg.get_rgb(sx0, sy0);
            let (r10, g10, b10) = fg.get_rgb(sx1, sy0);
            let (r01, g01, b01) = fg.get_rgb(sx0, sy1);
            let (r11, g11, b11) = fg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                    + v10 as f64 * fx * (1.0 - fy)
                    + v01 as f64 * (1.0 - fx) * fy
                    + v11 as f64 * fx * fy;
                (v + 0.5).clamp(0.0, 255.0) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });
    }

    #[test]
    fn debug_carte_fg_shift_candidates() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/carte_fg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("carte.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let mask = page.decode_mask().unwrap().unwrap();
        let fg = page.decode_foreground().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;
        let (reduction, virt_w, virt_h, virt_page_w, virt_page_h) = layer_virtual_geometry(&fg, w, h);

        let compare = |name: &str, x_shift: u32, y_shift: u32| {
            let mut out = Pixmap::white(w, h);
            let mw = mask.width.min(w);
            let mh = mask.height.min(h);
            for y in 0..mh {
                for x in 0..mw {
                    if mask.get(x, y) {
                        let px = (x + x_shift).min(virt_page_w - 1);
                        let py = (y + y_shift).min(virt_page_h - 1);
                        let sx = (px / reduction).min(virt_w - 1).min(fg.width - 1);
                        let sy = (py / reduction).min(virt_h - 1).min(fg.height - 1);
                        let (r, g, b) = fg.get_rgb(sx, sy);
                        out.set_rgb(x, y, r, g, b);
                    }
                }
            }
            let stats = diff_stats(&out.to_ppm(), &expected);
            eprintln!(
                "carte fg shift {} byte_mismatch={} pixel_mismatch={}",
                name,
                stats.byte_mismatches,
                stats.pixel_mismatches,
            );
        };

        for x_shift in 0..reduction.min(3) {
            for y_shift in 0..reduction.min(3) {
                compare(&format!("{}_{}", x_shift, y_shift), x_shift, y_shift);
            }
        }
    }

    #[test]
    fn debug_colorbook_bg_mismatch_profile() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/colorbook_p1_bg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;
        let actual = composite_bg_only(w, h, &bg, w, h).to_ppm();
        let a = &actual[ppm_header_end(&actual)..];
        let e = &expected[ppm_header_end(&expected)..];

        let mut col_diff = vec![0usize; w as usize];
        let mut row_diff = vec![0usize; h as usize];
        let mut total = 0usize;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 3;
                if a[i] != e[i] || a[i + 1] != e[i + 1] || a[i + 2] != e[i + 2] {
                    total += 1;
                    col_diff[x] += 1;
                    row_diff[y] += 1;
                }
            }
        }

        let sum_range = |vals: &[usize], start: usize, end: usize| -> usize {
            vals[start.min(vals.len())..end.min(vals.len())].iter().sum()
        };
        let max_col = col_diff.iter().enumerate().max_by_key(|(_, v)| *v).unwrap();
        let max_row = row_diff.iter().enumerate().max_by_key(|(_, v)| *v).unwrap();

        eprintln!(
            "colorbook bg profile total={} left32={} right32={} top32={} bottom32={} center={} max_col={}({}) max_row={}({})",
            total,
            sum_range(&col_diff, 0, 32),
            sum_range(&col_diff, w as usize - 32, w as usize),
            sum_range(&row_diff, h as usize - 32, h as usize),
            sum_range(&row_diff, 0, 32),
            sum_range(&col_diff, w as usize / 4, (w as usize * 3) / 4),
            max_col.0,
            max_col.1,
            max_row.0,
            max_row.1,
        );
    }

    #[test]
    fn debug_colorbook_bg_channel_bilinear_candidate() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/colorbook_p1_bg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let planes = page.decode_background_planes().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;

        let sample_plane = |plane: &[i16], src_w: u32, src_h: u32, page_x: u32, page_y: u32| -> i32 {
            let (_reduction, virt_w, virt_h, virt_page_w, virt_page_h) = {
                let red_w = (w + src_w - 1) / src_w;
                let red_h = (h + src_h - 1) / src_h;
                let reduction = red_w.max(red_h).max(1);
                let virt_w = (w / reduction).max(1);
                let virt_h = (h / reduction).max(1);
                let virt_page_w = virt_w * reduction;
                let virt_page_h = virt_h * reduction;
                (reduction, virt_w, virt_h, virt_page_w, virt_page_h)
            };
            let sw = virt_w as f64;
            let sh = virt_h as f64;
            let dw = virt_page_w as f64;
            let dh = virt_page_h as f64;
            let px = page_x.min(virt_page_w - 1);
            let py = page_y.min(virt_page_h - 1);
            let sx = ((px as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((py as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(virt_w.saturating_sub(1)).min(src_w - 1);
            let sy1 = (sy0 + 1).min(virt_h.saturating_sub(1)).min(src_h - 1);
            let fx = sx - sx0 as f64;
            let fy = sy - sy0 as f64;
            let get = |x: u32, y: u32| plane[(y * src_w + x) as usize] as f64;
            let v = get(sx0.min(src_w - 1), sy0.min(src_h - 1)) * (1.0 - fx) * (1.0 - fy)
                + get(sx1, sy0.min(src_h - 1)) * fx * (1.0 - fy)
                + get(sx0.min(src_w - 1), sy1) * (1.0 - fx) * fy
                + get(sx1, sy1) * fx * fy;
            v.round() as i32
        };

        let mut out = Pixmap::white(w, h);
        let cb = planes.cb.as_ref().unwrap();
        let cr = planes.cr.as_ref().unwrap();
        for y in 0..h {
            for x in 0..w {
                let yv = sample_plane(&planes.y, planes.width, planes.height, x, y);
                let bv = sample_plane(cb, planes.width, planes.height, x, y);
                let rv = sample_plane(cr, planes.width, planes.height, x, y);
                let t2 = rv + (rv >> 1);
                let t3 = yv + 128 - (bv >> 2);
                let red = (yv + 128 + t2).clamp(0, 255) as u8;
                let green = (t3 - (t2 >> 1)).clamp(0, 255) as u8;
                let blue = (t3 + (bv << 1)).clamp(0, 255) as u8;
                out.set_rgb(x, y, red, green, blue);
            }
        }

        let stats = diff_stats(&out.to_ppm(), &expected);
        eprintln!(
            "colorbook bg channel-bilinear bytes={} pixels={} mean_abs_rgb=({:.4},{:.4},{:.4}) max_abs_rgb=({},{},{})",
            stats.byte_mismatches,
            stats.pixel_mismatches,
            stats.sum_abs_r as f64 / stats.pixel_count as f64,
            stats.sum_abs_g as f64 / stats.pixel_count as f64,
            stats.sum_abs_b as f64 / stats.pixel_count as f64,
            stats.max_abs_r,
            stats.max_abs_g,
            stats.max_abs_b,
        );
    }

    #[test]
    fn debug_colorbook_bg_xphase_search() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/colorbook_p1_bg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;
        let (_, virt_w, virt_h, virt_page_w, virt_page_h) = layer_virtual_geometry(&bg, w, h);
        let mut best = Vec::new();

        for step in -16..=16 {
            let x_phase = step as f64 / 16.0;
            let mut out = Pixmap::white(w, h);
            for y in 0..h {
                let sh = virt_h as f64;
                let dh = virt_page_h as f64;
                let py = y.min(virt_page_h - 1);
                let sy = ((py as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
                let sy0 = sy as u32;
                let sy1 = (sy0 + 1).min(virt_h.saturating_sub(1)).min(bg.height - 1);
                let fy = sy - sy0 as f64;
                for x in 0..w {
                    let sw = virt_w as f64;
                    let dw = virt_page_w as f64;
                    let px = x.min(virt_page_w - 1);
                    let sx = ((px as f64 + 0.5) * sw / dw - 0.5 + x_phase).clamp(0.0, sw - 1.0);
                    let sx0 = sx as u32;
                    let sx1 = (sx0 + 1).min(virt_w.saturating_sub(1)).min(bg.width - 1);
                    let fx = sx - sx0 as f64;
                    let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
                    let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
                    let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
                    let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
                    let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                        let v = v00 as f64 * (1.0 - fx) * (1.0 - fy)
                            + v10 as f64 * fx * (1.0 - fy)
                            + v01 as f64 * (1.0 - fx) * fy
                            + v11 as f64 * fx * fy;
                        (v + 0.5).clamp(0.0, 255.0) as u8
                    };
                    out.set_rgb(
                        x,
                        y,
                        interp(r00, r10, r01, r11),
                        interp(g00, g10, g01, g11),
                        interp(b00, b10, b01, b11),
                    );
                }
            }
            let stats = diff_stats(&out.to_ppm(), &expected);
            best.push((stats.byte_mismatches, stats.pixel_mismatches, step));
        }

        best.sort_unstable();
        for (rank, (byte_mismatches, pixel_mismatches, step)) in best.into_iter().take(10).enumerate() {
            eprintln!(
                "colorbook bg xphase rank={} step={} phase={:.4} byte_mismatch={} pixel_mismatch={}",
                rank + 1,
                step,
                step as f64 / 16.0,
                byte_mismatches,
                pixel_mismatches,
            );
        }
    }

    #[test]
    fn debug_colorbook_bg_arithmetic_candidates() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/colorbook_p1_bg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;
        let (_, virt_w, virt_h, virt_page_w, virt_page_h) = layer_virtual_geometry(&bg, w, h);

        let compare = |name: &str, sample: &dyn Fn(u32, u32) -> (u8, u8, u8)| {
            let mut out = Pixmap::white(w, h);
            for y in 0..h {
                for x in 0..w {
                    let (r, g, b) = sample(x, y);
                    out.set_rgb(x, y, r, g, b);
                }
            }
            let stats = diff_stats(&out.to_ppm(), &expected);
            eprintln!(
                "colorbook bg arithmetic {} bytes={} pixels={} mean_abs_rgb=({:.4},{:.4},{:.4})",
                name,
                stats.byte_mismatches,
                stats.pixel_mismatches,
                stats.sum_abs_r as f64 / stats.pixel_count as f64,
                stats.sum_abs_g as f64 / stats.pixel_count as f64,
                stats.sum_abs_b as f64 / stats.pixel_count as f64,
            );
        };

        compare("current_float", &|x, y| sample_bilinear_virtual(&bg, x, y, w, h));

        compare("fixed16_direct", &|x, y| {
            let sw = virt_w as i64;
            let sh = virt_h as i64;
            let dw = virt_page_w as i64;
            let dh = virt_page_h as i64;
            let px = x.min(virt_page_w - 1) as i64;
            let py = y.min(virt_page_h - 1) as i64;
            let sx_fp = (((2 * px + 1) * sw << 16) / (2 * dw)) - (1 << 15);
            let sy_fp = (((2 * py + 1) * sh << 16) / (2 * dh)) - (1 << 15);
            let sx_fp = sx_fp.clamp(0, (sw - 1) << 16);
            let sy_fp = sy_fp.clamp(0, (sh - 1) << 16);
            let sx0 = (sx_fp >> 16) as u32;
            let sy0 = (sy_fp >> 16) as u32;
            let sx1 = (sx0 + 1).min(virt_w.saturating_sub(1)).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(virt_h.saturating_sub(1)).min(bg.height - 1);
            let fx = (sx_fp & 0xffff) as i64;
            let fy = (sy_fp & 0xffff) as i64;
            let wx0 = 65536 - fx;
            let wy0 = 65536 - fy;
            let wx1 = fx;
            let wy1 = fy;
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            let interp = |v00: u8, v10: u8, v01: u8, v11: u8| -> u8 {
                let acc = v00 as i64 * wx0 * wy0
                    + v10 as i64 * wx1 * wy0
                    + v01 as i64 * wx0 * wy1
                    + v11 as i64 * wx1 * wy1;
                ((acc + (1 << 31)) >> 32).clamp(0, 255) as u8
            };
            (
                interp(r00, r10, r01, r11),
                interp(g00, g10, g01, g11),
                interp(b00, b10, b01, b11),
            )
        });

        compare("separable_round8", &|x, y| {
            let sw = virt_w as f64;
            let sh = virt_h as f64;
            let dw = virt_page_w as f64;
            let dh = virt_page_h as f64;
            let px = x.min(virt_page_w - 1);
            let py = y.min(virt_page_h - 1);
            let sx = ((px as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((py as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(virt_w.saturating_sub(1)).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(virt_h.saturating_sub(1)).min(bg.height - 1);
            let fx = ((sx - sx0 as f64) * 256.0 + 0.5).floor().clamp(0.0, 255.0) as u32;
            let fy = ((sy - sy0 as f64) * 256.0 + 0.5).floor().clamp(0.0, 255.0) as u32;
            let interp_h = |v0: u8, v1: u8| -> u32 {
                ((v0 as u32 * (256 - fx) + v1 as u32 * fx + 128) >> 8).clamp(0, 255)
            };
            let interp_v = |v0: u32, v1: u32| -> u8 {
                ((v0 * (256 - fy) + v1 * fy + 128) >> 8).clamp(0, 255) as u8
            };
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            (
                interp_v(interp_h(r00, r10), interp_h(r01, r11)),
                interp_v(interp_h(g00, g10), interp_h(g01, g11)),
                interp_v(interp_h(b00, b10), interp_h(b01, b11)),
            )
        });

        compare("separable_floor8", &|x, y| {
            let sw = virt_w as f64;
            let sh = virt_h as f64;
            let dw = virt_page_w as f64;
            let dh = virt_page_h as f64;
            let px = x.min(virt_page_w - 1);
            let py = y.min(virt_page_h - 1);
            let sx = ((px as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
            let sy = ((py as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);
            let sx0 = sx as u32;
            let sy0 = sy as u32;
            let sx1 = (sx0 + 1).min(virt_w.saturating_sub(1)).min(bg.width - 1);
            let sy1 = (sy0 + 1).min(virt_h.saturating_sub(1)).min(bg.height - 1);
            let fx = ((sx - sx0 as f64) * 256.0).floor().clamp(0.0, 255.0) as u32;
            let fy = ((sy - sy0 as f64) * 256.0).floor().clamp(0.0, 255.0) as u32;
            let interp_h = |v0: u8, v1: u8| -> u32 {
                ((v0 as u32 * (256 - fx) + v1 as u32 * fx) >> 8).clamp(0, 255)
            };
            let interp_v = |v0: u32, v1: u32| -> u8 {
                ((v0 * (256 - fy) + v1 * fy) >> 8).clamp(0, 255) as u8
            };
            let (r00, g00, b00) = bg.get_rgb(sx0.min(bg.width - 1), sy0.min(bg.height - 1));
            let (r10, g10, b10) = bg.get_rgb(sx1, sy0.min(bg.height - 1));
            let (r01, g01, b01) = bg.get_rgb(sx0.min(bg.width - 1), sy1);
            let (r11, g11, b11) = bg.get_rgb(sx1, sy1);
            (
                interp_v(interp_h(r00, r10), interp_h(r01, r11)),
                interp_v(interp_h(g00, g10), interp_h(g01, g11)),
                interp_v(interp_h(b00, b10), interp_h(b01, b11)),
            )
        });
    }

    #[test]
    fn debug_colorbook_full_mismatch_profile() {
        let expected = std::fs::read(golden_path().join("colorbook_p1.ppm")).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let mask = page.decode_mask().unwrap().unwrap();
        let actual = render(&page).unwrap().to_ppm();

        let a = &actual[ppm_header_end(&actual)..];
        let e = &expected[ppm_header_end(&expected)..];
        let stats = diff_stats(&actual, &expected);

        let mut fg_pixels = 0usize;
        let mut bg_pixels = 0usize;
        let mut fg_pixel_mismatches = 0usize;
        let mut bg_pixel_mismatches = 0usize;
        let mut fg_byte_mismatches = 0usize;
        let mut bg_byte_mismatches = 0usize;
        let mut fg_sum_abs = [0u64; 3];
        let mut bg_sum_abs = [0u64; 3];

        let w = page.info.width as u32;
        let h = page.info.height as u32;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 3;
                let dr = (a[i] as i16 - e[i] as i16).unsigned_abs() as u8;
                let dg = (a[i + 1] as i16 - e[i + 1] as i16).unsigned_abs() as u8;
                let db = (a[i + 2] as i16 - e[i + 2] as i16).unsigned_abs() as u8;
                let is_fg = x < mask.width as usize && y < mask.height as usize && mask.get(x as u32, y as u32);
                let (pixels, pixel_mismatches, byte_mismatches, sum_abs) = if is_fg {
                    (&mut fg_pixels, &mut fg_pixel_mismatches, &mut fg_byte_mismatches, &mut fg_sum_abs)
                } else {
                    (&mut bg_pixels, &mut bg_pixel_mismatches, &mut bg_byte_mismatches, &mut bg_sum_abs)
                };
                *pixels += 1;
                if dr != 0 || dg != 0 || db != 0 {
                    *pixel_mismatches += 1;
                }
                *byte_mismatches += (dr != 0) as usize + (dg != 0) as usize + (db != 0) as usize;
                sum_abs[0] += dr as u64;
                sum_abs[1] += dg as u64;
                sum_abs[2] += db as u64;
            }
        }

        eprintln!(
            "colorbook full bytes={} pixels={} mean_abs_rgb=({:.4},{:.4},{:.4}) max_abs_rgb=({},{},{})",
            stats.byte_mismatches,
            stats.pixel_mismatches,
            stats.sum_abs_r as f64 / stats.pixel_count as f64,
            stats.sum_abs_g as f64 / stats.pixel_count as f64,
            stats.sum_abs_b as f64 / stats.pixel_count as f64,
            stats.max_abs_r,
            stats.max_abs_g,
            stats.max_abs_b,
        );
        eprintln!(
            "colorbook full fg bytes={} pixels={} mean_abs_rgb=({:.4},{:.4},{:.4})",
            fg_byte_mismatches,
            fg_pixel_mismatches,
            fg_sum_abs[0] as f64 / fg_pixels as f64,
            fg_sum_abs[1] as f64 / fg_pixels as f64,
            fg_sum_abs[2] as f64 / fg_pixels as f64,
        );
        eprintln!(
            "colorbook full bg bytes={} pixels={} mean_abs_rgb=({:.4},{:.4},{:.4})",
            bg_byte_mismatches,
            bg_pixel_mismatches,
            bg_sum_abs[0] as f64 / bg_pixels as f64,
            bg_sum_abs[1] as f64 / bg_pixels as f64,
            bg_sum_abs[2] as f64 / bg_pixels as f64,
        );
    }

    #[test]
    fn debug_colorbook_full_fg_shift_candidates() {
        let expected = std::fs::read(golden_path().join("colorbook_p1.ppm")).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let mask = page.decode_mask().unwrap().unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let fg = page.decode_foreground().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;
        let (reduction, virt_w, virt_h, virt_page_w, virt_page_h) = layer_virtual_geometry(&fg, w, h);

        let compare = |name: &str, x_shift: u32, y_shift: u32| {
            let mut out = Pixmap::white(w, h);
            for y in 0..h {
                for x in 0..w {
                    if x < mask.width && y < mask.height && mask.get(x, y) {
                        let px = (x + x_shift).min(virt_page_w - 1);
                        let py = (y + y_shift).min(virt_page_h - 1);
                        let sx = (px / reduction).min(virt_w - 1).min(fg.width - 1);
                        let sy = (py / reduction).min(virt_h - 1).min(fg.height - 1);
                        let (r, g, b) = fg.get_rgb(sx, sy);
                        out.set_rgb(x, y, r, g, b);
                    } else {
                        let (r, g, b) = sample_bilinear_virtual(&bg, x, y, w, h);
                        out.set_rgb(x, y, r, g, b);
                    }
                }
            }
            let stats = diff_stats(&out.to_ppm(), &expected);
            eprintln!(
                "colorbook full fg shift {} byte_mismatch={} pixel_mismatch={}",
                name,
                stats.byte_mismatches,
                stats.pixel_mismatches,
            );
        };

        for x_shift in 0..reduction.min(3) {
            for y_shift in 0..reduction.min(3) {
                compare(&format!("{}_{}", x_shift, y_shift), x_shift, y_shift);
            }
        }
    }

    #[test]
    fn debug_colorbook_fg_shift_search() {
        let ref_path = std::path::Path::new("/tmp/rdjvu_debug/colorbook_p1_fg.ppm");
        if !ref_path.exists() {
            return;
        }
        let expected = std::fs::read(ref_path).unwrap();
        let data = std::fs::read(assets_path().join("colorbook.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let mask = page.decode_mask().unwrap().unwrap();
        let fg = page.decode_foreground().unwrap().unwrap();
        let w = page.info.width as u32;
        let h = page.info.height as u32;
        let (reduction, virt_w, virt_h, virt_page_w, virt_page_h) = layer_virtual_geometry(&fg, w, h);
        let mut best = Vec::new();

        for x_shift in 0..reduction {
            for y_shift in 0..reduction {
                let mut out = Pixmap::white(w, h);
                let mw = mask.width.min(w);
                let mh = mask.height.min(h);
                for y in 0..mh {
                    for x in 0..mw {
                        if mask.get(x, y) {
                            let px = (x + x_shift).min(virt_page_w - 1);
                            let py = (y + y_shift).min(virt_page_h - 1);
                            let sx = (px / reduction).min(virt_w - 1).min(fg.width - 1);
                            let sy = (py / reduction).min(virt_h - 1).min(fg.height - 1);
                            let (r, g, b) = fg.get_rgb(sx, sy);
                            out.set_rgb(x, y, r, g, b);
                        }
                    }
                }
                let stats = diff_stats(&out.to_ppm(), &expected);
                best.push((stats.pixel_mismatches, stats.byte_mismatches, x_shift, y_shift));
            }
        }

        best.sort_unstable();
        for (rank, (pixel_mismatches, byte_mismatches, x_shift, y_shift)) in best.into_iter().take(10).enumerate() {
            eprintln!(
                "colorbook fg search rank={} shift=({}, {}) pixel_mismatch={} byte_mismatch={}",
                rank + 1,
                x_shift,
                y_shift,
                pixel_mismatches,
                byte_mismatches,
            );
        }
    }

    #[test]
    fn debug_layer_virtual_dims() {
        for (file, page_idx) in [("carte.djvu", 0usize), ("colorbook.djvu", 0usize)] {
            let data = std::fs::read(assets_path().join(file)).unwrap();
            let doc = Document::parse(&data).unwrap();
            let page = doc.page(page_idx).unwrap();
            let bg = page.decode_background().unwrap().unwrap();
            let fg = page.decode_foreground().unwrap().unwrap();
            let w = page.info.width as u32;
            let h = page.info.height as u32;
            let bg_geo = layer_virtual_geometry(&bg, w, h);
            let fg_geo = layer_virtual_geometry(&fg, w, h);
            eprintln!(
                "{} p{} page={}x{} bg={}x{} bg_geo={:?} fg={}x{} fg_geo={:?}",
                file,
                page_idx + 1,
                w,
                h,
                bg.width,
                bg.height,
                bg_geo,
                fg.width,
                fg.height,
                fg_geo,
            );
        }
    }

    #[test]
    fn debug_dump_carte_actual_ppm() {
        let out_dir = std::path::Path::new("/tmp/rdjvu_debug");
        std::fs::create_dir_all(out_dir).unwrap();
        let out_path = out_dir.join("carte_actual.ppm");
        let pm = render_page("carte.djvu", 0);
        std::fs::write(&out_path, pm.to_ppm()).unwrap();
    }

    #[test]
    fn debug_dump_carte_bg_actual_ppm() {
        let out_dir = std::path::Path::new("/tmp/rdjvu_debug");
        std::fs::create_dir_all(out_dir).unwrap();
        let data = std::fs::read(assets_path().join("carte.djvu")).unwrap();
        let doc = Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();
        let bg = page.decode_background().unwrap().unwrap();
        let out_path = out_dir.join("carte_bg_actual.ppm");
        std::fs::write(&out_path, bg.to_ppm()).unwrap();
    }
}
