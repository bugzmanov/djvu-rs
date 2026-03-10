#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut zp = djvu::zp::ZPDecoder::new(data);
    let mut ctx: u8 = 0;
    for _ in 0..10_000 {
        let _ = zp.decode(&mut ctx);
    }
});
