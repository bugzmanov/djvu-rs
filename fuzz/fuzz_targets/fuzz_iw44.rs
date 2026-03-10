#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut img = djvu::iw44::IW44Image::new();
    if img.decode_chunk(data).is_ok() {
        let _ = img.to_pixmap();
    }
});
