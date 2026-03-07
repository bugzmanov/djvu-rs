use rdjvu_core::{Bitmap, Error, Pixmap};
use rdjvu_document::{Page, Palette, Rotation};

/// Render a DjVu page to an RGBA pixmap.
pub fn render(page: &Page) -> Result<Pixmap, Error> {
    let w = page.info.width as u32;
    let h = page.info.height as u32;

    let has_palette = page.has_palette();

    let output = if has_palette {
        render_with_palette(page, w, h)?
    } else {
        let mask = page.decode_mask()?;
        let bg = page.decode_background()?;
        let fg = page.decode_foreground()?;

        match (&mask, &bg, &fg) {
            (None, Some(bg), _) => composite_bg_only(w, h, bg),
            (Some(mask), None, None) => composite_bilevel(w, h, mask),
            (Some(mask), Some(bg), Some(fg)) => composite_3layer(w, h, mask, bg, fg),
            (Some(mask), Some(bg), None) => composite_mask_bg(w, h, mask, bg),
            (Some(mask), None, Some(fg)) => composite_mask_fg(w, h, mask, fg),
            (None, None, _) => Pixmap::white(w, h),
        }
    };

    let output = apply_rotation(output, page.info.rotation);
    Ok(output)
}

// ============================================================
// BG-only: upscale background to page dimensions
// ============================================================

fn composite_bg_only(w: u32, h: u32, bg: &Pixmap) -> Pixmap {
    upscale(w, h, bg)
}

// ============================================================
// Mask-only: black where mask=1, white where mask=0
// ============================================================

fn composite_bilevel(w: u32, h: u32, mask: &Bitmap) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    let mw = mask.width.min(w);
    let mh = mask.height.min(h);
    for y in 0..mh {
        for x in 0..mw {
            if mask.get(x, y) {
                out.set_rgb(x, y, 0, 0, 0);
            }
        }
    }
    out
}

// ============================================================
// Mask + BG: background where mask=0, black text where mask=1
// ============================================================

fn composite_mask_bg(w: u32, h: u32, mask: &Bitmap, bg: &Pixmap) -> Pixmap {
    let mut out = upscale(w, h, bg);
    let mw = mask.width.min(w);
    let mh = mask.height.min(h);
    for y in 0..mh {
        for x in 0..mw {
            if mask.get(x, y) {
                out.set_rgb(x, y, 0, 0, 0);
            }
        }
    }
    out
}

// ============================================================
// Mask + FG (no BG): foreground color where mask=1, white elsewhere
// ============================================================

fn composite_mask_fg(w: u32, h: u32, mask: &Bitmap, fg: &Pixmap) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    let fg_scale = layer_scale(w, fg.width);
    let mw = mask.width.min(w);
    let mh = mask.height.min(h);
    for y in 0..mh {
        for x in 0..mw {
            if mask.get(x, y) {
                let (r, g, b) = sample_layer(fg, x, y, fg_scale);
                out.set_rgb(x, y, r, g, b);
            }
        }
    }
    out
}

// ============================================================
// 3-layer: mask selects between FG and BG
// ============================================================

fn composite_3layer(w: u32, h: u32, mask: &Bitmap, bg: &Pixmap, fg: &Pixmap) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    let bg_scale = layer_scale(w, bg.width);
    let fg_scale = layer_scale(w, fg.width);
    for y in 0..h {
        for x in 0..w {
            let is_fg = x < mask.width && y < mask.height && mask.get(x, y);
            if is_fg {
                let (r, g, b) = sample_layer(fg, x, y, fg_scale);
                out.set_rgb(x, y, r, g, b);
            } else {
                let (r, g, b) = sample_layer(bg, x, y, bg_scale);
                out.set_rgb(x, y, r, g, b);
            }
        }
    }
    out
}

// ============================================================
// Palette composite: mask + BG + FGbz palette colors
// ============================================================

