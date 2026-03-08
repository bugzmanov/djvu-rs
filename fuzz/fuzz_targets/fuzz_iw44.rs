#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut img = rdjvu_iw44::IW44Image::new();
    // Try decoding as a single chunk
    if img.decode_chunk(data).is_ok() {
        let _ = img.to_pixmap();
    }
});
