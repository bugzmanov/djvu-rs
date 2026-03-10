use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();
    let djvu_path = args
        .get(1)
        .expect("Usage: before_after <file.djvu> [page] [boldness]");
    let page_idx: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(0);
    let boldness: f32 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(0.7);

    let doc = rdjvu::Document::open(djvu_path).unwrap();
    let page = doc.page(page_idx).unwrap();

    let native_w = page.display_width();
    let native_h = page.display_height();

    // Simulate a typical terminal display: ~40% of native width
    let display_w = (native_w * 2 / 5).max(1);
    let display_h = (native_h * 2 / 5).max(1);

    eprintln!(
        "Page {}: {}x{} native → {}x{} display, boldness={}",
        page_idx, native_w, native_h, display_w, display_h, boldness
    );

    // "Before" — render at native, naive point-sample down (what the terminal does)
    let native = page.render().unwrap();
    let before = point_downsample(&native, display_w, display_h);
    write_ppm("/tmp/djvu_before.ppm", &before, display_w, display_h);
    eprintln!("Wrote /tmp/djvu_before.ppm (terminal point-sample downscale)");

    // "After" — render_aa: native render + box downsample with contrast boost
    let after = page.render_aa(display_w, display_h, boldness).unwrap();
    write_ppm("/tmp/djvu_after.ppm", &after.to_rgb(), after.width, after.height);
    eprintln!("Wrote /tmp/djvu_after.ppm (AA with boldness {})", boldness);
}

fn point_downsample(src: &rdjvu::Pixmap, tw: u32, th: u32) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((tw * th * 3) as usize);
    for y in 0..th {
        for x in 0..tw {
            let sx = ((x as f64 + 0.5) * src.width as f64 / tw as f64) as u32;
            let sy = ((y as f64 + 0.5) * src.height as f64 / th as f64) as u32;
            let (r, g, b) = src.get_rgb(sx.min(src.width - 1), sy.min(src.height - 1));
            rgb.push(r);
            rgb.push(g);
            rgb.push(b);
        }
    }
    rgb
}

fn write_ppm(path: &str, rgb: &[u8], w: u32, h: u32) {
    let header = format!("P6\n{} {}\n255\n", w, h);
    let mut out = Vec::with_capacity(header.len() + rgb.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(rgb);
    fs::write(path, &out).unwrap();
}