fn render_with_palette(page: &Page, w: u32, h: u32) -> Result<Pixmap, Error> {
    let mask_indexed = page.decode_mask_indexed()?;
    let bg = page.decode_background()?;
    let palette = page.decode_palette()?;

    match (mask_indexed, bg, palette) {
        (Some((mask, blit_map)), Some(bg), Some(pal)) => {
            Ok(composite_palette(w, h, &mask, &blit_map, &bg, &pal))
        }
        (Some((mask, blit_map)), None, Some(pal)) => {
            Ok(composite_palette_no_bg(w, h, &mask, &blit_map, &pal))
        }
        (None, Some(bg), _) => Ok(composite_bg_only(w, h, &bg)),
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

fn composite_palette(w: u32, h: u32, mask: &Bitmap, blit_map: &[i32], bg: &Pixmap, pal: &Palette) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    let bg_scale = layer_scale(w, bg.width);
    for y in 0..h {
        for x in 0..w {
            let is_fg = x < mask.width && y < mask.height && mask.get(x, y);
            if is_fg {
                let mi = y as usize * mask.width as usize + x as usize;
                let (r, g, b) = if mi < blit_map.len() {
                    palette_color(pal, blit_map[mi])
                } else {
                    (0, 0, 0)
                };
                out.set_rgb(x, y, r, g, b);
            } else {
                let (r, g, b) = sample_layer(bg, x, y, bg_scale);
                out.set_rgb(x, y, r, g, b);
            }
        }
    }
    out
}

fn composite_palette_no_bg(w: u32, h: u32, mask: &Bitmap, blit_map: &[i32], pal: &Palette) -> Pixmap {
    let mut out = Pixmap::white(w, h);
    let mw = mask.width.min(w);
    let mh = mask.height.min(h);
    for y in 0..mh {
        for x in 0..mw {
            if mask.get(x, y) {
                let mi = y as usize * mask.width as usize + x as usize;
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
// Integer-scale layer sampling via bilinear interpolation.
// ============================================================

fn layer_scale(page_w: u32, layer_w: u32) -> u32 {
    if layer_w == 0 {
        return 1;
    }
    let ratio = page_w as f64 / layer_w as f64;
    ratio.round().max(1.0) as u32
}

fn sample_layer(src: &Pixmap, x: u32, y: u32, scale: u32) -> (u8, u8, u8) {
    if scale <= 1 {
        return src.get_rgb(x.min(src.width - 1), y.min(src.height - 1));
    }
    let dst_w = src.width * scale;
    let dst_h = src.height * scale;
    let sw = src.width as f64;
    let sh = src.height as f64;
    let dw = dst_w as f64;
    let dh = dst_h as f64;
    let sx = ((x as f64 + 0.5) * sw / dw - 0.5).clamp(0.0, sw - 1.0);
    let sy = ((y as f64 + 0.5) * sh / dh - 0.5).clamp(0.0, sh - 1.0);

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

fn upscale(w: u32, h: u32, src: &Pixmap) -> Pixmap {
    if src.width == w && src.height == h {
        return src.clone();
    }
    let scale = layer_scale(w, src.width);
    let mut out = Pixmap::white(w, h);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = sample_layer(src, x, y, scale);
            out.set_rgb(x, y, r, g, b);
        }
    }
    out
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

    fn assert_ppm_match(pixmap: &Pixmap, golden_file: &str) {
        let actual = pixmap.to_ppm();
        let expected = std::fs::read(golden_path().join(golden_file)).unwrap();
        assert_eq!(actual.len(), expected.len(), "{}: size mismatch {} vs {}", golden_file, actual.len(), expected.len());

        let header_end = actual.iter().position(|&b| b == b'\n').unwrap() + 1;
        let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;
        let header_end = header_end + actual[header_end..].iter().position(|&b| b == b'\n').unwrap() + 1;

        let pixel_data_a = &actual[header_end..];
        let pixel_data_e = &expected[header_end..];

        let mut mismatches = 0;
        for i in 0..pixel_data_a.len().min(pixel_data_e.len()) {
            let d = (pixel_data_a[i] as i16 - pixel_data_e[i] as i16).unsigned_abs() as u8;
            if d != 0 {
                mismatches += 1;
            }
        }
        let total = pixel_data_a.len();
        if mismatches > 0 {
            panic!(
                "{}: {} pixel-bytes differ ({}/{} = {:.1}%)",
                golden_file, mismatches,
                mismatches, total,
                mismatches as f64 / total as f64 * 100.0
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
}
