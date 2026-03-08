#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Keep inputs small to avoid slow iterations from large bitmap allocations
    if data.len() > 10_000 {
        return;
    }
    // Without shared dict
    let _ = rdjvu_jb2::decode(data, None);
    // As dictionary
    let _ = rdjvu_jb2::decode_dict(data, None);
});
