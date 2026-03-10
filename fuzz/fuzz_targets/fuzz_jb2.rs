#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 10_000 {
        return;
    }
    let _ = djvu::jb2::decode(data, None);
    let _ = djvu::jb2::decode_dict(data, None);
});
