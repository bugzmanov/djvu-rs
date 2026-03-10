use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("usage: profile <file.djvu> [max_pages]");
    let max_pages: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(999);

    let t0 = Instant::now();
    let doc = djvu::Document::open(path).unwrap();
    let parse_time = t0.elapsed();
    let n = doc.page_count().min(max_pages);
    println!("{} pages, parse: {:.1}ms", doc.page_count(), parse_time.as_secs_f64() * 1000.0);

    for i in 0..n {
        let page = doc.page(i).unwrap();
        let dims = format!("{}x{}", page.width(), page.height());

        // Time full render
        let t = Instant::now();
        let _render = page.render();
        let render_ms = t.elapsed().as_secs_f64() * 1000.0;

        println!(
            "  p{:3} {} render={:.0}ms",
            i, dims, render_ms
        );
    }
}
