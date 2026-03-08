//! Extract raw chunk data from DjVu files to seed fuzz corpus.
//! Run with: cargo run --manifest-path fuzz/extract_corpus_runner/Cargo.toml

fn main() {
    let assets = std::path::Path::new("references/djvujs/library/assets");
    let corpus = std::path::Path::new("fuzz/corpus");

    let files = [
        "boy_jb2.djvu",
        "boy.djvu",
        "chicken.djvu",
        "carte.djvu",
        "navm_fgbz.djvu",
        "DjVu3Spec_bundled.djvu",
    ];

    for filename in &files {
        let data = std::fs::read(assets.join(filename)).unwrap();
        let file = rdjvu_iff::parse(&data).unwrap();
        let stem = filename.trim_end_matches(".djvu");

        // Extract Sjbz chunks for JB2 fuzzer
        for (i, chunk) in file.root.find_all(b"Sjbz").iter().enumerate() {
            let path = corpus.join("fuzz_jb2").join(format!("{}_sjbz_{}.bin", stem, i));
            std::fs::write(&path, chunk.data()).unwrap();
            println!("Wrote {}", path.display());
        }

        // Extract Djbz chunks for JB2 fuzzer
        for (i, chunk) in file.root.find_all(b"Djbz").iter().enumerate() {
            let path = corpus.join("fuzz_jb2").join(format!("{}_djbz_{}.bin", stem, i));
            std::fs::write(&path, chunk.data()).unwrap();
            println!("Wrote {}", path.display());
        }

        // Extract BG44/FG44 chunks for IW44 fuzzer
        for (i, chunk) in file.root.find_all(b"BG44").iter().enumerate() {
            let path = corpus.join("fuzz_iw44").join(format!("{}_bg44_{}.bin", stem, i));
            std::fs::write(&path, chunk.data()).unwrap();
            println!("Wrote {}", path.display());
        }
        for (i, chunk) in file.root.find_all(b"FG44").iter().enumerate() {
            let path = corpus.join("fuzz_iw44").join(format!("{}_fg44_{}.bin", stem, i));
            std::fs::write(&path, chunk.data()).unwrap();
            println!("Wrote {}", path.display());
        }
    }
}
