//! Render a DjVu page to a PPM file.
//!
//! Usage: cargo run --example render_page -- <file.djvu> [page] [output.ppm]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("Usage: render_page <file.djvu> [page] [output.ppm]");
    let page_idx: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(0);
    let output = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/page.ppm");

    let doc = djvu::Document::open(path).unwrap();
    let page = doc.page(page_idx).unwrap();

    eprintln!(
        "Page {}: {}x{} @ {} dpi, rotation={:?}",
        page.index(),
        page.width(),
        page.height(),
        page.dpi(),
        page.rotation(),
    );

    let pixmap = page.render().unwrap();
    std::fs::write(output, pixmap.to_ppm()).unwrap();
    eprintln!("Wrote {}", output);
}
