//! Extract raw chunk data from DjVu files to seed fuzz corpus.
//! Run with: cargo run --example extract_corpus

use rdjvu_iff::Chunk;

fn collect_leaves<'a>(chunk: &'a Chunk<'a>, target: &[u8; 4], out: &mut Vec<&'a [u8]>) {
    match chunk {
        Chunk::Leaf { id, .. } if id == target => {
            out.push(chunk.data());
        }
        Chunk::Form { children, .. } => {
            for child in children {
                collect_leaves(child, target, out);
            }
        }
        _ => {}
    }
}

fn main() {
    let assets = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("references/djvujs/library/assets");
    let corpus = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fuzz/corpus");

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

        for (tag, dir) in [("Sjbz", "fuzz_jb2"), ("Djbz", "fuzz_jb2"), ("BG44", "fuzz_iw44"), ("FG44", "fuzz_iw44")] {
            let mut chunks = Vec::new();
            collect_leaves(&file.root, tag.as_bytes().try_into().unwrap(), &mut chunks);
            for (i, data) in chunks.iter().enumerate() {
                let path = corpus.join(dir).join(format!("{}_{}{}.bin", stem, tag.to_lowercase(), i));
                std::fs::write(&path, data).unwrap();
                println!("Wrote {} ({} bytes)", path.display(), data.len());
            }
        }
    }
    println!("Done seeding corpus.");
}
