#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Must not panic on arbitrary input
    let _ = rdjvu_iff::parse(data);
});
