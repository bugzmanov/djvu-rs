//! Extract text from all pages of a DjVu document.
//!
//! Usage: cargo run --example extract_text -- <file.djvu>

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("Usage: extract_text <file.djvu>");

    let doc = rdjvu::Document::open(path).unwrap();

    for i in 0..doc.page_count() {
        let page = doc.page(i).unwrap();
        match page.text().unwrap() {
            Some(text) => {
                println!("--- Page {} ---", i + 1);
                println!("{}", text);
            }
            None => {
                eprintln!("Page {}: no text layer", i + 1);
            }
        }
    }
}
