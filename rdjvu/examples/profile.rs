use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("usage: profile <file.djvu> [max_pages]");
    let max_pages: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(999);

    let t0 = Instant::now();
    let data = std::fs::read(path).unwrap();
    let doc = rdjvu_document::Document::parse(&data).unwrap();
    let parse_time = t0.elapsed();
    let n = doc.page_count().min(max_pages);
    println!("{} pages, parse: {:.1}ms", doc.page_count(), parse_time.as_secs_f64() * 1000.0);

    for i in 0..n {
        let page = doc.page(i).unwrap();
        let dims = format!("{}x{}", page.info.width, page.info.height);

        // Time individual decode steps
        let t = Instant::now();
        let _mask = page.decode_mask();
        let mask_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let _bg = page.decode_background();
        let bg_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let _fg = page.decode_foreground();
        let fg_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let _pal = page.decode_palette();
        let pal_ms = t.elapsed().as_secs_f64() * 1000.0;

        // Time full render (re-decodes + composites)
        let t = Instant::now();
        let _render = rdjvu_render::render(&page);
        let render_ms = t.elapsed().as_secs_f64() * 1000.0;

        let decode_total = mask_ms + bg_ms + fg_ms + pal_ms;
        // render = re-decode + composite, so composite ≈ render - decode_total
        let composite_est = render_ms - decode_total;

        println!(
            "  p{:3} {} mask={:.0} bg={:.0} fg={:.0} pal={:.0} | composite≈{:.0} | render={:.0}ms",
            i, dims, mask_ms, bg_ms, fg_ms, pal_ms, composite_est, render_ms
        );
    }
}
