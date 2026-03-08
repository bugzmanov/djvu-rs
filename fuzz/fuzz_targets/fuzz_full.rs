#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Limit input to avoid OOM from huge decoded images
    if data.len() > 200_000 {
        return;
    }
    // Full pipeline: parse document, then try to render page 0
    if let Ok(doc) = rdjvu_document::Document::parse(data) {
        if let Ok(page) = doc.page(0) {
            let _ = rdjvu_render::render(&page);
        }
    }
});
