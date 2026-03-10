//! Print page dimensions, DPI, and rotation for every page.
//!
//! Usage: cargo run --example page_info -- <file.djvu>

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("Usage: page_info <file.djvu>");

    let doc = djvu::Document::open(path).unwrap();
    println!("{} pages", doc.page_count());

    for i in 0..doc.page_count() {
        let page = doc.page(i).unwrap();
        println!(
            "  Page {:3}: {:5}x{:<5} {:3} dpi  rotation={:?}  display={}x{}",
            i + 1,
            page.width(),
            page.height(),
            page.dpi(),
            page.rotation(),
            page.display_width(),
            page.display_height(),
        );
    }
}
