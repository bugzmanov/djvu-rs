use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::path::PathBuf;

fn assets_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("references/djvujs/library/assets")
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
        let doc = djvu::Document::open(assets_path().join(file)).unwrap();

        group.bench_function(BenchmarkId::new("page", name), |b| {
            b.iter(|| {
                let page = doc.page(pg).unwrap();
                page.render().unwrap()
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
