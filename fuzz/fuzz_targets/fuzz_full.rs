#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 200_000 {
        return;
    }
    if let Ok(doc) = djvu::Document::from_bytes(data.to_vec()) {
        if let Ok(page) = doc.page(0) {
            let _ = page.render();
        }
    }
});
