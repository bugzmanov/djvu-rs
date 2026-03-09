use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::path::PathBuf;

fn assets_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../references/djvujs/library/assets")
}

// --- IW44 background decode (ZP + wavelet + colorspace) ---

fn bench_iw44_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("iw44_decode");
    group.sample_size(10);

    for (name, file) in [
        ("big_scanned_1695x2287", "big-scanned-page.djvu"),
        ("carte_350x213", "carte.djvu"),
        ("chicken_181x240", "chicken.djvu"),
        ("colorbook_p0", "colorbook.djvu"),
    ] {
        let data = std::fs::read(assets_path().join(file)).unwrap();
        let doc = rdjvu_document::Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();

        group.bench_function(BenchmarkId::new("bg", name), |b| {
            b.iter(|| page.decode_background().unwrap());
        });
    }
    group.finish();
}

// --- JB2 mask decode ---

fn bench_jb2_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("jb2_decode");
    group.sample_size(10);

    for (name, file) in [
        ("boy_jb2_728x1082", "boy_jb2.djvu"),
        ("carte_4200x2556", "carte.djvu"),
        ("navm_p0_2550x3300", "navm_fgbz.djvu"),
        ("djvu3spec_p0_2539x3295", "DjVu3Spec_bundled.djvu"),
    ] {
        let data = std::fs::read(assets_path().join(file)).unwrap();
        let doc = rdjvu_document::Document::parse(&data).unwrap();
        let page = doc.page(0).unwrap();

        group.bench_function(BenchmarkId::new("mask", name), |b| {
            b.iter(|| page.decode_mask().unwrap());
        });
    }
    group.finish();
}

// --- Full render (decode + composite) ---

fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("render");
    group.sample_size(10);

    for (name, file, pg) in [
        ("big_scanned_bg_only", "big-scanned-page.djvu", 0usize),
        ("carte_3layer", "carte.djvu", 0),
        ("chicken_bg_only", "chicken.djvu", 0),
        ("colorbook_p0_3layer", "colorbook.djvu", 0),
        ("navm_p0_palette", "navm_fgbz.djvu", 0),
        ("djvu3spec_p0_mask", "DjVu3Spec_bundled.djvu", 0),
        ("djvu3spec_p4_mask_bg", "DjVu3Spec_bundled.djvu", 4),
        ("boy_jb2_mask", "boy_jb2.djvu", 0),
    ] {
        let data = std::fs::read(assets_path().join(file)).unwrap();
        let doc = rdjvu_document::Document::parse(&data).unwrap();
        let page = doc.page(pg).unwrap();

        group.bench_function(BenchmarkId::new("page", name), |b| {
            b.iter(|| rdjvu_render::render(&page).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_iw44_decode, bench_jb2_decode, bench_render);
criterion_main!(benches);
